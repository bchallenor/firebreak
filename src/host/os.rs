use std::prelude::v1::*;

use ipnet::IpNet;
use log::*;
use std::ffi::OsStr;
use std::io;

use crate::conn::os::OsNsConnPath;
use crate::host::*;
use crate::os::OsNs;

#[derive(Debug)]
pub struct OsHost {
    name: String,
    ns: OsNs,
}

#[derive(Debug)]
pub struct OsInterface {
    name: String,
    addr_with_net: IpNet,
    peer_name: String,
    peer_ns: OsNs,
}

impl Host for OsHost {
    type Interface = OsInterface;

    fn new(name: String) -> Result<Self, io::Error> {
        let mut ns = OsNs::new_net()?;
        ns.enable_link("lo")?;
        Ok(OsHost { name, ns })
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn new_interface(
        &mut self,
        name: String,
        addr_with_net: IpNet,
    ) -> Result<Self::Interface, io::Error> {
        let peer_name = self.name.clone();
        let mut peer_ns = OsNs::new_net()?;
        peer_ns.enable_link("lo")?;

        self.ns.add_veth_link(&name, &peer_name)?;
        self.ns.move_link(&peer_name, &mut peer_ns)?;

        self.ns.enable_link(&name)?;
        self.ns.set_addr(&name, addr_with_net)?;

        peer_ns.enable_link(&peer_name)?;

        Ok(OsInterface {
            name,
            addr_with_net,
            peer_name,
            peer_ns,
        })
    }

    fn load_nft_rules<R: io::Read + Send>(&mut self, rules: R) -> Result<(), io::Error> {
        self.ns.load_nft_rules(rules)
    }

    fn list_nft_rules(&self) -> Result<String, io::Error> {
        self.ns.list_nft_rules()
    }

    fn input_path<'a>(
        interface: &'a mut Self::Interface,
        host: &'a Self,
    ) -> Result<Box<dyn ConnPath + 'a>, io::Error> {
        let peer_addr_with_net = random_peer_addr_with_net(interface.addr_with_net);
        interface
            .peer_ns
            .set_addr(&interface.peer_name, peer_addr_with_net)?;
        trace!("{}:\n{}", interface.name, interface.peer_ns.list_addrs()?);
        trace!("{}:\n{}", host.name, host.ns.list_addrs()?);
        Ok(Box::new(OsNsConnPath::new(
            &interface.name,
            &interface.peer_ns,
            peer_addr_with_net.addr(),
            &host.name,
            &host.ns,
            interface.addr_with_net.addr(),
        )))
    }

    fn output_path<'a>(
        host: &'a Self,
        interface: &'a mut Self::Interface,
    ) -> Result<Box<dyn ConnPath + 'a>, io::Error> {
        let peer_addr_with_net = random_peer_addr_with_net(interface.addr_with_net);
        interface
            .peer_ns
            .set_addr(&interface.peer_name, peer_addr_with_net)?;
        trace!("{}:\n{}", host.name, host.ns.list_addrs()?);
        trace!("{}:\n{}", interface.name, interface.peer_ns.list_addrs()?);
        Ok(Box::new(OsNsConnPath::new(
            &host.name,
            &host.ns,
            interface.addr_with_net.addr(),
            &interface.name,
            &interface.peer_ns,
            peer_addr_with_net.addr(),
        )))
    }

    fn forward_path<'a>(
        source_interface: &'a mut Self::Interface,
        target_interface: &'a mut Self::Interface,
    ) -> Result<Box<dyn ConnPath + 'a>, io::Error> {
        let source_peer_addr_with_net = random_peer_addr_with_net(source_interface.addr_with_net);
        source_interface
            .peer_ns
            .set_addr(&source_interface.peer_name, source_peer_addr_with_net)?;
        source_interface
            .peer_ns
            .set_default_route(source_interface.addr_with_net.addr())?;
        let target_peer_addr_with_net = random_peer_addr_with_net(target_interface.addr_with_net);
        target_interface
            .peer_ns
            .set_addr(&target_interface.peer_name, target_peer_addr_with_net)?;
        target_interface
            .peer_ns
            .set_default_route(target_interface.addr_with_net.addr())?;
        trace!(
            "{}:\n{}",
            source_interface.name,
            source_interface.peer_ns.list_addrs()?
        );
        trace!(
            "{}:\n{}",
            target_interface.name,
            target_interface.peer_ns.list_addrs()?
        );
        Ok(Box::new(OsNsConnPath::new(
            &source_interface.name,
            &source_interface.peer_ns,
            source_peer_addr_with_net.addr(),
            &target_interface.name,
            &target_interface.peer_ns,
            target_peer_addr_with_net.addr(),
        )))
    }
}

impl Interface for OsInterface {
    fn name(&self) -> &str {
        &self.name
    }

    fn addr_with_net(&self) -> IpNet {
        self.addr_with_net
    }
}

impl OsNs {
    fn add_veth_link(&mut self, name: &str, peer_name: &str) -> Result<(), io::Error> {
        self.scoped_process(
            "ip",
            &[
                "link", "add", name, "type", "veth", "peer", "name", peer_name,
            ],
        )?;
        Ok(())
    }

    fn enable_link(&mut self, name: &str) -> Result<(), io::Error> {
        self.scoped_process("ip", &["link", "set", name, "up"])?;
        Ok(())
    }

    fn move_link(&mut self, name: &str, other: &mut Self) -> Result<(), io::Error> {
        self.scoped_process(
            "ip",
            &[
                OsStr::new("link"),
                OsStr::new("set"),
                OsStr::new(name),
                OsStr::new("netns"),
                other.path().as_os_str(),
            ],
        )?;
        Ok(())
    }

    fn set_addr(&mut self, name: &str, addr: IpNet) -> Result<(), io::Error> {
        self.scoped_process("ip", &["address", "flush", "dev", name])?;
        match addr {
            IpNet::V4(_) => {
                self.scoped_process("ip", &["address", "add", &addr.to_string(), "dev", name])?
            }
            IpNet::V6(_) => {
                // Disable duplicate address detection (DAD) so we can immediately bind the address
                self.scoped_process(
                    "ip",
                    &["address", "add", &addr.to_string(), "dev", name, "nodad"],
                )?
            }
        };
        Ok(())
    }

    fn list_addrs(&self) -> Result<String, io::Error> {
        let ret = self.scoped_process("ip", &["address"])?;
        Ok(ret)
    }

    fn set_default_route(&mut self, addr: IpAddr) -> Result<(), io::Error> {
        self.scoped_process("ip", &["route", "add", "default", "via", &addr.to_string()])?;
        Ok(())
    }

    fn load_nft_rules<R: io::Read + Send>(&mut self, rules: R) -> Result<(), io::Error> {
        self.scoped_process_with_input("nft", &["-f", "-"], rules)?;
        Ok(())
    }

    fn list_nft_rules(&self) -> Result<String, io::Error> {
        let ret = self.scoped_process("nft", &["list", "ruleset"])?;
        Ok(ret)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;
    use lazy_static::lazy_static;
    use paste::paste;

    use crate::conn::{ConnResult, ConnSpec};
    use crate::INIT;

    lazy_static! {
        static ref IPV4_ADDR_WITH_NET: Ipv4Net = "203.0.113.1/24".parse().unwrap();
        static ref IPV6_ADDR_WITH_NET: Ipv6Net = "2001:db8::1/64".parse().unwrap();
    }

    const TCP_SPEC: ConnSpec = ConnSpec::Tcp { port: 80 };
    const UDP_SPEC: ConnSpec = ConnSpec::Udp { port: 53 };

    const EMPTY_INPUT_RULES: &'static str = indoc! {r#"
        table inet filter {
        }
    "#};

    const ACCEPT_INPUT_RULES: &'static str = indoc! {r#"
        table inet filter {
            chain input {
                type filter hook input priority filter; policy accept;
                counter
            }
        }
    "#};

    const DROP_INPUT_RULES: &'static str = indoc! {r#"
        table inet filter {
            chain input {
                type filter hook input priority filter; policy drop;
                counter
            }
        }
    "#};

    async fn test_input<F>(
        addr_with_net: IpNet,
        spec: ConnSpec,
        rules: &str,
        expectation: F,
    ) -> Result<(), io::Error>
    where
        F: Fn(&dyn ConnPath) -> ConnResult,
    {
        *INIT;

        let mut router = OsHost::new("router".into())?;
        let mut wan = router.new_interface("wan".into(), addr_with_net)?;

        router.load_nft_rules(rules.as_bytes())?;

        let path = OsHost::input_path(&mut wan, &router)?;
        let expected_conn_result = expectation(&*path);

        let conn_result = path.connect(spec).await?;

        debug!("Firewall state:\n{}", router.list_nft_rules()?);
        assert_eq!(expected_conn_result, conn_result);

        Ok(())
    }

    fn expect_empty(path: &dyn ConnPath) -> ConnResult {
        ConnResult::Ok {
            source_addr: path.source_addr(),
        }
    }

    fn expect_accept(path: &dyn ConnPath) -> ConnResult {
        ConnResult::Ok {
            source_addr: path.source_addr(),
        }
    }

    fn expect_drop(_path: &dyn ConnPath) -> ConnResult {
        ConnResult::Unreachable
    }

    macro_rules! gen_test {
        ($direction:ident, $firewall:ident, $layer4:ident, $layer3:ident) => {
            paste! {
                #[tokio::test]
                async fn [< test_ $firewall _firewall _with_ $layer4 _over_ $layer3 _ $direction >]() -> Result<(), io::Error> {
                    [< test_ $direction >](
                        IpNet::from(*[< $layer3:snake:upper _ADDR_WITH_NET >]),
                        [< $layer4:snake:upper _SPEC >],
                        [< $firewall:snake:upper _ $direction:snake:upper _RULES >],
                        [< expect_ $firewall >]
                    ).await
                }
            }
        };
    }

    // TODO: test output/forward as well as input
    // TODO: test reject as well as drop
    gen_test!(input, empty, tcp, ipv4);
    gen_test!(input, empty, tcp, ipv6);
    gen_test!(input, empty, udp, ipv4);
    gen_test!(input, empty, udp, ipv6);
    gen_test!(input, accept, tcp, ipv4);
    gen_test!(input, accept, tcp, ipv6);
    gen_test!(input, accept, udp, ipv4);
    gen_test!(input, accept, udp, ipv6);
    gen_test!(input, drop, tcp, ipv4);
    gen_test!(input, drop, tcp, ipv6);
    gen_test!(input, drop, udp, ipv4);
    gen_test!(input, drop, udp, ipv6);
}
