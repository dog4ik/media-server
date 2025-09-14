use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
};

#[derive(Debug, Default)]
pub struct BanList(HashSet<IpAddr>);

impl BanList {
    pub fn new() -> Self {
        Self(HashSet::new())
    }

    pub fn add_ip(&mut self, addr: SocketAddr) -> bool {
        self.0.insert(addr.ip())
    }

    pub fn has(&self, addr: SocketAddr) -> bool {
        self.0.contains(&addr.ip())
    }
}
