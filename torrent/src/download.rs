use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
    net::SocketAddr,
    ops::Range,
    time::Duration,
};

use anyhow::anyhow;
use bytes::Bytes;
use tokio::{
    sync::mpsc,
    task::JoinSet,
    time::{timeout, Instant},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    peers::{BitField, Peer, PeerError, PeerIPC},
    protocol::{
        pex::{PexEntry, PexHistory, PexHistoryEntry, PexMessage},
        ut_metadata::{UtMessage, UtMetadata},
        Info,
    },
    scheduler::{ScheduleStrategy, Scheduler},
    storage::{StorageFeedback, StorageHandle},
    NewPeer,
};

#[derive(Debug, Clone)]
pub enum DownloadMessage {
    SetStategy(ScheduleStrategy),
    Abort,
    Pause,
    Resume,
}

// TODO: cancel, pause and other control
#[derive(Debug, Clone)]
pub struct DownloadHandle {
    pub download_tx: mpsc::Sender<DownloadMessage>,
    pub storage: StorageHandle,
    total_pieces: usize,
}

impl DownloadHandle {
    /// Abort download
    pub async fn abort(&self) -> anyhow::Result<()> {
        self.download_tx.send(DownloadMessage::Abort).await?;
        Ok(())
    }

    /// Pause download
    pub async fn pause(&self) -> anyhow::Result<()> {
        self.download_tx.send(DownloadMessage::Pause).await?;
        Ok(())
    }

    /// Resume download
    pub async fn resume(&self) -> anyhow::Result<()> {
        self.download_tx.send(DownloadMessage::Resume).await?;
        Ok(())
    }

    /// Notifiy scheduler about disired piece
    pub async fn ask_piece(&self, piece: usize) -> anyhow::Result<()> {
        self.download_tx
            .send(DownloadMessage::SetStategy(
                ScheduleStrategy::PieceRequest { piece },
            ))
            .await?;
        Ok(())
    }

    /// Change scheduling strategy
    pub async fn set_strategy(&self, strategy: ScheduleStrategy) -> anyhow::Result<()> {
        self.download_tx
            .send(DownloadMessage::SetStategy(strategy))
            .await?;
        Ok(())
    }

    /// Resolves when storage bitfield becomes full
    /// Cancel safe
    pub async fn wait(&mut self) {
        while let Ok(_) = self.storage.bitfield.changed().await {
            let bf = self.storage.bitfield.borrow_and_update();
            if bf.is_full(self.total_pieces) {
                break;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceHistory {
    /// Contains data that reperesents how difference between two measurments changed
    history: VecDeque<Performance>,
    // Snapshot of latest measuremnts. Used to calculate new measurements
    snapshot: Performance,
}

impl PerformanceHistory {
    const MAX_CAPACITY: usize = 20;

    pub fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(Self::MAX_CAPACITY),
            snapshot: Performance::default(),
        }
    }

    pub fn update(&mut self, new: Performance) {
        if self.history.len() == Self::MAX_CAPACITY {
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
        if self.history.is_empty() {
            return 0;
        }
        let mut avg = 0;
        for measure in &self.history {
            avg += measure.speed();
        }
        avg / self.history.len() as u64
    }
}

#[derive(Debug, Clone)]
pub struct ActivePeer {
    pub id: Uuid,
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
    /// Current pointer to the relevant pex history
    pub pex_idx: usize,
}

impl ActivePeer {
    pub fn new(command: mpsc::Sender<PeerCommand>, peer: &Peer, pex_idx: usize) -> Self {
        let choke_status = Status::default();
        Self {
            id: peer.uuid,
            command,
            ip: peer.ip(),
            bitfield: peer.bitfield.clone(),
            in_status: choke_status.clone(),
            out_status: choke_status,
            downloaded: 0,
            uploaded: 0,
            pending_blocks: Vec::new(),
            performance_history: PerformanceHistory::new(),
            pex_idx,
        }
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

impl Display for Block {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Block in piece {} with offset {} and length {}",
            self.piece, self.offset, self.length
        )
    }
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
    pub pending_blocks: usize,
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

/// Glue between active peers, scheduler, storage, udp listener
#[derive(Debug)]
pub struct Download {
    pub info_hash: [u8; 20],
    pub total_pieces: usize,
    pub ut_metadata: UtMetadata,
    pub peers_handles: JoinSet<(Uuid, Result<(), PeerError>)>,
    pub status_rx: mpsc::Receiver<PeerStatus>,
    pub status_tx: mpsc::Sender<PeerStatus>,
    pub storage_rx: mpsc::Receiver<StorageFeedback>,
    pub storage_tx: mpsc::Sender<StorageFeedback>,
    pub new_peers: mpsc::Receiver<NewPeer>,
    pub new_peers_join_set: JoinSet<Result<Peer, SocketAddr>>,
    pub pending_new_peers_ips: HashSet<SocketAddr>,
    pub pending_retrieves: HashMap<usize, Vec<(Uuid, Block)>>,
    pub scheduler: Scheduler,
    pub storage: StorageHandle,
    pub pex_history: PexHistory,
    pub cancellation_token: CancellationToken,
}

impl Download {
    pub async fn new(
        storage: StorageHandle,
        t: Info,
        new_peers: mpsc::Receiver<NewPeer>,
        cancellation_token: CancellationToken,
    ) -> Self {
        let info_hash = t.hash();
        let ut_metadata = UtMetadata::full_from_info(&t);
        let active_peers = JoinSet::new();
        let (status_tx, status_rx) = mpsc::channel(100);
        let (storage_tx, storage_rx) = mpsc::channel(100);
        let total_pieces = t.pieces.len();

        let scheduler = Scheduler::new(t);

        Self {
            new_peers,
            ut_metadata,
            new_peers_join_set: JoinSet::new(),
            pending_new_peers_ips: HashSet::new(),
            pending_retrieves: HashMap::new(),
            info_hash,
            peers_handles: active_peers,
            status_rx,
            status_tx,
            storage_rx,
            storage_tx,
            scheduler,
            storage,
            total_pieces,
            pex_history: PexHistory::new(),
            cancellation_token,
        }
    }

    pub fn start(self, progress: impl ProgressConsumer) -> DownloadHandle {
        let (download_tx, download_rx) = mpsc::channel(100);
        let download_handle = DownloadHandle {
            download_tx,
            total_pieces: self.total_pieces,
            storage: self.storage.clone(),
        };
        tokio::spawn(self.work(progress, download_rx));
        download_handle
    }

    async fn work(
        mut self,
        mut progress: impl ProgressConsumer,
        mut commands_rx: mpsc::Receiver<DownloadMessage>,
    ) -> anyhow::Result<()> {
        let mut optimistic_unchoke_interval = tokio::time::interval(Duration::from_secs(30));
        let mut choke_interval = tokio::time::interval(Duration::from_secs(10));
        let mut progress_dispatch_interval = tokio::time::interval(Duration::from_secs(1));

        // immidiate ticks
        optimistic_unchoke_interval.tick().await;
        choke_interval.tick().await;

        self.scheduler.start().await;

        loop {
            tokio::select! {
                Some(peer) = self.peers_handles.join_next() => self.handle_peer_join(peer),
                Some(status) = self.status_rx.recv() => self.handle_peer_status(status).await,
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
                Some(storage_update) = self.storage_rx.recv() => {
                    if self.handle_storage_feedback(storage_update).await {
                        return Ok(());
                    }
                },
                Some(message) = commands_rx.recv() => self.handle_command(message).await,
                else => {
                    break Err(anyhow!("Select branch"));
                }
            }
        }
    }

    fn handle_tracker_peer(&mut self, ip: SocketAddr) {
        if !self.scheduler.peers.iter().any(|p| p.ip == ip) && self.pending_new_peers_ips.insert(ip)
        {
            let info_hash = self.info_hash;
            self.new_peers_join_set.spawn(async move {
                let timeout_duration = Duration::from_secs(3);
                match timeout(timeout_duration, Peer::new_from_ip(ip, info_hash)).await {
                    Ok(Ok(peer)) => Ok(peer),
                    Ok(Err(e)) => {
                        tracing::error!("Failed to connect peer with ip {}: {}", ip, e);
                        Err(ip)
                    }
                    Err(_) => {
                        tracing::error!("Failed to connect peer with ip {} timed out", ip);
                        Err(ip)
                    }
                }
            });
        } else {
            tracing::warn!("Recieved duplicate peer with ip {}", ip);
        }
    }

    async fn handle_new_peer(&mut self, peer: Peer) {
        let (peer_command_tx, peer_command_rx) = mpsc::channel(100);
        let ipc = PeerIPC {
            status_tx: self.status_tx.clone(),
            commands_rx: peer_command_rx,
        };
        self.pex_history
            .push_value(PexHistoryEntry::added(peer.ip()));
        let pex_tip = self.pex_history.tip();
        let active_peer = ActivePeer::new(peer_command_tx, &peer, pex_tip);
        self.peers_handles.spawn(peer.download(ipc));
        let initial_pex_message = PexMessage {
            added: self
                .scheduler
                .peers
                .iter()
                .map(|p| PexEntry::new(p.ip, None))
                .collect(),
            dropped: vec![],
        };
        let _ = active_peer
            .command
            .send(PeerCommand::Pex {
                msg: initial_pex_message,
            })
            .await;
        self.scheduler.add_peer(active_peer);
        self.scheduler.schedule(self.scheduler.peers.len() - 1)
    }

    async fn handle_peer_status(&mut self, status: PeerStatus) {
        let Some(peer_idx) = self.scheduler.get_peer_idx(&status.peer_id) else {
            tracing::warn!(
                "Failed get peer's index. Peer id: {}, message: {}",
                status.peer_id,
                status.message_type
            );
            return;
        };

        match status.message_type {
            PeerStatusMessage::Request { block } => {
                tracing::info!("Someone have requested block: {block}");
                if let Some(retrieves) = self.pending_retrieves.get_mut(&(block.piece as usize)) {
                    retrieves.push((status.peer_id, block));
                } else {
                    self.pending_retrieves
                        .insert(block.piece as usize, vec![(status.peer_id, block)]);
                    self.storage
                        .retrieve_piece(block.piece as usize, self.storage_tx.clone())
                        .await;
                }
            }
            PeerStatusMessage::Choked => self.scheduler.handle_peer_choke(peer_idx).await,
            PeerStatusMessage::Unchoked => self.scheduler.handle_peer_unchoke(peer_idx).await,
            PeerStatusMessage::Data { block, bytes } => {
                match self.scheduler.save_block(peer_idx, block, bytes).await {
                    Ok(Some((piece_i, data))) => {
                        self.storage
                            .save_piece(piece_i, data, self.storage_tx.clone())
                            .await;
                    }
                    Ok(None) => {}
                    Err(_) => {
                        // tracing::error!("Failed to save block: {e}")
                    }
                }
                let _ = self.scheduler.schedule(peer_idx);
            }
            PeerStatusMessage::Have { piece } => {
                if let Some(peer) = self.scheduler.peers.get_mut(peer_idx) {
                    peer.bitfield.add(piece as usize).unwrap();
                }
            }
            PeerStatusMessage::Afk => {
                if let Some(peer) = self.scheduler.peers.get_mut(peer_idx) {
                    if !peer.pending_blocks.is_empty() {
                        self.scheduler.choke_peer(peer_idx);
                    }
                }
            }
            PeerStatusMessage::UtMetadataBlockRequest { block } => {
                if let Some(Some(bytes)) = self.ut_metadata.blocks.get(block) {
                    if let Some(peer) = self.scheduler.peers.get_mut(peer_idx) {
                        let ut_message = UtMessage::Data {
                            piece: block,
                            total_size: self.ut_metadata.size,
                        };
                        peer.command
                            .try_send(PeerCommand::UtMetadata {
                                msg: ut_message,
                                data: bytes.clone(),
                            })
                            .unwrap();
                    }
                } else {
                    tracing::error!("Non existand ut metadata block {block}");
                }
            }
            PeerStatusMessage::PexMessage { msg } => {
                tracing::info!("Recieved pex message with {} new peers", msg.added.len());
                for added_peer in msg.added {
                    self.handle_tracker_peer(added_peer.addr);
                }
            }
            PeerStatusMessage::PexRequest => {
                if let Some(peer) = self.scheduler.peers.get_mut(peer_idx) {
                    let msg = self.pex_history.pex_message(peer.pex_idx);
                    peer.pex_idx = self.pex_history.tip();
                    peer.command.try_send(PeerCommand::Pex { msg }).unwrap();
                }
            }
        }
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
        match join_res {
            Ok((peer_id, _)) => {
                let idx = self.scheduler.get_peer_idx(&peer_id).unwrap();

                if let Some(removed_peer) = self.scheduler.remove_peer(idx) {
                    self.pex_history
                        .push_value(PexHistoryEntry::dropped(removed_peer.ip));
                };
            }
            Err(e) => {
                panic!("Peer process paniced: {e}");
            }
        };
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
            .iter()
            .map(|p| PeerDownloadStats {
                downloaded: p.downloaded,
                history: p.performance_history.history.clone(),
                speed: p.performance_history.avg_speed(),
                pending_blocks: p.pending_blocks.len(),
            })
            .collect();
        let progress = DownloadProgress {
            peers,
            percent,
            pending_pieces: self.scheduler.pending_pieces.len(),
        };
        progress_consumer.consume_progress(progress);
    }

    async fn handle_storage_feedback(&mut self, storage_update: StorageFeedback) -> bool {
        match storage_update {
            StorageFeedback::Saved { piece_i } => {
                self.scheduler.add_piece(piece_i);
                self.scheduler.pending_saved_pieces.remove(&piece_i);
                if self.scheduler.is_torrent_finished() {
                    tracing::info!("Finished downloading torrent");
                    return true;
                };
            }
            StorageFeedback::Failed { piece_i } => {
                self.scheduler.pending_saved_pieces.remove(&piece_i);
            }
            StorageFeedback::Data { piece_i, bytes } => {
                let retrieves = self.pending_retrieves.remove(&piece_i).unwrap();
                if let Some(bytes) = bytes {
                    for (id, block) in retrieves {
                        self.scheduler
                            .send_block_to_peer(&id, block, bytes.clone())
                            .await;
                    }
                }
            }
        }
        false
    }

    pub async fn handle_command(&mut self, command: DownloadMessage) {
        match command {
            DownloadMessage::SetStategy(strategy) => {
                if let ScheduleStrategy::PieceRequest { .. } = strategy {
                    self.scheduler.max_pending_pieces = 2;
                };
                tracing::debug!(
                    "Switching schedule startegy from {} to {}",
                    self.scheduler.schedule_stategy,
                    strategy,
                );
                self.scheduler.schedule_stategy = strategy;
            }
            DownloadMessage::Abort => {
                tracing::debug!("Aborting torrent download");
                self.cancellation_token.cancel();
            }
            DownloadMessage::Pause => {
                tracing::warn!("Pause is not implemented")
            }
            DownloadMessage::Resume => {
                tracing::warn!("Resume is not implemented")
            }
        };
    }
}

#[derive(Debug, Clone)]
pub enum PeerCommand {
    Start { block: Block },
    Block { block: Block, data: Bytes },
    Cancel { block: Block },
    Have { piece: u32 },
    Pex { msg: PexMessage },
    UtMetadata { msg: UtMessage, data: Bytes },
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
    /// Peer requested block
    Request { block: Block },
    /// Peer choked us
    Choked,
    /// Peer unchoked us
    Unchoked,
    /// Peer send us data
    Data { block: Block, bytes: Bytes },
    /// Peer does not show signs of activity
    Afk,
    /// Peer got new piece available
    Have { piece: u32 },
    /// Peer requested ut_metadata block
    UtMetadataBlockRequest { block: usize },
    /// Peer send us pex message
    PexMessage { msg: PexMessage },
    /// Its time for pex message
    PexRequest,
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
            PeerStatusMessage::UtMetadataBlockRequest { block } => {
                write!(f, "ut_metadata block {}", block)
            }
            PeerStatusMessage::PexMessage { msg } => {
                write!(
                    f,
                    "pex message with {} entries added and {} entries removed",
                    msg.added.len(),
                    msg.dropped.len()
                )
            }
            PeerStatusMessage::PexRequest => {
                write!(f, "pex request")
            }
        }
    }
}
