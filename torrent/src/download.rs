use std::{
    collections::{HashSet, VecDeque},
    fmt::Display,
    io::Write,
    net::SocketAddr,
    ops::Range,
    time::{Duration, Instant},
};

use anyhow::Context;
use bytes::{BufMut, Bytes};
use tokio::{sync::mpsc, task::JoinSet, time::timeout};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{
    peers::{BitField, Peer, PeerError, PeerIPC},
    piece_picker::{Priority, ScheduleStrategy},
    protocol::{
        extension::Extension,
        peer::{ExtensionHandshake, HandShake, PeerMessage},
        pex::{PexEntry, PexHistory, PexHistoryEntry, PexMessage},
        ut_metadata::UtMessage,
    },
    scheduler::{self, PendingFiles, Scheduler},
    seeder::Seeder,
    storage::{StorageFeedback, StorageHandle},
    tracker::{DownloadStat, DownloadTracker, TrackerStatus},
    DownloadParams, NewPeer,
};

#[derive(Debug)]
pub enum DownloadMessage {
    SetStrategy(ScheduleStrategy),
    SetFilePriority {
        file_idx: usize,
        priority: Priority,
    },
    PostFullState {
        tx: tokio::sync::oneshot::Sender<FullState>,
    },
    Abort,
    Pause,
    Resume,
}

// TODO: cancel, pause and other control
#[derive(Debug, Clone)]
pub struct DownloadHandle {
    pub download_tx: mpsc::Sender<DownloadMessage>,
    pub cancellation_token: CancellationToken,
}

impl DownloadHandle {
    /// Abort download
    pub fn abort(&self) {
        self.cancellation_token.cancel();
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

    /// Change scheduling strategy
    pub async fn set_strategy(&self, strategy: ScheduleStrategy) -> anyhow::Result<()> {
        self.download_tx
            .send(DownloadMessage::SetStrategy(strategy))
            .await?;
        Ok(())
    }

    /// Change file's priority
    pub async fn set_file_priority(
        &self,
        file_idx: usize,
        priority: Priority,
    ) -> anyhow::Result<()> {
        self.download_tx
            .send(DownloadMessage::SetFilePriority { file_idx, priority })
            .await?;
        Ok(())
    }

    pub async fn full_state(&self) -> anyhow::Result<FullState> {
        use tokio::sync::oneshot;
        let (tx, rx) = oneshot::channel();
        self.download_tx
            .send(DownloadMessage::PostFullState { tx })
            .await?;
        Ok(rx.await?)
    }
}

#[derive(Debug, Clone)]
pub struct PerformanceHistory {
    /// Contains data that represents how difference between two measurements changed
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

    pub fn avg_down_speed(&self) -> u64 {
        if self.history.is_empty() {
            return 0;
        }
        let mut avg = 0;
        for measure in &self.history {
            avg += measure.download_speed();
        }
        avg / self.history.len() as u64
    }

    pub fn avg_down_speed_sec(&self, tick_duration: &Duration) -> u64 {
        let tick_secs = tick_duration.as_secs_f32();
        let download_speed = self.avg_down_speed() as f32 / tick_secs;
        download_speed as u64
    }

    pub fn avg_up_speed(&self) -> u64 {
        if self.history.is_empty() {
            return 0;
        }
        let mut avg = 0;
        for measure in &self.history {
            avg += measure.upload_speed();
        }
        avg / self.history.len() as u64
    }

    pub fn avg_up_speed_sec(&self, tick_duration: &Duration) -> u64 {
        let tick_secs = tick_duration.as_secs_f32();
        let upload_speed = self.avg_up_speed() as f32 / tick_secs;
        upload_speed as u64
    }
}

impl Default for PerformanceHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct InterestedPieces {
    bf: BitField,
    interested_amount: usize,
}

impl InterestedPieces {
    pub fn new(piece_table: &Vec<scheduler::SchedulerPiece>, peer_bf: &BitField) -> Self {
        let bf = BitField::empty(piece_table.len());

        let mut this = Self {
            bf,
            interested_amount: 0,
        };
        this.recalculate(piece_table, peer_bf);
        this
    }

    pub fn amount(&self) -> usize {
        self.interested_amount
    }

    pub fn recalculate(&mut self, piece_table: &[scheduler::SchedulerPiece], peer_bf: &BitField) {
        self.interested_amount = 0;
        for (i, piece) in piece_table.iter().enumerate() {
            if !piece.is_finished && !piece.priority.is_disabled() && peer_bf.has(i) {
                self.interested_amount += 1;
                self.bf.add(i).unwrap();
            } else {
                self.bf.remove(i).unwrap();
            }
        }
    }

    pub fn add_piece(&mut self, piece: usize) {
        if !self.bf.has(piece) {
            self.interested_amount += 1;
            self.bf.add(piece).unwrap();
        }
    }

    pub fn remove_piece(&mut self, piece: usize) {
        if self.bf.has(piece) {
            self.interested_amount -= 1;
            self.bf.remove(piece).unwrap();
        }
    }
}

#[derive(Debug)]
pub struct ActivePeer {
    pub id: Uuid,
    pub ip: SocketAddr,
    pub message_tx: flume::Sender<PeerMessage>,
    pub message_rx: flume::Receiver<PeerMessage>,
    pub bitfield: BitField,
    /// Our status towards peer
    pub out_status: Status,
    /// Peer's status towards us
    pub in_status: Status,
    /// Amount of bytes downloaded from peer
    pub downloaded: u64,
    /// Amount of bytes uploaded to peer
    pub uploaded: u64,
    /// Peer's performance history (holds diff rates) useful to say how peer is performing
    pub performance_history: PerformanceHistory,
    /// Current pointer to the relevant pex history
    pub pex_idx: usize,
    pub last_pex_message_time: Instant,
    pub cancellation_token: CancellationToken,
    pub interested_pieces: InterestedPieces,
    pub handshake: HandShake,
    pub extension_handshake: Option<ExtensionHandshake>,
    /// Amount of blocks that are in flight
    /// Note that this number is approximate and not accurate because of race conditions between chokes and requests
    pub pending_blocks: usize,
}

impl ActivePeer {
    pub fn new(
        message_tx: flume::Sender<PeerMessage>,
        message_rx: flume::Receiver<PeerMessage>,
        peer: &Peer,
        interested_pieces: InterestedPieces,
        pex_idx: usize,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            id: peer.uuid,
            message_tx,
            message_rx,
            ip: peer.ip(),
            bitfield: peer.bitfield.clone(),
            in_status: Status::default(),
            out_status: Status::default(),
            downloaded: 0,
            uploaded: 0,
            performance_history: PerformanceHistory::new(),
            pex_idx,
            last_pex_message_time: Instant::now(),
            cancellation_token,
            interested_pieces,
            handshake: peer.handshake.clone(),
            extension_handshake: peer.extension_handshake.clone(),
            pending_blocks: 0,
        }
    }

    pub fn set_out_choke(&mut self, force: bool) -> anyhow::Result<()> {
        match force {
            true => self.message_tx.try_send(PeerMessage::Choke)?,
            false => self.message_tx.try_send(PeerMessage::Unchoke)?,
        }
        self.out_status.set_choke(force);
        Ok(())
    }

    pub fn set_out_interest(&mut self, force: bool) -> anyhow::Result<()> {
        match force {
            true => self.message_tx.try_send(PeerMessage::Interested)?,
            false => self.message_tx.try_send(PeerMessage::NotInterested)?,
        }
        self.out_status.set_interest(force);
        Ok(())
    }

    pub fn send_extension_message<'e, T: Extension<'e>>(&self, msg: T) -> anyhow::Result<()> {
        let handshake = self
            .extension_handshake
            .as_ref()
            .context("peer doesn't not support extensions")?;
        let extension_id = *handshake
            .dict
            .get(T::NAME)
            .context("extension is not supported by peer")?;
        let extension_message = PeerMessage::Extension {
            extension_id,
            payload: msg.into(),
        };
        self.message_tx.try_send(extension_message)?;
        Ok(())
    }

    #[allow(unused)]
    pub fn send_pex_message(&mut self, latest_idx: usize) {
        // TODO: send the actual message
        self.last_pex_message_time = Instant::now();
        self.pex_idx = latest_idx
    }

    pub fn send_ut_metadata_block(
        &self,
        ut_message: UtMessage,
        piece: Bytes,
    ) -> anyhow::Result<()> {
        // TODO: avoid copying
        // parsing extension on tcp framing step will solve this issue
        // So it will be used like
        // self.message_tx.try_send(PeerMessage::UtExtension {
        //   extension_id,
        //   ut_message,
        //   piece,
        // })?;
        let extension_id = self
            .extension_handshake
            .as_ref()
            .and_then(|h| h.ut_metadata_id())
            .context("get ut_metadata extension id from handshake")?;
        let msg = ut_message.as_bytes();
        let payload = bytes::BytesMut::with_capacity(msg.len() + piece.len());
        let mut writer = payload.writer();
        writer.write_all(&msg)?;
        writer.write_all(&piece)?;

        self.message_tx.try_send(PeerMessage::Extension {
            extension_id,
            payload: writer.into_inner().freeze(),
        })?;
        Ok(())
    }

    /// Send cancel signal to the peer.
    /// It will force peer handle to join
    pub fn cancel_peer(&self) {
        self.cancellation_token.cancel();
    }

    pub fn add_interested(&mut self, piece: usize) {
        self.interested_pieces.add_piece(piece);
        if self.interested_pieces.amount() > 1 && !self.out_status.is_interested() {
            self.set_out_interest(true).unwrap();
        }
    }

    pub fn remove_interested(&mut self, piece: usize) {
        self.interested_pieces.remove_piece(piece);
        if self.interested_pieces.amount() == 0 && self.out_status.is_interested() {
            self.set_out_interest(false).unwrap();
        }
    }

    pub fn recalculate_interested_amount(&mut self, table: &[scheduler::SchedulerPiece]) {
        self.interested_pieces.recalculate(table, &self.bitfield);
        let amount = self.interested_pieces.amount();
        if amount == 0 && self.out_status.is_interested() {
            self.set_out_interest(false).unwrap();
        }
        if amount > 0 && !self.out_status.is_interested() {
            self.set_out_interest(true).unwrap();
        }
    }

    pub fn client_name(&self) -> &'static str {
        self.handshake.peer_id.client_name()
    }
}

#[derive(Debug, Clone, Copy)]
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
    pub fn set_choke(&mut self, force: bool) {
        if force {
            self.choked_time = Instant::now();
        }
        self.choked = force;
    }

    pub fn is_choked(&self) -> bool {
        self.choked
    }

    pub fn set_interest(&mut self, force: bool) {
        self.interested = force;
    }

    pub fn is_interested(&self) -> bool {
        self.interested
    }

    /// Get duration of being choked returning 0 Duration if currently choked
    pub fn choke_duration(&self) -> Duration {
        if self.is_choked() {
            Duration::ZERO
        } else {
            self.choked_time.elapsed()
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Performance {
    pub downloaded: u64,
    pub uploaded: u64,
}

impl Performance {
    pub fn new(downloaded: u64, uploaded: u64) -> Self {
        Self {
            downloaded,
            uploaded,
        }
    }

    /// download in bytes per measurement period
    pub fn download_speed(&self) -> u64 {
        self.downloaded
    }

    /// upload in bytes per measurement period
    pub fn upload_speed(&self) -> u64 {
        self.uploaded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Block {
    pub piece: u32,
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug, Clone)]
pub struct DataBlock {
    pub piece: u32,
    pub offset: u32,
    pub block: Bytes,
}

impl DataBlock {
    pub fn new(piece: u32, offset: u32, block: Bytes) -> Self {
        Self {
            piece,
            offset,
            block,
        }
    }

    pub fn len(&self) -> usize {
        self.block.len()
    }

    pub fn block(&self) -> Block {
        Block {
            piece: self.piece,
            offset: self.offset,
            length: self.block.len() as u32,
        }
    }
}

impl Display for DataBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Block in piece {} with offset {} and length {}",
            self.piece,
            self.offset,
            self.block.len()
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockPosition {
    pub offset: u32,
    pub length: u32,
}

impl BlockPosition {
    pub fn end(&self) -> u32 {
        self.offset + self.length
    }
}

impl From<Block> for BlockPosition {
    fn from(block: Block) -> Self {
        Self {
            offset: block.offset,
            length: block.length,
        }
    }
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
    pub fn from_position(piece: u32, position: BlockPosition) -> Self {
        Self {
            piece,
            offset: position.offset,
            length: position.length,
        }
    }

    pub fn range(&self) -> Range<usize> {
        let offset = self.offset as usize;
        offset..offset + self.length as usize
    }

    pub fn position(&self) -> BlockPosition {
        BlockPosition {
            offset: self.offset,
            length: self.length,
        }
    }

    pub fn empty(size: u32) -> Self {
        Self {
            piece: 0,
            offset: 0,
            length: size,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub enum PeerStateChange {
    Connect,
    Disconnect,
    InChoke(bool),
    OutChoke(bool),
    InInterested(bool),
    OutInterested(bool),
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub enum StateChange {
    FinishedPiece(usize),
    DownloadStateChange(DownloadState),
    TrackerAnnounce(String),
    FilePriorityChange {
        file_idx: usize,
        priority: Priority,
    },
    PeerStateChange {
        ip: SocketAddr,
        change: PeerStateChange,
    },
}

#[derive(Debug)]
pub struct FullStateFile {
    pub path: std::path::PathBuf,
    pub size: u64,
    pub index: usize,
    pub start_piece: usize,
    pub end_piece: usize,
    pub priority: Priority,
}

#[derive(Debug)]
pub struct FullStatePeer {
    pub addr: SocketAddr,
    pub uploaded: u64,
    pub downloaded: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub in_status: Status,
    pub out_status: Status,
    pub interested_amount: usize,
    pub pending_blocks_amount: usize,
    pub client_name: &'static str,
}

#[derive(Debug)]
pub struct FullStateTracker {
    pub url: String,
    pub last_announced_at: Instant,
    pub status: TrackerStatus,
    pub announce_interval: Duration,
}

#[derive(Debug)]
pub struct FullState {
    pub name: String,
    pub total_pieces: usize,
    pub percent: f32,
    pub total_size: u64,
    pub info_hash: [u8; 20],
    pub trackers: Vec<FullStateTracker>,
    pub peers: Vec<FullStatePeer>,
    pub files: Vec<FullStateFile>,
    pub bitfield: BitField,
    pub state: DownloadState,
    pub pending_pieces: Vec<usize>,
    pub tick_num: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerDownloadStats {
    pub ip: SocketAddr,
    pub downloaded: u64,
    pub uploaded: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub interested_amount: usize,
    pub pending_blocks_amount: usize,
}

#[derive(Debug, Clone, serde::Serialize, Default)]
pub struct DownloadProgress {
    pub peers: Vec<PeerDownloadStats>,
    pub percent: f32,
    pub changes: Vec<StateChange>,
    pub tick_num: usize,
}

impl DownloadProgress {
    pub fn download_speed(&self) -> u64 {
        self.peers.iter().map(|p| p.download_speed).sum()
    }
}

#[derive(Debug, serde::Serialize, Default)]
pub struct TrackerStats {
    pub url: String,
    pub announce_interval: Duration,
    pub peers: Option<usize>,
    pub leechers: Option<usize>,
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

impl ProgressConsumer for flume::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for () {
    fn consume_progress(&mut self, _progress: DownloadProgress) {
        ()
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize, Default, PartialEq)]
pub enum DownloadState {
    Paused,
    #[default]
    Pending,
    Seeding,
}

impl Display for DownloadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadState::Paused => write!(f, "Paused"),
            DownloadState::Pending => write!(f, "Pending"),
            DownloadState::Seeding => write!(f, "Seeding"),
        }
    }
}

const MAX_PEER_CONNECTIONS: usize = 75;
/// Keep it not super low to prevent event loop congestion
const DEFAULT_TICK_DURATION: Duration = Duration::from_millis(500);
const OPTIMISTIC_UNCHOKE_INTERVAL: Duration = Duration::from_secs(30);
const CHOKE_INTERVAL: Duration = Duration::from_secs(15);
const PEER_CHANNEL_CAPACITY: usize = 1000;

/// Glue between active peers, scheduler, storage, udp listener
#[derive(Debug)]
pub struct Download {
    info_hash: [u8; 20],
    peers_handles: JoinSet<(Uuid, Result<(), PeerError>)>,
    storage_rx: mpsc::Receiver<StorageFeedback>,
    new_peers: mpsc::Receiver<NewPeer>,
    new_peers_join_set: JoinSet<Result<Peer, SocketAddr>>,
    pending_new_peers_ips: HashSet<SocketAddr>,
    trackers: Vec<DownloadTracker>,
    scheduler: Scheduler,
    storage: StorageHandle,
    pex_history: PexHistory,
    cancellation_token: CancellationToken,
    state: DownloadState,
    tick_duration: Duration,
    last_optimistic_unchoke: Instant,
    last_choke: Instant,
    stat: DownloadStat,
    #[allow(unused)]
    seeder: Seeder,
    changes: Vec<StateChange>,
    info: crate::Info,
    tick_num: usize,
}

impl Download {
    pub fn new(
        storage_feedback: mpsc::Receiver<StorageFeedback>,
        storage: StorageHandle,
        download_params: DownloadParams,
        new_peers: mpsc::Receiver<NewPeer>,
        trackers: Vec<DownloadTracker>,
        cancellation_token: CancellationToken,
    ) -> Self {
        let info = download_params.info;
        let info_hash = info.hash();
        let active_peers = JoinSet::new();
        let output_files = info.output_files("");
        let pending_files = PendingFiles::from_output_files(
            info.piece_length,
            &output_files,
            download_params.files,
        );

        let stat = DownloadStat::new(&download_params.bitfield, &info);

        let scheduler = Scheduler::new(&info, pending_files, &download_params.bitfield);
        let state = scheduler.torrent_state();
        let seeder = Seeder::new(storage.clone());

        Self {
            new_peers,
            new_peers_join_set: JoinSet::new(),
            pending_new_peers_ips: HashSet::new(),
            trackers,
            info_hash,
            peers_handles: active_peers,
            storage_rx: storage_feedback,
            scheduler,
            storage,
            pex_history: PexHistory::new(),
            cancellation_token,
            state,
            tick_duration: DEFAULT_TICK_DURATION,
            last_optimistic_unchoke: Instant::now(),
            last_choke: Instant::now(),
            stat,
            seeder,
            changes: Vec::new(),
            info,
            tick_num: 0,
        }
    }

    pub fn start(
        self,
        progress: impl ProgressConsumer,
        task_tracker: &TaskTracker,
    ) -> DownloadHandle {
        let (download_tx, download_rx) = mpsc::channel(100);
        let download_handle = DownloadHandle {
            download_tx,
            cancellation_token: self.cancellation_token.clone(),
        };
        task_tracker.spawn(self.work(progress, download_rx));
        download_handle
    }

    fn handle_peer_messages(&mut self, peer_idx: usize) {
        let peer_rx = self.scheduler.peers[peer_idx].message_rx.clone();
        let ip = self.scheduler.peers[peer_idx].ip;

        while let Ok(peer_msg) = peer_rx.try_recv() {
            let mut add_peer_change = |change: PeerStateChange| {
                self.changes
                    .push(StateChange::PeerStateChange { ip, change })
            };
            match peer_msg {
                PeerMessage::Choke => {
                    self.scheduler.handle_peer_choke(peer_idx);
                    add_peer_change(PeerStateChange::InChoke(true));
                }
                PeerMessage::Unchoke => {
                    self.scheduler.handle_peer_unchoke(peer_idx);
                    add_peer_change(PeerStateChange::InChoke(false));
                }
                PeerMessage::Interested => {
                    self.scheduler.handle_peer_interest(peer_idx);
                    add_peer_change(PeerStateChange::InInterested(true));
                }
                PeerMessage::NotInterested => {
                    self.scheduler.handle_peer_uninterest(peer_idx);
                    add_peer_change(PeerStateChange::InInterested(false));
                }
                PeerMessage::Have { index } => self
                    .scheduler
                    .handle_peer_have_msg(peer_idx, index as usize),
                PeerMessage::Request {
                    index,
                    begin,
                    length,
                } => {
                    let block = Block {
                        piece: index,
                        offset: begin,
                        length,
                    };
                    // NOTE: this is wrong. We should add it when we are sending requested block.
                    self.stat.uploaded += block.length as u64;
                    let peer = &mut self.scheduler.peers[peer_idx];
                    if !peer.out_status.is_choked() && peer.in_status.is_interested() {
                        peer.uploaded += block.length as u64;
                        tracing::info!("Peer {} requested piece: {index}", peer.ip);
                        self.seeder.request_block(block, peer.message_tx.clone());
                    } else {
                        tracing::warn!(
                            "Peer {} requests piece while choked or not interested",
                            peer.ip
                        );
                    }
                }
                PeerMessage::Piece {
                    index,
                    begin,
                    block,
                } => {
                    let block = DataBlock::new(index, begin, block);
                    self.scheduler.save_block(peer_idx, block);
                }
                PeerMessage::Cancel { .. } => {}
                PeerMessage::Extension {
                    extension_id,
                    payload,
                } => {
                    let peer = &mut self.scheduler.peers[peer_idx];
                    tracing::debug!("Received extension message with id {extension_id}");
                    match extension_id {
                        PexMessage::CLIENT_ID => {
                            let info_hash = self.info_hash;
                            let pex_message = PexMessage::from_bytes(&payload)
                                .context("parse pex message")
                                .unwrap();
                            tracing::debug!(
                                "Received {} new peers from pex message",
                                pex_message.added.len()
                            );
                            for addr in pex_message.added.into_iter().filter_map(|a| {
                                if self
                                    .scheduler
                                    .peers
                                    .iter()
                                    .find(|p| p.ip == a.addr)
                                    .is_none()
                                {
                                    Some(a.addr)
                                } else {
                                    None
                                }
                            }) {
                                self.pending_new_peers_ips.insert(addr);
                                self.new_peers_join_set.spawn(async move {
                                    crate::peers::Peer::new_from_ip(addr, info_hash)
                                        .await
                                        .map_err(|_| addr)
                                });
                            }
                        }
                        UtMessage::CLIENT_ID => {
                            let ut_message = UtMessage::from_bytes(&payload)
                                .context("parse ut_metadata message")
                                .unwrap();
                            match ut_message {
                                UtMessage::Request { piece } => {
                                    if let Some(block) = self.scheduler.ut_metadata.get_piece(piece)
                                    {
                                        peer.send_ut_metadata_block(
                                            UtMessage::Data {
                                                piece,
                                                total_size: self.scheduler.ut_metadata.size,
                                            },
                                            block,
                                        )
                                        .unwrap()
                                    } else {
                                        peer.send_extension_message(UtMessage::Reject { piece })
                                            .unwrap();
                                    };
                                }
                                _ => {}
                            }
                        }
                        _ => {
                            // unknown extension
                        }
                    }
                }
                // It is valid to send the handshake message more than once during the lifetime of a connection,
                // the sending client should not be disconnected.
                // An implementation may choose to ignore the subsequent handshake messages (or parts of them).
                // Subsequent handshake messages can be used to enable/disable extensions without restarting the connection.
                // If a peer supports changing extensions at run time, it should note that the m dictionary is additive.
                // It's enough that it contains the actual CHANGES to the extension list. To disable the support for LT_metadata at run-time,
                // without affecting any other extensions, this message should be sent: d11:LT_metadatai0ee.
                PeerMessage::ExtensionHandshake { .. } => {}
                PeerMessage::Bitfield { .. } => {
                    // logic error
                    self.scheduler.peers[peer_idx].cancel_peer();
                }
                PeerMessage::HeartBeat => {}
            }
        }

        let peer = &self.scheduler.peers[peer_idx];
        if !peer.in_status.is_choked() && peer.out_status.is_interested() {
            self.scheduler.schedule(peer_idx, &self.tick_duration);
        }
    }

    async fn work(
        mut self,
        mut progress: impl ProgressConsumer,
        mut commands_rx: mpsc::Receiver<DownloadMessage>,
    ) -> anyhow::Result<()> {
        // initial tracker announce
        for tracker in &mut self.trackers {
            tracker.announce(self.stat);
        }

        let mut tick_interval = tokio::time::interval(self.tick_duration);

        loop {
            let loop_start = Instant::now();
            // 1. We must remove dropped clients.

            while let Some(peer) = self.peers_handles.try_join_next() {
                self.handle_peer_join(peer);
            }

            // 2. We iterate over all peers, measure performance, schedule more blocks, save ready
            //    blocks, handle their messages

            let mut min_pex_tip = usize::MAX;

            //let prev_pending_amount = self.scheduler.pending_pieces.len();

            // 99% of time spent here
            let handle_peer_messages = Instant::now();
            for i in 0..self.scheduler.peers.len() {
                self.handle_peer_messages(i);
                let pex_idx = self.scheduler.peers[i].pex_idx;
                if pex_idx < min_pex_tip {
                    min_pex_tip = pex_idx
                }
            }
            tracing::debug!(
                "Handled peer's messages in {:?}",
                handle_peer_messages.elapsed()
            );

            // iterate over newly added pieces
            //for piece in &self.scheduler.pending_pieces[prev_pending_amount..] {
            //    for peer in &mut self.scheduler.peers {
            //        if peer.bitfield.has(*piece) {
            //            peer.add_interested();
            //        }
            //    }
            //}

            self.scheduler.pending_pieces.retain(|pending_piece| {
                let piece = &mut self.scheduler.piece_table[*pending_piece];
                let blocks = piece.pending_blocks.as_mut().unwrap();
                let is_full = blocks.is_full();
                if is_full {
                    let pending_blocks = piece.pending_blocks.take().unwrap();
                    piece.is_saving = true;
                    if pending_blocks.is_sub_rational() {
                        self.scheduler.sub_rational_amount -= 1;
                    }
                    self.storage
                        .try_save_piece(*pending_piece, pending_blocks.as_bytes())
                        .unwrap();
                }
                !is_full
            });

            // 3. Once we have everyone's performance up to date we change our choke status if
            //    it is time for optimistic unchoke/choke interval

            if loop_start.duration_since(self.last_optimistic_unchoke) > OPTIMISTIC_UNCHOKE_INTERVAL
            {
                self.last_optimistic_unchoke = loop_start;
                // do optimistic unchoke
            }

            if loop_start.duration_since(self.last_choke) > CHOKE_INTERVAL {
                self.last_choke = loop_start;
                self.scheduler.rechoke_peer();
                // choke someone
            }

            while let Ok(new_peer) = self.new_peers.try_recv() {
                match new_peer {
                    NewPeer::ListenerOrigin(peer) => self.handle_new_peer(peer),
                };
            }

            while let Some(Ok(joined_peer)) = self.new_peers_join_set.try_join_next() {
                let ip = match joined_peer {
                    Ok(peer) => {
                        let ip = peer.ip();
                        self.handle_new_peer(peer);
                        ip
                    }
                    Err(ip) => ip,
                };
                self.pending_new_peers_ips.remove(&ip);
            }

            while let Ok(storage_update) = self.storage_rx.try_recv() {
                self.handle_storage_feedback(storage_update);
            }

            while let Ok(command) = commands_rx.try_recv() {
                self.handle_command(command).await;
            }

            self.scheduler.register_performance();
            self.handle_tracker_updates(loop_start);

            self.handle_progress_dispatch(&mut progress);

            tracing::debug!(took = ?loop_start.elapsed(), "Download tick finished");
            self.tick_num += 1;

            // 4. We sleep until the next tick
            tokio::select! {
                _ = tick_interval.tick() => {}
                _ = self.cancellation_token.cancelled() => {
                    self.handle_shutdown().await;
                    break Ok(());
                }
            }
        }
    }

    fn handle_tracker_updates(&mut self, loop_start: Instant) {
        for tracker in &mut self.trackers {
            if loop_start.duration_since(tracker.last_announced_at) > tracker.announce_interval {
                self.changes
                    .push(StateChange::TrackerAnnounce(tracker.url().to_owned()));
                tracker.announce(self.stat);
            }

            for ip in tracker.handle_messages() {
                if self.scheduler.peers.len() >= MAX_PEER_CONNECTIONS {
                    // TODO: store new peers somewhere for later use
                    continue;
                }
                if !self.scheduler.peers.iter().any(|p| p.ip == ip)
                    && self.pending_new_peers_ips.insert(ip)
                {
                    let info_hash = self.info_hash;
                    self.new_peers_join_set.spawn(async move {
                        let timeout_duration = Duration::from_secs(3);
                        match timeout(timeout_duration, Peer::new_from_ip(ip, info_hash)).await {
                            Ok(Ok(peer)) => Ok(peer),
                            Ok(Err(e)) => {
                                tracing::trace!("Failed to connect peer {ip}: {e}");
                                Err(ip)
                            }
                            Err(_) => {
                                tracing::trace!("Failed to connect peer {ip}: time out");
                                Err(ip)
                            }
                        }
                    });
                } else {
                    tracing::trace!("Received duplicate peer {ip}");
                }
            }
        }
    }

    fn handle_new_peer(&mut self, peer: Peer) {
        if self.scheduler.peers.len() >= MAX_PEER_CONNECTIONS {
            return;
        }
        let total_pieces = self.scheduler.piece_table.len();
        if let Err(e) = peer.bitfield.validate(total_pieces) {
            tracing::warn!("Failed to validate peer's bitfiled: {e}");
            return;
        }
        let (message_tx, message_rx) = flume::bounded(PEER_CHANNEL_CAPACITY);
        let (peer_message_tx, peer_message_rx) = flume::bounded(PEER_CHANNEL_CAPACITY);
        let child_token = self.cancellation_token.child_token();
        let ipc = PeerIPC {
            message_tx: peer_message_tx.clone(),
            message_rx,
        };
        self.pex_history
            .push_value(PexHistoryEntry::added(peer.ip()));
        let pex_tip = self.pex_history.tip();
        let interested_pieces = InterestedPieces::new(&self.scheduler.piece_table, &peer.bitfield);
        let active_peer = ActivePeer::new(
            message_tx,
            peer_message_rx,
            &peer,
            interested_pieces,
            pex_tip,
            child_token.clone(),
        );
        self.peers_handles.spawn(peer.download(ipc, child_token));
        if active_peer
            .extension_handshake
            .as_ref()
            .is_some_and(|h| h.pex_id().is_some())
        {
            let initial_pex_message = PexMessage {
                added: self
                    .scheduler
                    .peers
                    .iter()
                    .map(|p| PexEntry::new(p.ip, None))
                    .collect(),
                dropped: vec![],
            };
            if let Err(e) = active_peer.send_extension_message(initial_pex_message) {
                tracing::warn!("Failed to send pex initial message to peer: {e}")
            };
        }
        self.changes.push(StateChange::PeerStateChange {
            ip: active_peer.ip,
            change: PeerStateChange::Connect,
        });
        self.scheduler.add_peer(active_peer);
    }

    fn handle_peer_join(
        &mut self,
        join_res: Result<(Uuid, Result<(), PeerError>), tokio::task::JoinError>,
    ) {
        if let Ok((uuid, Err(peer_err))) = &join_res {
            tracing::warn!(
                "Peer with id: {} joined with error: {:?} {}",
                uuid,
                peer_err.error_type,
                peer_err.msg
            );
        }

        match join_res {
            Ok((uuid, _)) => {
                let idx = self.scheduler.get_peer_idx(&uuid).unwrap();
                if let Some(removed_peer) = self.scheduler.remove_peer(idx) {
                    self.changes.push(StateChange::PeerStateChange {
                        ip: removed_peer,
                        change: PeerStateChange::Disconnect,
                    });
                    self.pex_history
                        .push_value(PexHistoryEntry::dropped(removed_peer));
                };
            }
            Err(e) => {
                panic!("Peer task panicked: {e}");
            }
        };
    }

    fn handle_progress_dispatch(&mut self, progress_consumer: &mut impl ProgressConsumer) {
        let percent = self.scheduler.percent_pending_pieces();
        let peers = self
            .scheduler
            .peers
            .iter()
            .map(|p| {
                let download_speed = p
                    .performance_history
                    .avg_down_speed_sec(&self.tick_duration);
                let upload_speed = p.performance_history.avg_up_speed_sec(&self.tick_duration);
                PeerDownloadStats {
                    ip: p.ip,
                    downloaded: p.downloaded,
                    uploaded: p.uploaded,
                    interested_amount: p.interested_pieces.amount(),
                    download_speed,
                    upload_speed,
                    pending_blocks_amount: p.pending_blocks,
                }
            })
            .collect();
        let mut changes = Vec::new();
        changes.append(&mut self.changes);
        let progress = DownloadProgress {
            tick_num: self.tick_num,
            peers,
            percent,
            changes,
        };
        progress_consumer.consume_progress(progress);
    }

    fn handle_storage_feedback(&mut self, storage_update: StorageFeedback) {
        match storage_update {
            StorageFeedback::Saved { piece_i } => {
                // NOTE: this is wrong. last piece might be less than piece size
                self.stat.downloaded += self.scheduler.piece_size as u64;
                self.scheduler.add_piece(piece_i);
                self.changes.push(StateChange::FinishedPiece(piece_i));
                if self.scheduler.is_torrent_finished() {
                    tracing::info!("Finished downloading torrent");
                    self.state = DownloadState::Seeding;
                    for peer in &mut self.scheduler.peers {
                        peer.pending_blocks = 0;
                    }
                    self.changes
                        .push(StateChange::DownloadStateChange(self.state));
                };
            }
            StorageFeedback::Failed { piece_i } => {
                self.scheduler.fail_piece(piece_i);
            }
            StorageFeedback::Data { piece_i, bytes } => {
                if let Some(bytes) = bytes {
                    self.seeder.handle_retrieve(piece_i, bytes.clone());
                }
            }
        }
    }

    pub async fn handle_command(&mut self, command: DownloadMessage) {
        match command {
            DownloadMessage::SetStrategy(strategy) => {
                if let ScheduleStrategy::Request(_) = strategy {
                    self.scheduler.max_pending_pieces = 2;
                } else {
                    self.scheduler.max_pending_pieces = 40;
                };
                tracing::debug!(
                    "Switching schedule strategy from {} to {}",
                    self.scheduler.strategy(),
                    strategy,
                );
                self.scheduler.set_strategy(strategy);
            }
            DownloadMessage::SetFilePriority { file_idx, priority } => {
                self.changes
                    .push(StateChange::FilePriorityChange { file_idx, priority });
                if self.scheduler.change_file_priority(file_idx, priority) {
                    if priority == Priority::Disabled {
                        self.storage.disable_file(file_idx).await;
                    } else {
                        self.storage.enable_file(file_idx).await;
                    }
                };
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
            DownloadMessage::PostFullState { tx } => {
                let _ = tx.send(self.full_state());
            }
        };
    }

    pub fn full_state(&self) -> FullState {
        let trackers = self
            .trackers
            .iter()
            .map(|t| FullStateTracker {
                url: t.url().to_owned(),
                last_announced_at: t.last_announced_at,
                status: t.status.clone(),
                announce_interval: t.announce_interval,
            })
            .collect();

        let peers = self
            .scheduler
            .peers
            .iter()
            .map(|p| FullStatePeer {
                addr: p.ip,
                uploaded: p.uploaded,
                downloaded: p.downloaded,
                upload_speed: p.performance_history.avg_up_speed_sec(&self.tick_duration),
                download_speed: p
                    .performance_history
                    .avg_down_speed_sec(&self.tick_duration),
                in_status: p.in_status,
                out_status: p.out_status,
                interested_amount: p.interested_pieces.amount(),
                pending_blocks_amount: p.pending_blocks,
                client_name: p.client_name(),
            })
            .collect();
        let output_files = self.info.output_files("");
        let files = self
            .scheduler
            .pending_files
            .files
            .iter()
            .map(|p| FullStateFile {
                index: p.index,
                start_piece: p.start_piece,
                end_piece: p.end_piece,
                path: output_files[p.index].path().to_owned(),
                size: output_files[p.index].length(),
                priority: p.priority,
            })
            .collect();

        let mut bitfield = BitField::empty(self.scheduler.piece_table.len());
        self.scheduler
            .piece_table
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_finished)
            .for_each(|(i, _)| bitfield.add(i).unwrap());

        let info_hash = self.info_hash;
        let name = self.info.name.clone();
        let total_size = self.info.total_size();
        let total_pieces = self.info.pieces.len();
        let percent = self.scheduler.percent_pending_pieces();
        let tick_num = self.tick_num;

        FullState {
            name,
            total_pieces,
            percent,
            total_size,
            info_hash,
            trackers,
            peers,
            files,
            bitfield,
            state: self.state,
            pending_pieces: self.scheduler.pending_pieces.clone(),
            tick_num,
        }
    }

    pub async fn handle_shutdown(&mut self) {
        tracing::info!("Gracefully shutting down download");
        // wait for peers to close
        while let Some(_) = self.peers_handles.join_next().await {}
    }
}
