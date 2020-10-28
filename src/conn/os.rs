use std::prelude::v1::*;

use async_trait::async_trait;
use futures::future::{AbortHandle, Abortable, Aborted};
use futures::prelude::*;
use futures::{try_join, FutureExt};
use log::*;
use std::io;
use std::net::IpAddr;
use tokio::net::{TcpListener, TcpSocket, UdpSocket};
use tokio::prelude::*;
use tokio::time::error::Elapsed;
use tokio::time::{timeout, Duration};

use crate::conn::*;
use crate::os::OsNs;

pub struct OsNsConnPath<'a> {
    source_name: &'a str,
    source: &'a OsNs,
    source_addr: IpAddr,
    target_name: &'a str,
    target: &'a OsNs,
    target_addr: IpAddr,
}

impl<'a> OsNsConnPath<'a> {
    pub fn new(
        source_name: &'a str,
        source: &'a OsNs,
        source_addr: IpAddr,
        target_name: &'a str,
        target: &'a OsNs,
        target_addr: IpAddr,
    ) -> OsNsConnPath<'a> {
        OsNsConnPath {
            source_name,
            source,
            source_addr,
            target_name,
            target,
            target_addr,
        }
    }
}

#[async_trait]
impl<'a> ConnPath for OsNsConnPath<'a> {
    fn source_name(&self) -> &str {
        &self.source_name
    }

    fn source_addr(&self) -> IpAddr {
        self.source_addr
    }

    fn target_name(&self) -> &str {
        &self.target_name
    }

    fn target_addr(&self) -> IpAddr {
        self.target_addr
    }

    async fn connect(&self, spec: ConnSpec) -> Result<ConnResult, io::Error> {
        info!(
            "Attempting to connect from {} ({}) to {} ({}) via {:?}",
            self.source_name, self.source_addr, self.target_name, self.target_addr, spec
        );
        let timeout = Duration::from_secs(2);
        let result = match spec {
            ConnSpec::Tcp { port } => Tcp { port }.connect_with_timeout(&self, timeout).await,
            ConnSpec::Udp { port } => Udp { port }.connect_with_timeout(&self, timeout).await,
        }?;
        info!(
            "Attempt to connect from {} ({}) to {} ({}) via {:?} resulted in: {:?}",
            self.source_name, self.source_addr, self.target_name, self.target_addr, spec, result,
        );
        Ok(result)
    }
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum ClientStatus {
    SentCookie(SentCookie),
    Refused,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
struct SentCookie {
    cookie: u128,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
enum ServerStatus {
    ReceivedCookie(ReceivedCookie),
    Aborted,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
struct ReceivedCookie {
    cookie: u128,
    peer_addr: IpAddr,
}

#[async_trait]
trait OsNsConnector: Sized + Sync {
    type ServerSocket: Send;

    async fn bind_server(
        &self,
        target: &OsNs,
        target_addr: IpAddr,
    ) -> Result<Self::ServerSocket, io::Error>;

    async fn server(&self, socket: Self::ServerSocket) -> Result<ServerStatus, io::Error>;

    async fn client(
        &self,
        source: &OsNs,
        source_addr: IpAddr,
        target_addr: IpAddr,
    ) -> Result<ClientStatus, io::Error>;

    async fn connect_with_timeout<'a>(
        &self,
        path: &OsNsConnPath<'a>,
        duration: Duration,
    ) -> Result<ConnResult, io::Error> {
        timeout(duration, self.connect(path))
            .unwrap_or_else(|Elapsed { .. }| Ok(ConnResult::Unreachable))
            .await
    }

    async fn connect<'a>(&self, path: &OsNsConnPath<'a>) -> Result<ConnResult, io::Error> {
        // Ensure the server is bound, with any errors handled, before we start the client
        debug!("Binding server...");
        let listener = self.bind_server(path.target, path.target_addr).await?;
        debug!("Bound server");

        let (server_abort_handle, server_abort_reg) = AbortHandle::new_pair();

        let server =
            Abortable::new(self.server(listener), server_abort_reg).unwrap_or_else(|_: Aborted| {
                debug!("Aborted server");
                Ok(ServerStatus::Aborted)
            });

        let client = self
            .client(path.source, path.source_addr, path.target_addr)
            .inspect(|r| match r {
                Ok(ClientStatus::SentCookie(_)) => (),
                Ok(ClientStatus::Refused) | Err(_) => {
                    server_abort_handle.abort();
                }
            });

        debug!("Running client and server...");
        match try_join!(client, server)? {
            (ClientStatus::SentCookie(tx), ServerStatus::ReceivedCookie(rx)) => {
                assert_eq!(rx.cookie, tx.cookie);
                Ok(ConnResult::Ok {
                    source_addr: rx.peer_addr,
                })
            }
            (ClientStatus::Refused, ServerStatus::Aborted) => Ok(ConnResult::Refused),
            other => unreachable!("Invalid state: {:?}", other),
        }
    }
}

struct Tcp {
    port: u16,
}

struct Udp {
    port: u16,
}

#[async_trait]
impl OsNsConnector for Tcp {
    type ServerSocket = TcpListener;

    async fn bind_server(
        &self,
        target: &OsNs,
        target_addr: IpAddr,
    ) -> Result<TcpListener, io::Error> {
        let socket = target.scoped(|| match target_addr {
            IpAddr::V4(_) => TcpSocket::new_v4(),
            IpAddr::V6(_) => TcpSocket::new_v6(),
        })?;
        socket.bind((target_addr, self.port).into())?;
        socket.listen(1)
    }

    async fn server(&self, socket: TcpListener) -> Result<ServerStatus, io::Error> {
        let (mut stream, peer_addr) = socket.accept().await?;
        debug!("Accepted connection");
        let cookie = stream.read_u128().await?;
        debug!("Received cookie {} from {}", cookie, peer_addr);
        Ok(ServerStatus::ReceivedCookie(ReceivedCookie {
            cookie,
            peer_addr: peer_addr.ip(),
        }))
    }

    async fn client(
        &self,
        source: &OsNs,
        _source_addr: IpAddr,
        target_addr: IpAddr,
    ) -> Result<ClientStatus, io::Error> {
        debug!("Connecting");
        let socket = source.scoped(|| match target_addr {
            IpAddr::V4(_) => TcpSocket::new_v4(),
            IpAddr::V6(_) => TcpSocket::new_v6(),
        })?;
        match socket.connect((target_addr, self.port).into()).await {
            Ok(mut stream) => {
                debug!("Connected");
                let cookie: u128 = rand::random();
                stream.write_u128(cookie).await?;
                debug!("Sent cookie: {:?}", cookie);
                Ok(ClientStatus::SentCookie(SentCookie { cookie }))
            }
            Err(err) if err.raw_os_error() == Some(libc::ECONNREFUSED) => {
                debug!("Refused");
                Ok(ClientStatus::Refused)
            }
            Err(err) => Err(err),
        }
    }
}

#[async_trait]
impl OsNsConnector for Udp {
    type ServerSocket = UdpSocket;

    async fn bind_server(
        &self,
        target: &OsNs,
        target_addr: IpAddr,
    ) -> Result<UdpSocket, io::Error> {
        target
            .scoped(|| std::net::UdpSocket::bind((target_addr, self.port)))
            .and_then(UdpSocket::from_std)
    }

    async fn server(&self, socket: UdpSocket) -> Result<ServerStatus, io::Error> {
        let mut buf = 0u128.to_be_bytes();
        let (size, peer_addr) = socket.recv_from(&mut buf).await?;
        debug!("Received packet");
        assert_eq!(size, buf.len());
        let cookie = u128::from_be_bytes(buf);
        debug!("Received cookie {} from {}", cookie, peer_addr);
        Ok(ServerStatus::ReceivedCookie(ReceivedCookie {
            cookie,
            peer_addr: peer_addr.ip(),
        }))
    }

    async fn client(
        &self,
        source: &OsNs,
        source_addr: IpAddr,
        target_addr: IpAddr,
    ) -> Result<ClientStatus, io::Error> {
        debug!("Connecting");
        let socket: UdpSocket = source
            .scoped(|| std::net::UdpSocket::bind((source_addr, 0)))
            .and_then(UdpSocket::from_std)?;
        socket.connect((target_addr, self.port)).await?;
        debug!("Connected");
        let cookie: u128 = rand::random();
        socket.send(&cookie.to_be_bytes()).await?;
        debug!("Sent cookie: {:?}", cookie);
        match socket.take_error()? {
            None => Ok(ClientStatus::SentCookie(SentCookie { cookie })),
            Some(err) if err.raw_os_error() == Some(libc::ECONNREFUSED) => {
                debug!("Refused");
                Ok(ClientStatus::Refused)
            }
            Some(err) => Err(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use lazy_static::lazy_static;
    use std::net::{Ipv4Addr, Ipv6Addr};

    use crate::INIT;

    lazy_static! {
        static ref NS: OsNs = {
            *INIT;
            let ns = OsNs::new_net().expect("Failed to create new network namespace");
            ns.scoped_process("ip", &["link", "set", "lo", "up"])
                .expect("Failed to enable loopback interface in network namespace");
            ns
        };
        static ref IPV4_LOCALHOST_CONN_PATH: OsNsConnPath<'static> = {
            OsNsConnPath::new(
                "source",
                &NS,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                "target",
                &NS,
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            )
        };
        static ref IPV6_LOCALHOST_CONN_PATH: OsNsConnPath<'static> = {
            OsNsConnPath::new(
                "source",
                &NS,
                IpAddr::V6(Ipv6Addr::LOCALHOST),
                "target",
                &NS,
                IpAddr::V6(Ipv6Addr::LOCALHOST),
            )
        };
    }

    #[tokio::test]
    async fn tcp_v4_ok() -> Result<(), io::Error> {
        let connector = Tcp { port: 1 };
        let result = connector.connect(&IPV4_LOCALHOST_CONN_PATH).await?;
        assert_eq!(
            ConnResult::Ok {
                source_addr: IpAddr::V4(Ipv4Addr::LOCALHOST)
            },
            result
        );
        Ok(())
    }

    #[tokio::test]
    async fn tcp_v6_ok() -> Result<(), io::Error> {
        let connector = Tcp { port: 1 };
        let result = connector.connect(&IPV6_LOCALHOST_CONN_PATH).await?;
        assert_eq!(
            ConnResult::Ok {
                source_addr: IpAddr::V6(Ipv6Addr::LOCALHOST)
            },
            result
        );
        Ok(())
    }

    #[tokio::test]
    async fn udp_v4_ok() -> Result<(), io::Error> {
        let connector = Udp { port: 1 };
        let result = connector.connect(&IPV4_LOCALHOST_CONN_PATH).await?;
        assert_eq!(
            ConnResult::Ok {
                source_addr: IpAddr::V4(Ipv4Addr::LOCALHOST)
            },
            result
        );
        Ok(())
    }

    #[tokio::test]
    async fn udp_v6_ok() -> Result<(), io::Error> {
        let connector = Udp { port: 1 };
        let result = connector.connect(&IPV6_LOCALHOST_CONN_PATH).await?;
        assert_eq!(
            ConnResult::Ok {
                source_addr: IpAddr::V6(Ipv6Addr::LOCALHOST)
            },
            result
        );
        Ok(())
    }
}
