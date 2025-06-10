use std::{
    collections::{BinaryHeap, hash_map},
    net::SocketAddr,
};

use tokio::{sync::mpsc, task::JoinSet};

use crate::{download::PEER_CONNECT_TIMEOUT, peer_listener::NewPeer, peers::Peer};

#[derive(Debug, Clone, Copy)]
struct StoredPeer {
    ip: SocketAddr,
    priority: u32,
}

impl StoredPeer {
    pub fn new(ip: SocketAddr, my_ip: SocketAddr) -> Self {
        let priority = crate::protocol::peer::canonical_peer_priority(ip, my_ip);
        Self { ip, priority }
    }
    pub fn new_with_base_priority(ip: SocketAddr) -> Self {
        Self { ip, priority: 100 }
    }
}

impl PartialEq for StoredPeer {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}

impl Eq for StoredPeer {}

impl Ord for StoredPeer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority.cmp(&other.priority)
    }
}

impl PartialOrd for StoredPeer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Default)]
/// Spawns tasks to connect peers ensuring no duplicate concurrent connects
struct PeerConnector {
    join_set: JoinSet<Result<Peer, SocketAddr>>,
}

impl PeerConnector {
    pub fn connect(&mut self, ip: SocketAddr, info_hash: [u8; 20]) {
        self.join_set.spawn(async move {
            match tokio::time::timeout(PEER_CONNECT_TIMEOUT, Peer::new_from_ip(ip, info_hash)).await
            {
                Ok(Ok(peer)) => Ok(peer),
                _ => Err(ip),
            }
        });
    }
}

#[derive(Debug)]
enum PeerStatus {
    Active,
    Banned,
    Stored,
    Connecting,
}

/// Holds peers that didn't fit in connection slots
#[derive(Debug)]
pub struct PeerStorage {
    my_ip: Option<SocketAddr>,
    peer_statuses: hash_map::HashMap<SocketAddr, PeerStatus>,
    best_peers: BinaryHeap<StoredPeer>,
    peer_connector: PeerConnector,
}

impl PeerStorage {
    const MAX_SIZE: usize = 1_000;

    pub fn new(ban_list: Vec<SocketAddr>, my_ip: Option<SocketAddr>) -> Self {
        let peer_statuses =
            hash_map::HashMap::from_iter(ban_list.into_iter().map(|ip| (ip, PeerStatus::Banned)));
        let peer_connector = PeerConnector::default();
        Self {
            peer_connector,
            my_ip,
            peer_statuses,
            best_peers: BinaryHeap::new(),
        }
    }

    /// Returns whether inserted peer is new
    pub fn add_validate(&mut self, peer: Peer, total_pieces: usize) -> bool {
        if self.len() >= Self::MAX_SIZE {
            tracing::warn!(
                "Can't save peer for later. Peer storage is full {}/{}",
                self.len(),
                Self::MAX_SIZE
            );
            return false;
        }
        let ip = peer.ip();
        if self.my_ip().is_none() {
            if let Some(my_ip) = peer.extension_handshake.as_ref().and_then(|e| e.your_ip()) {
                tracing::info!(%my_ip, peer = %peer.ip(), "Resolving my_ip from peer");
                // TODO: use tcp listener port
                self.set_my_ip(Some(SocketAddr::new(my_ip, 0)));
            };
        }

        if let Err(e) = peer.bitfield.validate(total_pieces) {
            tracing::warn!("Failed to validate peer's bitfield: {e}");
            return false;
        }

        self.add(ip)
    }
    /// Returns whether inserted peer is new
    pub fn add(&mut self, ip: SocketAddr) -> bool {
        if self.len() >= Self::MAX_SIZE {
            tracing::warn!(
                "Can't save peer for later. Peer storage is full {}/{}",
                self.len(),
                Self::MAX_SIZE
            );
            return false;
        }
        match self.peer_statuses.entry(ip) {
            hash_map::Entry::Occupied(_) => false,
            hash_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(PeerStatus::Stored);
                match self.my_ip {
                    Some(my_ip) => self.best_peers.push(StoredPeer::new(ip, my_ip)),
                    None => self.best_peers.push(StoredPeer::new_with_base_priority(ip)),
                };
                true
            }
        }
    }

    pub fn connect_best(&mut self, info_hash: &[u8; 20]) -> Option<SocketAddr> {
        let best = self.best_peers.pop()?;
        let peer_status = self
            .peer_statuses
            .get_mut(&best.ip)
            .expect("all peers are tracked");
        *peer_status = PeerStatus::Connecting;
        self.peer_connector.connect(best.ip, *info_hash);
        Some(best.ip)
    }

    /// Join peer that dropped connection
    pub fn join_disconnected_peer(&mut self, ip: SocketAddr) {
        match self.peer_statuses.entry(ip) {
            hash_map::Entry::Occupied(entry) => match entry.get() {
                PeerStatus::Active => {
                    entry.remove();
                }
                PeerStatus::Banned => {
                    tracing::trace!("Keeping banned peer in storage")
                }
                PeerStatus::Stored => {
                    tracing::error!("Joining stored peer");
                    panic!("Invariant detected: Joining stored peer");
                }
                PeerStatus::Connecting => {
                    tracing::error!("Joining connecting peer");
                    panic!("Invariant detected: Joining connecting peer");
                }
            },
            hash_map::Entry::Vacant(_) => {
                tracing::error!("Joined peer is not tracked");
            }
        }
    }

    /// Returns active peer that must be inserted in scheduler
    ///
    /// Decision to connect to this peer is made before it was connected
    /// therefore there is no need to check if this connection is allowed.
    pub fn join_connected_peer(&mut self, total_pieces: usize) -> Option<Peer> {
        while let Some(Ok(joined_peer)) = self.peer_connector.join_set.try_join_next() {
            match joined_peer {
                Ok(peer) => {
                    let ip = peer.ip();

                    if self.my_ip().is_none() {
                        if let Some(my_ip) =
                            peer.extension_handshake.as_ref().and_then(|e| e.your_ip())
                        {
                            tracing::info!(%my_ip, peer = %peer.ip(), "Resolving my_ip from peer");
                            // TODO: use tcp listener port
                            self.set_my_ip(Some(SocketAddr::new(my_ip, 0)));
                        };
                    }

                    if let Err(e) = peer.bitfield.validate(total_pieces) {
                        tracing::warn!("Failed to validate peer's bitfield: {e}");
                        return None;
                    }

                    let mut entry = match self.peer_statuses.entry(ip) {
                        hash_map::Entry::Occupied(entry) => entry,
                        hash_map::Entry::Vacant(_) => {
                            panic!("Invariant encountered: Connected peer is not tracked")
                        }
                    };
                    match entry.get() {
                        PeerStatus::Banned => {
                            tracing::error!("Tried to connect banned peer");
                            return None;
                        }
                        PeerStatus::Active | PeerStatus::Stored | PeerStatus::Connecting => {
                            entry.insert(PeerStatus::Active);
                            return Some(peer);
                        }
                    }
                }
                Err(ip) => {
                    let entry = match self.peer_statuses.entry(ip) {
                        hash_map::Entry::Occupied(entry) => entry,
                        hash_map::Entry::Vacant(_) => {
                            panic!("Invariant encountered: Connected peer is not tracked")
                        }
                    };
                    match entry.get() {
                        PeerStatus::Banned => {
                            tracing::error!("Tried to connect banned peer");
                        }
                        PeerStatus::Active | PeerStatus::Stored | PeerStatus::Connecting => {
                            entry.remove();
                        }
                    }
                }
            };
        }
        None
    }

    pub fn accept_new_peer(&mut self, peer: &Peer, total_pieces: usize) -> Option<SocketAddr> {
        let ip = peer.ip();
        if self.my_ip().is_none() {
            if let Some(my_ip) = peer.extension_handshake.as_ref().and_then(|e| e.your_ip()) {
                tracing::info!(%my_ip, peer = %peer.ip(), "Resolving my_ip from peer");
                // TODO: use tcp listener port
                self.set_my_ip(Some(SocketAddr::new(my_ip, 0)));
            };
        }

        if let Err(e) = peer.bitfield.validate(total_pieces) {
            tracing::warn!("Failed to validate peer's bitfield: {e}");
            return None;
        }

        let mut entry = match self.peer_statuses.entry(ip) {
            hash_map::Entry::Occupied(entry) => entry,
            hash_map::Entry::Vacant(entry) => {
                entry.insert(PeerStatus::Active);
                return Some(ip);
            }
        };
        match entry.get() {
            PeerStatus::Banned => {
                tracing::error!("Tried to connect banned peer");
                return None;
            }
            PeerStatus::Active => {
                tracing::error!("Tried to connect already active peer");
                return None;
            }
            PeerStatus::Stored | PeerStatus::Connecting => {
                entry.insert(PeerStatus::Active);
                return Some(ip);
            }
        }
    }

    pub fn discard_channel_peer(
        &mut self,
        channel: &mut mpsc::Receiver<NewPeer>,
    ) -> Option<SocketAddr> {
        if let Ok(new_peer) = channel.try_recv() {
            match new_peer {
                NewPeer::ListenerOrigin(peer) => {
                    let ip = peer.ip();
                    if self.my_ip().is_none() {
                        if let Some(my_ip) =
                            peer.extension_handshake.as_ref().and_then(|e| e.your_ip())
                        {
                            tracing::info!(%my_ip, peer = %peer.ip(), "Resolving my_ip from peer");
                            // TODO: use tcp listener port
                            self.set_my_ip(Some(SocketAddr::new(my_ip, 0)));
                        };
                    }
                    self.add(ip);
                    return Some(ip);
                }
            }
        }
        None
    }

    pub fn discard_store_connected_peer(&mut self) -> Option<Peer> {
        while let Some(Ok(joined_peer)) = self.peer_connector.join_set.try_join_next() {
            match joined_peer {
                Ok(peer) => {
                    let ip = peer.ip();
                    let status = self
                        .peer_statuses
                        .get_mut(&ip)
                        .expect("all peers are tracked");
                    match self.my_ip {
                        Some(my_ip) => self.best_peers.push(StoredPeer::new(ip, my_ip)),
                        None => self.best_peers.push(StoredPeer::new_with_base_priority(ip)),
                    };
                    *status = PeerStatus::Stored;
                    return Some(peer);
                }
                Err(ip) => self.peer_statuses.remove(&ip),
            };
        }
        None
    }

    pub fn set_my_ip(&mut self, ip: Option<SocketAddr>) {
        self.my_ip = ip;
        if let Some(ip) = ip {
            let mut old_heap = BinaryHeap::with_capacity(self.best_peers.len());
            std::mem::swap(&mut self.best_peers, &mut old_heap);
            for peer in old_heap {
                self.best_peers.push(StoredPeer::new(peer.ip, ip));
            }
        }
    }

    pub fn my_ip(&self) -> Option<SocketAddr> {
        self.my_ip
    }

    pub fn pending_amount(&self) -> usize {
        self.peer_connector.join_set.len()
    }

    pub fn len(&self) -> usize {
        self.best_peers.len()
    }
}
