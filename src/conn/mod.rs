use std::prelude::v1::*;

use async_trait::async_trait;
use std::io;
use std::net::IpAddr;

pub mod os;

#[async_trait]
pub trait ConnPath: Sync {
    fn source_name(&self) -> &str;
    fn source_addr(&self) -> IpAddr;
    fn target_name(&self) -> &str;
    fn target_addr(&self) -> IpAddr;

    async fn connect(&self, spec: ConnSpec) -> Result<ConnEffect, io::Error>;
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ConnSpec {
    Tcp { port: u16 },
    Udp { port: u16 },
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum ConnEffect {
    Ok { source_addr: IpAddr },
    Refused,
    Unreachable,
}
