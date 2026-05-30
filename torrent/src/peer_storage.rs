use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use tokio::{sync::mpsc, task::JoinSet};

use crate::{download::PEER_CONNECT_TIMEOUT, peer_listener::NewPeer, peers::Peer};

const MAX_PEERLIST_SIZE: usize = 1_000;
const MAX_FAILCOUNT: u16 = 10;
const MIN_RECONNECT_SECS: u64 = 60;
const CANDIDATE_SCAN_SIZE: usize = 300;
const CANDIDATE_CACHE_SIZE: usize = 10;

fn addr_ord(a: &SocketAddr, b: &SocketAddr) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (SocketAddr::V4(a), SocketAddr::V4(b)) => a
            .ip()
            .octets()
            .cmp(&b.ip().octets())
            .then(a.port().cmp(&b.port())),
        (SocketAddr::V6(a), SocketAddr::V6(b)) => a
            .ip()
            .octets()
            .cmp(&b.ip().octets())
            .then(a.port().cmp(&b.port())),
        (SocketAddr::V4(_), SocketAddr::V6(_)) => Ordering::Less,
        (SocketAddr::V6(_), SocketAddr::V4(_)) => Ordering::Greater,
    }
}

#[derive(Debug, Eq, PartialEq)]
struct ConnectCandiatate {
    failcount: u16,
    last_connected: Option<Instant>,
    bep40_priority: u32,
    ip: SocketAddr,
}

impl Ord for ConnectCandiatate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.failcount
            .cmp(&other.failcount)
            .then(self.last_connected.cmp(&other.last_connected))
            .then(other.bep40_priority.cmp(&self.bep40_priority))
    }
}

impl PartialOrd for ConnectCandiatate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeerStatus {
    Stored,
    Connecting,
    Active,
    Banned,
}

#[derive(Debug)]
struct TorrentPeer {
    ip: SocketAddr,
    failcount: u16,
    last_connected: Option<Instant>,
    status: PeerStatus,
}

impl TorrentPeer {
    fn new(ip: SocketAddr) -> Self {
        Self {
            ip,
            failcount: 0,
            last_connected: None,
            status: PeerStatus::Stored,
        }
    }

    fn banned(ip: SocketAddr) -> Self {
        Self {
            ip,
            failcount: 0,
            last_connected: None,
            status: PeerStatus::Banned,
        }
    }

    fn can_connect(&self) -> bool {
        self.status == PeerStatus::Stored
            && self.failcount < MAX_FAILCOUNT
            && self.last_connected.map_or(true, |t| {
                t.elapsed() >= Duration::from_secs((self.failcount + 1) as u64 * MIN_RECONNECT_SECS)
            })
    }
}

#[derive(Debug, Default)]
struct PeerConnector {
    join_set: JoinSet<Result<Peer, SocketAddr>>,
}

impl PeerConnector {
    fn connect(&mut self, ip: SocketAddr, info_hash: [u8; 20]) {
        self.join_set.spawn(async move {
            match tokio::time::timeout(PEER_CONNECT_TIMEOUT, Peer::new_from_ip(ip, info_hash)).await
            {
                Ok(Ok(peer)) => Ok(peer),
                _ => Err(ip),
            }
        });
    }
}

/// Holds all known peers for the torrent
#[derive(Debug, Default)]
pub struct PeerStorage {
    my_ip: Option<SocketAddr>,
    /// Peer list sorted by ip address
    peers: Vec<TorrentPeer>,
    /// Cached list of the best peers
    candidate_cache: Vec<SocketAddr>,
    /// Index into [Self::peers]. Start position of the best [peers pick window](CANDIDATE_SCAN_SIZE)
    round_robin: usize,
    peer_connector: PeerConnector,
}

impl PeerStorage {
    pub fn new(ban_list: Vec<SocketAddr>, my_ip: Option<SocketAddr>) -> Self {
        let mut peers: Vec<TorrentPeer> = ban_list.into_iter().map(TorrentPeer::banned).collect();
        peers.sort_by(|a, b| addr_ord(&a.ip, &b.ip));
        Self {
            my_ip,
            peers,
            candidate_cache: Vec::new(),
            round_robin: 0,
            peer_connector: PeerConnector::default(),
        }
    }

    fn search(&self, ip: SocketAddr) -> Result<usize, usize> {
        self.peers.binary_search_by(|p| addr_ord(&p.ip, &ip))
    }

    fn peer_at_mut(&mut self, ip: SocketAddr) -> Option<&mut TorrentPeer> {
        self.search(ip).ok().map(|i| &mut self.peers[i])
    }

    fn try_update_my_ip(&mut self, peer: &Peer) {
        if self.my_ip.is_none() {
            if let Some(my_ip) = peer.extension_handshake.as_ref().and_then(|e| e.your_ip()) {
                tracing::info!(%my_ip, peer = %peer.ip(), "Resolving my_ip from peer");
                self.my_ip = Some(SocketAddr::new(my_ip, 0));
                self.candidate_cache.clear();
            }
        }
    }

    /// Returns whether the peer was newly inserted.
    pub fn add(&mut self, ip: SocketAddr) -> bool {
        if self.peers.len() >= MAX_PEERLIST_SIZE {
            tracing::warn!(
                "Peer list full ({}/{}), dropping {ip}",
                self.peers.len(),
                MAX_PEERLIST_SIZE
            );
            return false;
        }
        match self.search(ip) {
            Ok(_) => false,
            Err(idx) => {
                self.peers.insert(idx, TorrentPeer::new(ip));
                true
            }
        }
    }

    /// Returns whether the peer was newly inserted.
    pub fn add_validate(&mut self, peer: Peer, total_pieces: usize) -> bool {
        let ip = peer.ip();
        self.try_update_my_ip(&peer);
        if let Err(e) = peer.bitfield.validate(total_pieces) {
            tracing::warn!("Failed to validate peer's bitfield: {e}");
            return false;
        }
        self.add(ip)
    }

    fn find_connect_candidates(&mut self) {
        let len = self.peers.len();
        if len == 0 {
            return;
        }
        let scan = len.min(CANDIDATE_SCAN_SIZE);
        let start = self.round_robin % len;
        let my_ip = self.my_ip;

        let mut candidates = Vec::new();

        for i in 0..scan {
            let peer = &self.peers[(start + i) % len];
            if !peer.can_connect() {
                continue;
            }
            let priority = my_ip.map_or(100, |my| {
                crate::protocol::peer::canonical_peer_priority(peer.ip, my)
            });
            candidates.push(ConnectCandiatate {
                failcount: peer.failcount,
                last_connected: peer.last_connected,
                bep40_priority: priority,
                ip: peer.ip,
            });
        }

        self.round_robin = (start + scan) % len.max(1);

        candidates.sort();

        // Reversed so pop_back() yields the best candidate.
        self.candidate_cache = candidates
            .into_iter()
            .rev()
            .map(|candidate| candidate.ip)
            .take(CANDIDATE_CACHE_SIZE)
            .collect();
    }

    /// Initiate a connection to the highest-ranked stored peer.
    pub fn connect_best(&mut self, info_hash: &[u8; 20]) -> Option<SocketAddr> {
        {
            let mut cache = std::mem::take(&mut self.candidate_cache);
            let peers = &self.peers;
            cache.retain(|ip| {
                peers
                    .binary_search_by(|p| addr_ord(&p.ip, ip))
                    .ok()
                    .map(|i| peers[i].can_connect())
                    .unwrap_or(false)
            });
            self.candidate_cache = cache;
        }

        if self.candidate_cache.is_empty() {
            self.find_connect_candidates();
        }

        let ip = self.candidate_cache.pop()?;
        let peer = self.peer_at_mut(ip)?;
        peer.status = PeerStatus::Connecting;
        self.peer_connector.connect(ip, *info_hash);
        Some(ip)
    }

    pub fn join_connected_peer(&mut self, total_pieces: usize) -> Option<Peer> {
        while let Some(Ok(result)) = self.peer_connector.join_set.try_join_next() {
            match result {
                Ok(peer) => {
                    self.try_update_my_ip(&peer);
                    let ip = peer.ip();
                    if let Err(e) = peer.bitfield.validate(total_pieces) {
                        tracing::warn!("Failed to validate peer's bitfield: {e}");
                        if let Some(p) = self.peer_at_mut(ip) {
                            p.failcount += 1;
                            p.status = PeerStatus::Stored;
                        }
                        continue;
                    }
                    if let Some(p) = self.peer_at_mut(ip) {
                        p.status = PeerStatus::Active;
                        p.failcount = 0;
                    }
                    return Some(peer);
                }
                Err(ip) => {
                    if let Some(p) = self.peer_at_mut(ip) {
                        p.failcount += 1;
                        p.status = PeerStatus::Stored;
                    }
                }
            }
        }
        None
    }

    /// Park a disconnected active peer as stored for potential reconnection.
    pub fn join_disconnected_peer(&mut self, ip: SocketAddr) {
        match self.peer_at_mut(ip) {
            Some(peer) => match peer.status {
                PeerStatus::Active => {
                    peer.status = PeerStatus::Stored;
                    peer.last_connected = Some(Instant::now());
                }
                PeerStatus::Banned => {
                    tracing::trace!("Keeping banned peer in storage");
                }
                PeerStatus::Stored => {
                    tracing::error!("Joining stored peer, invariant violated");
                }
                PeerStatus::Connecting => {
                    tracing::error!("Joining connecting peer, invariant violated");
                }
            },
            None => {
                tracing::error!("Joined peer is not tracked");
            }
        }
    }

    /// Accept an inbound peer connection. Returns the peer's address on success.
    pub fn accept_new_peer(&mut self, peer: &Peer, total_pieces: usize) -> Option<SocketAddr> {
        let ip = peer.ip();
        self.try_update_my_ip(peer);
        if let Err(e) = peer.bitfield.validate(total_pieces) {
            tracing::warn!("Failed to validate peer's bitfield: {e}");
            return None;
        }
        match self.search(ip) {
            Ok(idx) => match self.peers[idx].status {
                PeerStatus::Banned => {
                    tracing::error!("Rejected banned inbound peer");
                    None
                }
                PeerStatus::Active => {
                    tracing::error!("Rejected duplicate inbound peer");
                    None
                }
                PeerStatus::Stored | PeerStatus::Connecting => {
                    self.peers[idx].status = PeerStatus::Active;
                    self.peers[idx].failcount = 0;
                    Some(ip)
                }
            },
            Err(insert_idx) => {
                if self.peers.len() >= MAX_PEERLIST_SIZE {
                    tracing::warn!("Peer list full, dropping inbound peer {ip}");
                    return None;
                }
                self.peers.insert(
                    insert_idx,
                    TorrentPeer {
                        ip,
                        failcount: 0,
                        last_connected: None,
                        status: PeerStatus::Active,
                    },
                );
                Some(ip)
            }
        }
    }

    /// Discard an inbound peer that arrived while paused; store its address for later.
    pub fn discard_channel_peer(
        &mut self,
        channel: &mut mpsc::Receiver<NewPeer>,
    ) -> Option<SocketAddr> {
        if let Ok(new_peer) = channel.try_recv() {
            match new_peer {
                NewPeer::ListenerOrigin(peer) => {
                    let ip = peer.ip();
                    self.try_update_my_ip(&peer);
                    self.add(ip);
                    return Some(ip);
                }
            }
        }
        None
    }

    /// Drain in-flight connections while paused, storing successful ones for
    /// reconnection after resume.
    pub fn discard_store_connected_peer(&mut self) -> Option<Peer> {
        while let Some(Ok(result)) = self.peer_connector.join_set.try_join_next() {
            match result {
                Ok(peer) => {
                    let ip = peer.ip();
                    if let Some(p) = self.peer_at_mut(ip) {
                        p.status = PeerStatus::Stored;
                        p.failcount = 0;
                    }
                    return Some(peer);
                }
                Err(ip) => {
                    if let Some(p) = self.peer_at_mut(ip) {
                        p.failcount += 1;
                        p.status = PeerStatus::Stored;
                    }
                }
            }
        }
        None
    }

    pub fn pending_amount(&self) -> usize {
        self.peer_connector.join_set.len()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, SocketAddr, SocketAddrV4},
        time::{Duration, Instant},
    };

    use crate::peer_storage::{
        CANDIDATE_SCAN_SIZE, ConnectCandiatate, MAX_FAILCOUNT, MAX_PEERLIST_SIZE,
        MIN_RECONNECT_SECS, PeerStatus, PeerStorage, TorrentPeer, addr_ord,
    };

    const INFO_HASH: [u8; 20] = [0u8; 20];

    fn ip(a: u8, b: u8, c: u8, d: u8) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(a, b, c, d), 6881))
    }

    impl PeerStorage {
        fn get(&self, addr: SocketAddr) -> Option<&TorrentPeer> {
            self.search(addr).ok().map(|i| &self.peers[i])
        }
    }

    #[test]
    fn add_new_peer_returns_true() {
        let mut s = PeerStorage::default();
        assert!(s.add(ip(10, 0, 0, 1)));
    }

    #[test]
    fn add_duplicate_returns_false_and_does_not_grow() {
        let mut s = PeerStorage::default();
        let a = ip(10, 0, 0, 1);
        assert!(s.add(a));
        assert!(!s.add(a));
        assert_eq!(s.peers.len(), 1);
    }

    #[test]
    fn add_maintains_sorted_order() {
        let mut s = PeerStorage::default();
        for addr in [ip(10, 0, 0, 3), ip(10, 0, 0, 1), ip(10, 0, 0, 2)] {
            s.add(addr);
        }
        for w in s.peers.windows(2) {
            assert!(addr_ord(&w[0].ip, &w[1].ip).is_lt());
        }
    }

    #[test]
    fn add_returns_false_when_at_capacity() {
        let mut s = PeerStorage::default();
        for i in 0..MAX_PEERLIST_SIZE as u32 {
            s.add(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(
                    ((i >> 16) & 0xff) as u8,
                    ((i >> 8) & 0xff) as u8,
                    (i & 0xff) as u8,
                    1,
                ),
                6881,
            )));
        }
        assert!(!s.add(ip(255, 255, 255, 1)));
        assert_eq!(s.peers.len(), MAX_PEERLIST_SIZE);
    }

    // ban list

    #[test]
    fn banned_peer_cannot_connect() {
        let a = ip(10, 0, 0, 1);
        let s = PeerStorage::new(vec![a], None);
        assert!(!s.get(a).unwrap().can_connect());
    }

    #[test]
    fn add_does_not_overwrite_banned_entry() {
        let a = ip(10, 0, 0, 1);
        let mut s = PeerStorage::new(vec![a], None);
        assert!(!s.add(a)); // already present → false
        assert!(!s.get(a).unwrap().can_connect()); // still banned
    }

    // can_connect

    #[test]
    fn can_connect_fresh_peer() {
        assert!(TorrentPeer::new(ip(10, 0, 0, 1)).can_connect());
    }

    #[test]
    fn can_connect_false_at_max_failcount() {
        let mut p = TorrentPeer::new(ip(10, 0, 0, 1));
        p.failcount = MAX_FAILCOUNT;
        assert!(!p.can_connect());
    }

    #[test]
    fn can_connect_false_within_backoff_window() {
        let mut p = TorrentPeer::new(ip(10, 0, 0, 1));
        p.last_connected = Some(Instant::now());
        assert!(!p.can_connect()); // backoff = (0+1)*60s
    }

    #[test]
    fn can_connect_true_after_backoff_expires() {
        let mut p = TorrentPeer::new(ip(10, 0, 0, 1));
        p.last_connected = Some(Instant::now() - Duration::from_secs(MIN_RECONNECT_SECS + 1));
        assert!(p.can_connect());
    }

    #[test]
    fn backoff_scales_with_failcount() {
        let mut p = TorrentPeer::new(ip(10, 0, 0, 1));
        p.failcount = 2;
        // backoff = (2+1)*60 = 180s; 120s is not enough
        p.last_connected = Some(Instant::now() - Duration::from_secs(120));
        assert!(!p.can_connect());
        // 181s is enough
        p.last_connected = Some(Instant::now() - Duration::from_secs(181));
        assert!(p.can_connect());
    }

    // connect_best

    #[tokio::test]
    async fn connect_best_returns_ip_and_spawns_task() {
        let mut s = PeerStorage::default();
        let a = ip(10, 0, 0, 1);
        s.add(a);
        assert_eq!(s.connect_best(&INFO_HASH), Some(a));
        assert_eq!(s.pending_amount(), 1);
        // peer is now Connecting - not a candidate anymore
        assert!(!s.get(a).unwrap().can_connect());
    }

    #[tokio::test]
    async fn connect_best_empty_returns_none() {
        let mut s = PeerStorage::default();
        assert!(s.connect_best(&INFO_HASH).is_none());
        assert_eq!(s.pending_amount(), 0);
    }

    #[tokio::test]
    async fn connect_best_skips_banned_peers() {
        let a = ip(10, 0, 0, 1);
        let mut s = PeerStorage::new(vec![a], None);
        assert!(s.connect_best(&INFO_HASH).is_none());
        assert_eq!(s.pending_amount(), 0);
    }

    #[tokio::test]
    async fn connect_best_skips_peer_within_backoff() {
        let mut s = PeerStorage::default();
        let a = ip(10, 0, 0, 1);
        s.add(a);
        s.peer_at_mut(a).unwrap().last_connected = Some(Instant::now());
        assert!(s.connect_best(&INFO_HASH).is_none());
    }

    #[tokio::test]
    async fn connect_best_prefers_lower_failcount() {
        let mut s = PeerStorage::default();
        let good = ip(10, 0, 0, 1); // failcount 0
        let worse = ip(10, 0, 0, 2); // failcount 3
        s.add(good);
        s.add(worse);
        {
            let p = s.peer_at_mut(worse).unwrap();
            p.failcount = 3;
            p.last_connected = Some(Instant::now() - Duration::from_secs(4 * MIN_RECONNECT_SECS));
        }
        assert_eq!(s.connect_best(&INFO_HASH), Some(good));
    }

    #[tokio::test]
    async fn connect_best_prefers_never_connected_over_old_connection() {
        let mut s = PeerStorage::default();
        let never = ip(10, 0, 0, 1);
        let old = ip(10, 0, 0, 2);
        s.add(never);
        s.add(old);
        // old peer connected before, but past backoff - still lower rank than never-connected
        s.peer_at_mut(old).unwrap().last_connected =
            Some(Instant::now() - Duration::from_secs(MIN_RECONNECT_SECS + 1));
        assert_eq!(s.connect_best(&INFO_HASH), Some(never));
    }

    // join_disconnected_peer

    #[test]
    fn disconnected_active_peer_is_parked_as_stored() {
        let mut s = PeerStorage::default();
        let a = ip(10, 0, 0, 1);
        s.add(a);
        s.peer_at_mut(a).unwrap().status = PeerStatus::Active;

        s.join_disconnected_peer(a);

        let p = s.get(a).unwrap();
        assert_eq!(p.status, PeerStatus::Stored);
        assert!(p.last_connected.is_some());
    }

    #[test]
    fn disconnected_peer_is_in_backoff_immediately_after_disconnect() {
        let mut s = PeerStorage::default();
        let a = ip(10, 0, 0, 1);
        s.add(a);
        s.peer_at_mut(a).unwrap().status = PeerStatus::Active;
        s.join_disconnected_peer(a);

        assert!(!s.get(a).unwrap().can_connect());
    }

    #[tokio::test]
    async fn reconnect_succeeds_after_backoff_expires() {
        let mut s = PeerStorage::default();
        let a = ip(10, 0, 0, 1);
        s.add(a);
        s.peer_at_mut(a).unwrap().status = PeerStatus::Active;
        s.join_disconnected_peer(a);

        // Fast-forward past the backoff window
        s.peer_at_mut(a).unwrap().last_connected =
            Some(Instant::now() - Duration::from_secs(MIN_RECONNECT_SECS + 1));

        assert_eq!(s.connect_best(&INFO_HASH), Some(a));
    }

    // round-robin

    #[test]
    fn round_robin_cycles_through_peers() {
        let mut s = PeerStorage::default();
        // Use more peers than one scan covers so the index doesn't wrap to 0.
        let n = CANDIDATE_SCAN_SIZE + 5;
        for i in 0..n {
            let a = i as u8;
            let b = (i >> 8) as u8;
            s.add(SocketAddr::V4(SocketAddrV4::new(
                Ipv4Addr::new(10, b, a, 1),
                6881,
            )));
        }
        let len = s.peers.len();
        s.find_connect_candidates();
        assert_eq!(s.round_robin, CANDIDATE_SCAN_SIZE % len);
        s.find_connect_candidates();
        assert_eq!(s.round_robin, (CANDIDATE_SCAN_SIZE * 2) % len);
    }

    #[test]
    fn round_robin_wraps_around() {
        let mut s = PeerStorage::default();
        s.add(ip(10, 0, 0, 1));
        // Drive it past the list length to force a wrap
        s.round_robin = 11;
        s.find_connect_candidates();
    }

    // ConnectCandiatate ordering

    fn candidate(
        failcount: u16,
        last_connected: Option<Instant>,
        bep40_priority: u32,
    ) -> ConnectCandiatate {
        ConnectCandiatate {
            failcount,
            last_connected,
            bep40_priority,
            ip: ip(10, 0, 0, 1),
        }
    }

    #[test]
    fn candidate_lower_failcount_is_less() {
        let good = candidate(0, None, 0);
        let bad = candidate(3, None, 0);
        assert!(good < bad);
    }

    #[test]
    fn candidate_older_last_connected_is_less() {
        let old = candidate(0, Some(Instant::now() - Duration::from_secs(200)), 0);
        let recent = candidate(0, Some(Instant::now()), 0);
        assert!(old < recent);
    }

    #[test]
    fn candidate_never_connected_is_less_than_old_connection() {
        let never = candidate(0, None, 0);
        let old = candidate(0, Some(Instant::now() - Duration::from_secs(1000)), 0);
        assert!(never < old);
    }

    #[test]
    fn candidate_higher_bep40_priority_is_less() {
        let high_prio = candidate(0, None, 100);
        let low_prio = candidate(0, None, 10);
        assert!(high_prio < low_prio);
    }

    #[test]
    fn candidate_failcount_dominates_last_connected() {
        // Lower failcount wins even with a less favourable last_connected.
        let low_fc = candidate(1, Some(Instant::now()), 0);
        let high_fc = candidate(3, None, 0);
        assert!(low_fc < high_fc);
    }

    #[test]
    fn candidate_last_connected_dominates_bep40() {
        // Older last_connected wins over higher bep40 priority.
        let old = candidate(0, Some(Instant::now() - Duration::from_secs(200)), 0);
        let recent_high = candidate(0, Some(Instant::now()), 100);
        assert!(old < recent_high);
    }

    #[test]
    fn candidate_equal_is_not_less() {
        let a = candidate(1, None, 50);
        let b = candidate(1, None, 50);
        assert!(!(a < b) && !(b < a));
    }
}
