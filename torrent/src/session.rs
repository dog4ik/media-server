use std::sync::atomic::{AtomicUsize, Ordering};

/// Global store used to limit the amount of connected peers between all torrents downloads
#[derive(Debug, Default)]
pub struct SessionContext {
    peers_amount: AtomicUsize,
    torrents_amount: AtomicUsize,
    /// Maximum of total peer connections the client is allowed to have at the same time
    max_peer_connections: usize,
}

impl SessionContext {
    pub fn new(max_peer_connections: usize) -> Self {
        Self {
            peers_amount: AtomicUsize::new(0),
            torrents_amount: AtomicUsize::new(0),
            max_peer_connections,
        }
    }
}

impl SessionContext {
    pub fn add_peer(&self) {
        self.peers_amount.fetch_add(1, Ordering::AcqRel);
    }

    pub fn remove_peer(&self) {
        self.peers_amount.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn add_torrent(&self) {
        self.torrents_amount.fetch_add(1, Ordering::AcqRel);
    }

    pub fn remove_torrent(&self) {
        self.torrents_amount.fetch_sub(1, Ordering::AcqRel);
    }

    pub fn max_connections_per_torrent(&self) -> usize {
        self.max_peer_connections / self.torrents_amount.load(Ordering::Acquire)
    }
}
