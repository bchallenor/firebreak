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

    use indoc::formatdoc;
    use lazy_static::lazy_static;
    use paste::paste;

    use crate::conn::{ConnEffect, ConnSpec};
    use crate::INIT;

    lazy_static! {
        static ref IPV4_ADDRS_WITH_NET: Vec<IpNet> = vec![
            "198.51.100.1/24".parse().unwrap(),
            "203.0.113.1/24".parse().unwrap(),
        ];
        static ref IPV6_ADDRS_WITH_NET: Vec<IpNet> = vec![
            "2001:db8:1111:1111::1/64".parse().unwrap(),
            "2001:db8:2222:2222::1/64".parse().unwrap(),
        ];
    }

    const TCP_SPEC: ConnSpec = ConnSpec::Tcp { port: 80 };
    const UDP_SPEC: ConnSpec = ConnSpec::Udp { port: 53 };

    async fn test_input<BF, EF>(
        addrs_with_net: &[IpNet],
        spec: ConnSpec,
        build_rule: BF,
        expect_effect: EF,
    ) -> Result<(), io::Error>
    where
        BF: Fn(ConnSpec) -> String,
        EF: Fn(&dyn ConnPath) -> ConnEffect,
    {
        *INIT;

        let mut router = OsHost::new("router".into())?;
        let mut wan = router.new_interface("wan".into(), addrs_with_net[0])?;

        let rules = formatdoc! {
            r#"
                table inet filter {{
                    chain input {{
                        type filter hook input priority filter;
                        {rule}
                        log prefix "Other packet: " counter accept
                    }}
                }}
            "#,
            rule = build_rule(spec)
        };
        router.load_nft_rules(rules.as_bytes())?;

        let path = OsHost::input_path(&mut wan, &router)?;
        let expected_conn_effect = expect_effect(&*path);

        let conn_effect = path.connect(spec).await;

        debug!("Firewall state:\n{}", router.list_nft_rules()?);
        assert_eq!(expected_conn_effect, conn_effect?);

        Ok(())
    }

    async fn test_output<BF, EF>(
        addrs_with_net: &[IpNet],
        spec: ConnSpec,
        build_rule: BF,
        expect_effect: EF,
    ) -> Result<(), io::Error>
    where
        BF: Fn(ConnSpec) -> String,
        EF: Fn(&dyn ConnPath) -> ConnEffect,
    {
        *INIT;

        let mut router = OsHost::new("router".into())?;
        let mut wan = router.new_interface("wan".into(), addrs_with_net[0])?;

        let rules = formatdoc! {
            r#"
                table inet filter {{
                    chain output {{
                        type filter hook output priority filter;
                        {rule}
                        log prefix "Other packet: " counter accept
                    }}
                }}
            "#,
            rule = build_rule(spec)
        };
        router.load_nft_rules(rules.as_bytes())?;

        let path = OsHost::output_path(&router, &mut wan)?;
        let expected_conn_effect = expect_effect(&*path);

        let conn_effect = path.connect(spec).await;

        debug!("Firewall state:\n{}", router.list_nft_rules()?);
        assert_eq!(expected_conn_effect, conn_effect?);

        Ok(())
    }

    fn build_accept(spec: ConnSpec) -> String {
        match spec {
            ConnSpec::Tcp { port } => format!("tcp dport {} counter accept", port),
            ConnSpec::Udp { port } => format!("udp dport {} counter accept", port),
        }
    }

    fn build_drop(spec: ConnSpec) -> String {
        match spec {
            ConnSpec::Tcp { port } => format!("tcp dport {} counter drop", port),
            ConnSpec::Udp { port } => format!("udp dport {} counter drop", port),
        }
    }

    fn build_reject(spec: ConnSpec) -> String {
        match spec {
            ConnSpec::Tcp { port } => format!("tcp dport {} counter reject with tcp reset", port),
            ConnSpec::Udp { port } => format!("udp dport {} counter reject", port),
        }
    }

    fn expect_ok(path: &dyn ConnPath) -> ConnEffect {
        ConnEffect::Ok {
            source_addr: path.source_addr(),
        }
    }

    fn expect_unreachable(_path: &dyn ConnPath) -> ConnEffect {
        ConnEffect::Unreachable
    }

    fn expect_refused(_path: &dyn ConnPath) -> ConnEffect {
        ConnEffect::Refused
    }

    macro_rules! gen_test {
        ($direction:ident, $action:ident, $effect:ident, $layer4:ident, $layer3:ident) => {
            paste! {
                #[tokio::test]
                async fn [< test_ $action _firewall _with_ $layer4 _over_ $layer3 _ $direction >]() -> Result<(), io::Error> {
                    [< test_ $direction >](
                        &[< $layer3:snake:upper _ADDRS_WITH_NET >],
                        [< $layer4:snake:upper _SPEC >],
                        [< build_ $action >],
                        [< expect_ $effect >]
                    ).await
                }
            }
        };
    }

    // TODO: test forward as well as input/output

    gen_test!(input, accept, ok, tcp, ipv4);
    gen_test!(input, accept, ok, tcp, ipv6);
    gen_test!(input, accept, ok, udp, ipv4);
    gen_test!(input, accept, ok, udp, ipv6);
    gen_test!(input, drop, unreachable, tcp, ipv4);
    gen_test!(input, drop, unreachable, tcp, ipv6);
    gen_test!(input, drop, unreachable, udp, ipv4);
    gen_test!(input, drop, unreachable, udp, ipv6);
    gen_test!(input, reject, refused, tcp, ipv4);
    gen_test!(input, reject, refused, tcp, ipv6);
    gen_test!(input, reject, refused, udp, ipv4);
    gen_test!(input, reject, refused, udp, ipv6);

    // Note that on Linux, output drop has different effects for TCP and UDP
    gen_test!(output, accept, ok, tcp, ipv4);
    gen_test!(output, accept, ok, tcp, ipv6);
    gen_test!(output, accept, ok, udp, ipv4);
    gen_test!(output, accept, ok, udp, ipv6);
    gen_test!(output, drop, unreachable, tcp, ipv4);
    gen_test!(output, drop, unreachable, tcp, ipv6);
    gen_test!(output, drop, refused, udp, ipv4);
    gen_test!(output, drop, refused, udp, ipv6);
    gen_test!(output, reject, refused, tcp, ipv4);
    gen_test!(output, reject, refused, tcp, ipv6);
    gen_test!(output, reject, refused, udp, ipv4);
    gen_test!(output, reject, refused, udp, ipv6);
}
