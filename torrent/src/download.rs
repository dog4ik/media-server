use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
    net::SocketAddr,
    ops::Range,
    path::Path,
    time::Duration,
};

use anyhow::{anyhow, bail, ensure};
use bytes::{Bytes, BytesMut};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::{timeout, Instant},
};
use uuid::Uuid;

use crate::{
    peers::{BitField, Peer, PeerError, PeerIPC},
    protocol::Info,
    scheduler::Scheduler,
    NewPeer, Torrent,
};

/// Piece representation where all blocks are sorted
#[derive(Debug, Clone)]
pub struct Piece {
    pub index: u32,
    pub length: u32,
    pub blocks: Vec<(u32, Bytes)>,
}

impl Piece {
    pub fn empty(index: u32, length: u32) -> Self {
        Self {
            index,
            length,
            blocks: Vec::new(),
        }
    }

    pub fn is_full(&self) -> bool {
        self.blocks
            .iter()
            .map(|(_, bytes)| bytes.len() as u32)
            .sum::<u32>()
            == self.length
    }

    pub fn add_block(&mut self, offset: u32, bytes: Bytes) -> anyhow::Result<()> {
        let end_new_block = offset + bytes.len() as u32;
        if end_new_block > self.length {
            bail!("block is bigger then length of the piece")
        }
        if self.blocks.len() == 0 {
            self.blocks.push((offset, bytes));
            return Ok(());
        }
        for (existing_offset, existing_bytes) in &self.blocks {
            let end_existing_block = existing_offset + existing_bytes.len() as u32;

            if offset < end_existing_block && end_new_block > *existing_offset {
                bail!("block conflicts with existing blocks");
            }
        }

        self.blocks
            .iter()
            .position(|(existing_offset, _)| *existing_offset <= offset)
            .map(|index| {
                self.blocks.insert(index, (offset, bytes));
            })
            .ok_or(anyhow!("failed to insert block"))
    }

    pub fn as_bytes(mut self) -> anyhow::Result<Bytes> {
        let length = self.blocks.iter().map(|(_, bytes)| bytes.len()).sum();
        let mut bytes = BytesMut::with_capacity(length);
        self.blocks.sort_by_key(|(offset, _)| *offset);
        for (_, block_bytes) in &self.blocks {
            bytes.extend_from_slice(&block_bytes);
        }
        ensure!(bytes.len() as u32 == self.length);
        Ok(bytes.into())
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceHistory {
    /// Contains data that reperesents how difference between two measurments changed
    history: VecDeque<Performance>,
    snapshot: Performance,
}

impl PerformanceHistory {
    const CAPACITY: usize = 20;

    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(Self::CAPACITY),
            snapshot: Performance::default(),
        }
    }

    pub fn update(&mut self, new: Performance) {
        if self.history.len() == Self::CAPACITY {
            self.history.pop_back();
        }
        let perf = Performance::new(
            new.downloaded - self.snapshot.downloaded,
            new.uploaded - self.snapshot.uploaded,
        );
        self.snapshot = new;
        self.history.push_front(perf);
    }

    /// Lastest measurement
    pub fn before_last(&self) -> Option<&Performance> {
        if self.history.len() > 2 {
            self.history.get(self.history.len() - 2)
        } else {
            None
        }
    }

    /// Lastest measurement
    pub fn last(&self) -> Option<&Performance> {
        self.history.front()
    }

    pub fn reset(&mut self) {
        *self = Self::new()
    }

    pub fn avg_speed(&self) -> u64 {
        let mut avg = 0;
        for measure in &self.history {
            avg += measure.speed();
        }
        if self.history.is_empty() {
            return 0;
        }
        avg / self.history.len() as u64
    }
}

#[derive(Debug, Clone)]
pub struct ActivePeer {
    pub ip: SocketAddr,
    pub command: mpsc::Sender<PeerCommand>,
    pub bitfield: BitField,
    /// Our status towards peer
    pub out_status: Status,
    /// Peer's status towards us
    pub in_status: Status,
    /// Pending blocks are used if peer panics or chokes, also it indicates that peer is busy
    pub pending_blocks: Vec<Block>,
    /// Amount of bytes downloaded from peer
    pub downloaded: u64,
    /// Amount of bytes uploaded to peer
    pub uploaded: u64,
    /// Peer's perfomance history (holds diff rates) useful to say how peer is performing
    pub performance_history: PerformanceHistory,
}

impl ActivePeer {
    pub fn new(command: mpsc::Sender<PeerCommand>, peer: &Peer) -> Self {
        let choke_status = Status::default();
        Self {
            command,
            ip: peer.ip(),
            bitfield: peer.bitfield.clone(),
            in_status: choke_status.clone(),
            out_status: choke_status,
            downloaded: 0,
            uploaded: 0,
            pending_blocks: Vec::new(),
            performance_history: PerformanceHistory::new(),
        }
    }

    /// Peer's upload speed in bytes per second
    pub fn upload_speed(&self) -> usize {
        todo!()
    }

    pub async fn out_choke(&mut self) {
        self.command.try_send(PeerCommand::Choke).unwrap();
        self.out_status.choke();
    }

    pub async fn out_unchoke(&mut self) {
        self.command.try_send(PeerCommand::Unchoke).unwrap();
        self.out_status.choke();
    }

    pub fn in_choke(&mut self) {
        self.in_status.choke();
    }

    pub fn in_unchoke(&mut self) {
        self.in_status.unchoke();
    }
}

#[derive(Debug, Clone)]
pub struct Status {
    choked: bool,
    choked_time: Instant,
    interested: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            choked: true,
            choked_time: Instant::now(),
            interested: false,
        }
    }
}

impl Status {
    pub fn choke(&mut self) {
        self.choked = true;
        self.choked_time = Instant::now();
    }

    pub fn unchoke(&mut self) {
        self.choked = false;
    }

    pub fn is_choked(&self) -> bool {
        self.choked
    }

    pub fn is_interested(&self) -> bool {
        self.interested
    }

    /// Get duration of being choked returing 0 Duration if currently choked
    pub fn choke_duration(&self) -> Duration {
        if self.is_choked() {
            Duration::ZERO
        } else {
            self.choked_time.elapsed()
        }
    }

    pub fn interest(&mut self) {
        self.interested = true;
    }

    pub fn uninterest(&mut self) {
        self.interested = false;
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Performance {
    pub downloaded: u64,
    pub uploaded: u64,
}

impl Default for Performance {
    fn default() -> Self {
        Self {
            downloaded: 0,
            uploaded: 0,
        }
    }
}

impl Performance {
    pub fn new(downloaded: u64, uploaded: u64) -> Self {
        Self {
            downloaded,
            uploaded,
        }
    }

    /// download in bytes per measurement period
    pub fn speed(&self) -> u64 {
        self.downloaded
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    pub piece: u32,
    pub offset: u32,
    pub length: u32,
}

impl Block {
    pub fn range(&self) -> Range<u32> {
        self.offset..self.offset + self.length
    }

    pub fn empty(size: u32) -> Self {
        Self {
            piece: 0,
            offset: 0,
            length: size,
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub struct PeerDownloadStats {
    pub downloaded: u64,
    pub history: VecDeque<Performance>,
    pub speed: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct DownloadProgress {
    pub peers: Vec<PeerDownloadStats>,
    pub pending_pieces: usize,
    pub percent: f64,
}

impl Display for DownloadProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(&self).unwrap())
    }
}

pub trait ProgressConsumer: Send + 'static {
    fn consume_progress(&mut self, progress: DownloadProgress);
}

impl<F> ProgressConsumer for F
where
    F: FnMut(DownloadProgress) + Send + 'static,
{
    fn consume_progress(&mut self, progress: DownloadProgress) {
        self(progress);
    }
}

impl ProgressConsumer for std::sync::mpsc::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for tokio::sync::mpsc::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.try_send(progress);
    }
}

impl ProgressConsumer for tokio::sync::broadcast::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for tokio::sync::watch::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.send(progress);
    }
}

/// Glue between active peers and scheduler
#[derive(Debug)]
pub struct Download {
    pub info_hash: [u8; 20],
    pub peers_handles: JoinSet<(Uuid, Result<(), PeerError>)>,
    pub status_rx: mpsc::Receiver<PeerStatus>,
    pub status_tx: mpsc::Sender<PeerStatus>,
    pub new_peers: mpsc::Receiver<NewPeer>,
    pub new_peers_join_set: JoinSet<Result<Peer, SocketAddr>>,
    pub pending_new_peers_ips: HashSet<SocketAddr>,
    pub scheduler: Scheduler,
}

impl Download {
    pub async fn new(
        output: impl AsRef<Path>,
        t: Info,
        new_peers: mpsc::Receiver<NewPeer>,
    ) -> Self {
        let info_hash = t.hash();
        let active_peers = JoinSet::new();
        let (status_tx, status_rx) = mpsc::channel(1000);
        let commands = HashMap::new();

        let scheduler = Scheduler::new(output, t, commands);

        Self {
            new_peers,
            new_peers_join_set: JoinSet::new(),
            pending_new_peers_ips: HashSet::new(),
            info_hash,
            peers_handles: active_peers,
            status_rx,
            status_tx,
            scheduler,
        }
    }

    pub async fn start(
        mut self,
        mut progress: impl ProgressConsumer,
        start_peers: Vec<Peer>,
    ) -> anyhow::Result<()> {
        for peer in start_peers {
            self.handle_new_peer(peer).await;
        }
        let mut optimistic_unchoke_interval = tokio::time::interval(Duration::from_secs(30));
        let mut choke_interval = tokio::time::interval(Duration::from_secs(10));
        let mut progress_dispatch_interval = tokio::time::interval(Duration::from_secs(1));

        // immidiate tick
        optimistic_unchoke_interval.tick().await;
        choke_interval.tick().await;

        self.scheduler.start().await;

        loop {
            tokio::select! {
                Some(peer) = self.peers_handles.join_next() => self.handle_peer_join(peer),
                Some(status) = self.status_rx.recv() => {
                    if self.handle_peer_status(status).await {
                        return Ok(())
                    };
                },
                Some(new_peer) = self.new_peers.recv() => {
                    match new_peer {
                        NewPeer::ListenerOrigin(peer) => self.handle_new_peer(peer).await,
                        NewPeer::TrackerOrigin(ip) => self.handle_tracker_peer(ip),
                    };
                },
                Some(Ok(peer)) = self.new_peers_join_set.join_next() => {
                    let ip = match peer {
                        Ok(peer) => {
                            let ip = peer.ip();
                            self.handle_new_peer(peer).await;
                            ip
                        },
                        Err(ip) => ip,
                    };
                    self.pending_new_peers_ips.remove(&ip);
                },
                _ = optimistic_unchoke_interval.tick() => self.handle_optimistic_unchoke().await,
                _ = choke_interval.tick() => self.handle_choke_interval().await,
                _ = progress_dispatch_interval.tick() => self.handle_progress_dispatch(&mut progress),
                else => {
                    break Err(anyhow!("Select branch"));
                }
            }
        }
    }

    fn handle_tracker_peer(&mut self, ip: SocketAddr) {
        if self.pending_new_peers_ips.insert(ip) {
            let info_hash = self.info_hash.clone();
            self.new_peers_join_set.spawn(async move {
                let timeout_duration = Duration::from_millis(500);
                match timeout(timeout_duration, Peer::new_from_ip(ip, info_hash)).await {
                    Ok(Ok(peer)) => Ok(peer),
                    Ok(Err(e)) => {
                        tracing::trace!("Peer with ip {} errored: {}", ip, e);
                        Err(ip)
                    }
                    Err(_) => {
                        tracing::trace!("Peer with ip {} timed out", ip);
                        Err(ip)
                    }
                }
            });
        } else {
            tracing::trace!("Recieved duplicate peer with ip {}", ip);
        }
    }

    async fn handle_new_peer(&mut self, peer: Peer) {
        let (tx, rx) = mpsc::channel(10000);
        let ipc = PeerIPC {
            status_tx: self.status_tx.clone(),
            commands_rx: rx,
        };
        let active_peer = ActivePeer::new(tx, &peer);
        self.scheduler.add_peer(active_peer, peer.uuid).await;
        self.peers_handles.spawn(peer.download(ipc));
    }

    async fn handle_peer_status(&mut self, status: PeerStatus) -> bool {
        match status.message_type {
            PeerStatusMessage::Request { response, block } => {
                println!("Someone requested a block");
                let _ = response.send(self.scheduler.retrieve_piece(block.piece as usize).await);
            }
            PeerStatusMessage::Choked => self.scheduler.handle_peer_choke(status.peer_id).await,
            PeerStatusMessage::Unchoked => self.scheduler.handle_peer_unchoke(status.peer_id).await,
            PeerStatusMessage::Data { block, bytes } => {
                let _ = self
                    .scheduler
                    .save_block(status.peer_id, block, bytes)
                    .await;
                let _ = self.scheduler.schedule(&status.peer_id);
                if self.scheduler.is_torrent_finished() {
                    tracing::info!("Finished downloading torrent");
                    return true;
                };
            }
            PeerStatusMessage::Have { piece } => {
                if let Some(peer) = self.scheduler.peers.get_mut(&status.peer_id) {
                    peer.bitfield.add(piece as usize).unwrap();
                }
            }
            PeerStatusMessage::Afk => {
                if let Some(peer) = self.scheduler.peers.get_mut(&status.peer_id) {
                    if !peer.pending_blocks.is_empty() {
                        self.scheduler.choke_peer(status.peer_id);
                    }
                }
            }
        }
        false
    }

    fn handle_peer_join(
        &mut self,
        join_res: Result<(Uuid, Result<(), PeerError>), tokio::task::JoinError>,
    ) {
        if let Ok((peer_id, Err(peer_err))) = &join_res {
            tracing::warn!(
                "Peer with id: {} joined with error: {:?} {}",
                peer_id,
                peer_err.error_type,
                peer_err.msg
            );
        }

        // remove peer from scheduler or propagate panic
        if let Ok((peer_id, _)) = join_res {
            self.scheduler.remove_peer(peer_id);
        } else {
            panic!("Peer process paniced");
        }
    }

    async fn handle_choke_interval(&mut self) {
        println!("Choke interval");
    }

    async fn handle_optimistic_unchoke(&mut self) {
        println!("Optimistic unchoke interval");
    }

    fn handle_progress_dispatch(&mut self, progress_consumer: &mut impl ProgressConsumer) {
        self.scheduler.register_performance();
        let downloaded_pieces = self.scheduler.bitfield.pieces().count() as f64;
        let total_pieces = self.scheduler.total_pieces() as f64;
        let percent = downloaded_pieces / total_pieces * 100.0;
        let peers = self
            .scheduler
            .peers
            .values()
            .map(|p| PeerDownloadStats {
                downloaded: p.downloaded,
                history: p.performance_history.history.clone(),
                speed: p.performance_history.avg_speed(),
            })
            .collect();
        let progress = DownloadProgress {
            peers,
            percent,
            pending_pieces: self.scheduler.pending_pieces.len(),
        };
        progress_consumer.consume_progress(progress);
    }
}

#[derive(Debug, Clone)]
pub enum PeerCommand {
    Start { block: Block },
    Cancel { block: Block },
    Have { piece: u32 },
    Interested,
    Abort,
    Choke,
    Unchoke,
    NotInterested,
}

#[derive(Debug)]
pub struct PeerStatus {
    pub peer_id: Uuid,
    pub message_type: PeerStatusMessage,
}

#[derive(Debug)]
pub enum PeerStatusMessage {
    Request {
        response: oneshot::Sender<Option<Bytes>>,
        block: Block,
    },
    Choked,
    Unchoked,
    Data {
        block: Block,
        bytes: Bytes,
    },
    Afk,
    /// Peer got new piece available
    Have {
        piece: u32,
    },
}

impl Display for PeerStatusMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerStatusMessage::Request { block, .. } => {
                write!(f, "Request for piece: {}", block.piece)
            }
            PeerStatusMessage::Choked => write!(f, "Choked"),
            PeerStatusMessage::Unchoked => write!(f, "Unchoked"),
            PeerStatusMessage::Data { .. } => write!(f, "Peer batch"),
            PeerStatusMessage::Afk => write!(f, "Afk"),
            PeerStatusMessage::Have { piece } => write!(f, "Have piece {}", piece),
        }
    }
}
