use std::prelude::v1::*;

use ipnet::{IpNet, Ipv4Net, Ipv6Net};
use rand::random;
use std::io;
use std::net::IpAddr;

use crate::conn::ConnPath;

pub mod os;

pub trait Host: Sized {
    type Interface: Interface;

    fn new(name: String) -> Result<Self, io::Error>;

    fn name(&self) -> &str;
    fn new_interface(
        &mut self,
        name: String,
        addr_with_net: IpNet,
    ) -> Result<Self::Interface, io::Error>;

    fn load_nft_rules<R: io::Read + Send>(&mut self, rules: R) -> Result<(), io::Error>;
    fn list_nft_rules(&self) -> Result<String, io::Error>;

    fn input_path<'a>(
        interface: &'a mut Self::Interface,
        host: &'a Self,
    ) -> Result<Box<dyn ConnPath + 'a>, io::Error>;
    fn output_path<'a>(
        host: &'a Self,
        interface: &'a mut Self::Interface,
    ) -> Result<Box<dyn ConnPath + 'a>, io::Error>;
    fn forward_path<'a>(
        source_interface: &'a mut Self::Interface,
        target_interface: &'a mut Self::Interface,
    ) -> Result<Box<dyn ConnPath + 'a>, io::Error>;
}

pub trait Interface: Sized {
    fn name(&self) -> &str;
    fn addr_with_net(&self) -> IpNet;
    fn addr(&self) -> IpAddr {
        self.addr_with_net().addr()
    }
}

fn random_peer_addr_with_net(addr_with_net: IpNet) -> IpNet {
    loop {
        let ret = match addr_with_net {
            IpNet::V4(addr_with_net) => {
                let peer_addr = u32::from(addr_with_net.network())
                    | (random::<u32>() & u32::from(addr_with_net.hostmask()));
                IpNet::V4(
                    Ipv4Net::new(peer_addr.into(), addr_with_net.prefix_len())
                        .expect("Prefix len is known to be valid"),
                )
            }
            IpNet::V6(addr_with_net) => {
                let peer_addr = u128::from(addr_with_net.network())
                    | (random::<u128>() & u128::from(addr_with_net.hostmask()));
                IpNet::V6(
                    Ipv6Net::new(peer_addr.into(), addr_with_net.prefix_len())
                        .expect("Prefix len is known to be valid"),
                )
            }
        };
        assert_eq!(ret.network(), addr_with_net.network());
        if ret != addr_with_net {
            return ret;
        }
    }
}
