use std::{
    fmt::Display,
    net::SocketAddr,
    ops::Range,
    time::{Duration, Instant},
};

use anyhow::Context;
use bytes::Bytes;
use progress_consumer::{DownloadProgress, ProgressConsumer};
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{
    DownloadParams, FullState, FullStateFile, FullStatePeer, FullStateTracker, PeerDownloadStats,
    PeerStateChange, StateChange,
    bitfield::BitField,
    peer_listener::NewPeer,
    peer_storage::PeerStorage,
    peers::{Peer, PeerError, PeerIPC},
    piece_picker::{Priority, ScheduleStrategy},
    protocol::{
        extension::Extension,
        peer::PeerMessage,
        pex::{PexEntry, PexHistory, PexHistoryEntry, PexMessage},
        ut_metadata::UtMessage,
    },
    scheduler::{PendingFiles, Scheduler},
    seeder::Seeder,
    session::SessionContext,
    storage::{StorageError, StorageFeedback, StorageHandle, StorageResult},
    tracker::{DownloadStat, DownloadTracker},
};

pub mod peer;
/// Torrent download progress types
pub mod progress_consumer;

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
    Validate,
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

    /// Validate files
    pub async fn validate(&self) -> anyhow::Result<()> {
        self.download_tx.send(DownloadMessage::Validate).await?;
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

impl PartialEq for DataBlock {
    fn eq(&self, other: &Self) -> bool {
        self.piece == other.piece
            && self.offset == other.offset
            && self.block.len() == other.block.len()
    }
}

impl Eq for DataBlock {}

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DownloadError {
    Storage(StorageError),
}

impl std::error::Error for DownloadState {}

impl Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadError::Storage(e) => write!(f, "storage error: {e}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum DownloadState {
    Error(DownloadError),
    Validation {
        validated_amount: usize,
    },
    Paused,
    #[default]
    Pending,
    Seeding,
}

impl DownloadState {
    /// When torrent is paused event loop will not accept incoming connections.
    /// All peer connections should be dropped and no messages should be received / send.
    pub fn is_paused(&self) -> bool {
        match self {
            DownloadState::Error(_) | DownloadState::Validation { .. } | DownloadState::Paused => {
                true
            }
            DownloadState::Pending | DownloadState::Seeding => false,
        }
    }
}

impl Display for DownloadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DownloadState::Error(e) => write!(f, "Error: {e}"),
            DownloadState::Validation { .. } => write!(f, "Validation"),
            DownloadState::Paused => write!(f, "Paused"),
            DownloadState::Pending => write!(f, "Pending"),
            DownloadState::Seeding => write!(f, "Seeding"),
        }
    }
}

/// Keep it not super low to prevent event loop congestion
const DEFAULT_TICK_DURATION: Duration = Duration::from_millis(500);
const OPTIMISTIC_UNCHOKE_INTERVAL: Duration = Duration::from_secs(30);
pub const CHOKE_INTERVAL: Duration = Duration::from_secs(15);
pub const PEER_IN_CHANNEL_CAPACITY: usize = 1000;
pub const PEER_OUT_CHANNEL_CAPACITY: usize = 2000;
pub const PEER_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PEX_MESSAGE_INTERVAL: Duration = Duration::from_secs(90);
// How many unused pex history entries trigger the cleanup
const PEX_HISTORY_CLEANUP_THRESHOLD: usize = 500;

/// Glue between active peers, scheduler, storage, udp listener
#[derive(Debug)]
pub struct Download {
    session: std::sync::Arc<SessionContext>,
    info_hash: [u8; 20],
    peers_handles: JoinSet<(Uuid, Result<(), PeerError>)>,
    storage_rx: mpsc::Receiver<Result<StorageFeedback, StorageError>>,
    new_peers: mpsc::Receiver<NewPeer>,
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
    seeder: Seeder,
    changes: Vec<StateChange>,
    info: crate::Info,
    tick_num: usize,
    peer_storage: PeerStorage,
}

impl Download {
    pub fn new(
        session: std::sync::Arc<SessionContext>,
        storage_feedback: mpsc::Receiver<StorageResult<StorageFeedback>>,
        storage: StorageHandle,
        download_params: DownloadParams,
        new_peers: mpsc::Receiver<NewPeer>,
        trackers: Vec<DownloadTracker>,
        cancellation_token: CancellationToken,
        client_external_ip: Option<SocketAddr>,
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
        // TODO: Known external ip is not guaranteed!
        let peer_storage = PeerStorage::new(vec![], client_external_ip);

        Self {
            session,
            new_peers,
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
            peer_storage,
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
        let ctx = self.session.clone();
        task_tracker.spawn(async move {
            if let Err(e) = self.work(progress, download_rx).await {
                tracing::error!("Torrent download quit with error: {e}");
            };
            ctx.remove_torrent();
        });
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
                PeerMessage::Request(block) => {
                    // NOTE: this is wrong. We should add it when we are sending requested block.
                    self.stat.uploaded += block.length as u64;
                    let peer = &mut self.scheduler.peers[peer_idx];
                    if !peer.out_status.is_choked() && peer.in_status.is_interested() {
                        peer.uploaded += block.length as u64;
                        tracing::info!("Peer {} requested piece: {}", peer.ip, block.piece);
                        self.seeder.request_block(block, peer.message_tx.clone());
                    }
                }
                PeerMessage::Piece(block) => {
                    self.scheduler.save_block(peer_idx, block);
                }
                PeerMessage::Cancel { .. } => {}
                PeerMessage::Extension {
                    extension_id,
                    payload,
                } => {
                    tracing::debug!("Received extension message with id {extension_id}");
                    match extension_id {
                        PexMessage::CLIENT_ID => {
                            if let Err(e) = self.handle_pex_message(payload) {
                                tracing::warn!(%ip, "Failed to process pex message: {e}");
                            }
                        }
                        UtMessage::CLIENT_ID => {
                            if let Err(e) = self.handle_ut_message(peer_idx, payload) {
                                tracing::warn!(%ip, "Failed to process ut message: {e}");
                            };
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

    fn handle_pex_message(&mut self, payload: Bytes) -> anyhow::Result<()> {
        let pex_message = PexMessage::from_bytes(&payload).context("parse pex message")?;
        tracing::debug!(
            "Received {} new peers from pex message",
            pex_message.added.len()
        );
        for entry in pex_message.added {
            self.peer_storage.add(entry.addr);
        }
        Ok(())
    }

    fn handle_ut_message(&mut self, peer_idx: usize, payload: Bytes) -> anyhow::Result<()> {
        let ut_message = UtMessage::from_bytes(&payload).context("parse ut_metadata message")?;
        if let UtMessage::Request { piece } = ut_message {
            let peer = &self.scheduler.peers[peer_idx];
            if let Some(block) = self.scheduler.ut_metadata.get_piece(piece) {
                peer.send_ut_metadata_block(
                    UtMessage::Data {
                        piece,
                        total_size: self.scheduler.ut_metadata.size,
                    },
                    block,
                )?
            } else {
                peer.send_extension_message(UtMessage::Reject { piece })?;
            };
        }
        Ok(())
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
            tracing::trace!(download_state = %self.state, tick_num = %self.tick_num, "Started new download tick");
            // 1. We must remove dropped clients.

            while let Some(peer) = self.peers_handles.try_join_next() {
                self.handle_peer_join(peer);
            }

            match self.state {
                DownloadState::Error(_) => self.process_paused_tick(),
                DownloadState::Validation { .. } => self.process_paused_tick(),
                DownloadState::Paused => self.process_paused_tick(),
                DownloadState::Pending | DownloadState::Seeding => {
                    self.process_active_tick(loop_start).await
                }
            };

            while let Ok(storage_update) = self.storage_rx.try_recv() {
                self.handle_storage_feedback(storage_update);
            }

            self.scheduler.register_performance();
            self.handle_tracker_updates(loop_start);

            self.handle_progress_dispatch(&mut progress);

            tracing::trace!(took = ?loop_start.elapsed(), "Download tick finished");
            self.tick_num += 1;

            loop {
                // 4. We sleep until the next tick
                tokio::select! {
                    _ = tick_interval.tick() => {
                        break;
                    }
                    Some(command) = commands_rx.recv() => self.handle_command(command).await,
                    _ = self.cancellation_token.cancelled() => {
                        self.handle_shutdown().await;
                        return Ok(());
                    }
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
                self.peer_storage.add(ip);
            }
        }
    }

    fn set_download_state(&mut self, new_state: DownloadState) {
        if new_state == self.state {
            tracing::warn!(%new_state, "Redundant state change");
            return;
        }
        match new_state {
            DownloadState::Error(e) => {
                tracing::error!("Setting download state to error: {e}")
            }
            DownloadState::Validation { .. } => {
                tracing::info!("Setting download state to validation")
            }
            DownloadState::Paused => tracing::info!("Setting download state to paused"),
            DownloadState::Pending => tracing::info!("Setting download state to pending"),
            DownloadState::Seeding => tracing::info!("Setting download state to seeding"),
        }
        // handle resume
        if self.state.is_paused() && !new_state.is_paused() {
            debug_assert!(self.scheduler.peers.is_empty());
        }

        // handle pause
        if !self.state.is_paused() && new_state.is_paused() {
            // Peer will join later
            for peer in &self.scheduler.peers {
                peer.cancel_peer();
            }
        }

        self.changes
            .push(StateChange::DownloadStateChange(new_state.into()));
        self.state = new_state;
    }

    fn process_paused_tick(&mut self) {
        while self.peer_storage.discard_store_connected_peer().is_some() {}
        while self
            .peer_storage
            .discard_channel_peer(&mut self.new_peers)
            .is_some()
        {}
    }

    async fn process_active_tick(&mut self, loop_start: Instant) {
        // 2. We iterate over all peers, measure performance, schedule more blocks, save ready
        //    blocks, handle their messages

        let mut min_pex_tip = usize::MAX;

        //let prev_pending_amount = self.scheduler.pending_pieces.len();

        // 99% of time spent here
        let handle_peer_messages = Instant::now();
        for i in 0..self.scheduler.peers.len() {
            self.handle_peer_messages(i);
            let peer = &mut self.scheduler.peers[i];
            let pex_idx = peer.pex_idx;
            if peer.last_pex_message_time.duration_since(loop_start) > PEX_MESSAGE_INTERVAL {
                peer.send_pex_message(&self.pex_history);
            }
            if pex_idx < min_pex_tip {
                min_pex_tip = pex_idx
            }
        }
        tracing::trace!(
            "Handled peer's messages in {:?}",
            handle_peer_messages.elapsed()
        );

        if min_pex_tip != usize::MAX
            && min_pex_tip != 0
            && self.pex_history.tip() - min_pex_tip > PEX_HISTORY_CLEANUP_THRESHOLD
        {
            tracing::debug!(min_pex_tip, pex_tip = %self.pex_history.tip(), "Shrinking pex history");
            self.shrink_pex_history(min_pex_tip);
        }

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
                    // no available capacity
                    .unwrap();
            }
            !is_full
        });

        // 3. Once we have everyone's performance up to date we change our choke status if
        //    it is time for optimistic unchoke/choke interval

        if loop_start.duration_since(self.last_optimistic_unchoke) > OPTIMISTIC_UNCHOKE_INTERVAL {
            self.last_optimistic_unchoke = loop_start;
            // do optimistic unchoke
        }

        if loop_start.duration_since(self.last_choke) > CHOKE_INTERVAL {
            self.last_choke = loop_start;
            self.scheduler.rechoke_peer();
            // choke someone
        }

        while let Some(peer) = self
            .peer_storage
            .join_connected_peer(self.scheduler.piece_table.len())
        {
            self.handle_new_peer(peer);
        }

        let max_connections_per_torrent = self.session.max_connections_per_torrent();

        let mut allowed_new_connections = max_connections_per_torrent
            .saturating_sub(self.scheduler.peers.len() + self.peer_storage.pending_amount());

        while let Ok(new_peer) = self.new_peers.try_recv() {
            match new_peer {
                NewPeer::ListenerOrigin(peer) => {
                    if allowed_new_connections > 0 {
                        if self
                            .peer_storage
                            .accept_new_peer(&peer, self.scheduler.piece_table.len())
                            .is_some()
                        {
                            allowed_new_connections -= 1;
                            self.handle_new_peer(peer);
                        }
                    } else {
                        self.peer_storage
                            .add_validate(peer, self.scheduler.piece_table.len());
                    }
                }
            };
        }

        if allowed_new_connections > 0 {
            while self.peer_storage.connect_best(&self.info_hash).is_some() {
                allowed_new_connections -= 1;
                if allowed_new_connections == 0 {
                    break;
                }
            }
        }
    }

    fn handle_new_peer(&mut self, peer: Peer) {
        let (message_tx, message_rx) = flume::bounded(PEER_OUT_CHANNEL_CAPACITY);
        let (peer_message_tx, peer_message_rx) = flume::bounded(PEER_IN_CHANNEL_CAPACITY);
        let child_token = self.cancellation_token.child_token();
        let ipc = PeerIPC {
            message_tx: peer_message_tx.clone(),
            message_rx,
        };
        self.pex_history
            .push_value(PexHistoryEntry::added(peer.ip()));
        let pex_tip = self.pex_history.tip();
        let interested_pieces =
            peer::InterestedPieces::new(&self.scheduler.piece_table, &peer.bitfield);
        let active_peer = peer::ActivePeer::new(
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
        self.session.add_peer();
        self.scheduler.add_peer(active_peer);
    }

    fn handle_peer_join(
        &mut self,
        join_res: Result<(Uuid, Result<(), PeerError>), tokio::task::JoinError>,
    ) {
        self.session.remove_peer();
        if let Ok((uuid, Err(peer_err))) = &join_res {
            tracing::trace!(
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
                    self.peer_storage.join_disconnected_peer(removed_peer);
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
        let percent = match self.state {
            DownloadState::Validation { validated_amount } => {
                validated_amount as f32 / self.scheduler.piece_table.len() as f32 * 100.
            }
            _ => self.scheduler.downloaded_pieces_percent(),
        };
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

    fn handle_storage_feedback(&mut self, storage_update: Result<StorageFeedback, StorageError>) {
        match storage_update {
            Ok(StorageFeedback::Saved { piece_i }) => {
                self.stat.downloaded +=
                    self.scheduler.piece_length_measurer.piece_length(piece_i) as u64;
                self.scheduler.add_piece(piece_i);
                self.changes.push(StateChange::FinishedPiece(piece_i));
                if self.scheduler.is_torrent_finished() {
                    self.set_download_state(DownloadState::Seeding);
                    for peer in &mut self.scheduler.peers {
                        peer.pending_blocks = 0;
                    }
                };
            }
            Err(StorageError { piece, kind }) => {
                self.scheduler.fail_piece(piece);
                match kind {
                    crate::storage::StorageErrorKind::Fs(_) => self.set_download_state(
                        DownloadState::Error(DownloadError::Storage(StorageError { kind, piece })),
                    ),
                    crate::storage::StorageErrorKind::Hash => {}
                    crate::storage::StorageErrorKind::Bounds => unreachable!(),
                    crate::storage::StorageErrorKind::MissingPiece => {
                        self.seeder.handle_retrieve_error(piece)
                    }
                }
            }
            Ok(StorageFeedback::Data { piece_i, bytes }) => {
                self.seeder.handle_retrieve(piece_i, bytes);
            }
            Ok(StorageFeedback::ValidationProgress { piece, is_valid }) => {
                tracing::debug!(piece, is_valid, "Validation progress");
                if let DownloadState::Validation { validated_amount } = &mut self.state {
                    tracing::trace!(
                        piece,
                        is_valid,
                        validated_amount,
                        "Received validation progress"
                    );
                    *validated_amount += 1;
                    if *validated_amount == self.scheduler.piece_table.len() {
                        tracing::info!("Torrent validation finished, changing download status");
                        if self.scheduler.is_torrent_finished() {
                            self.set_download_state(DownloadState::Seeding);
                        } else {
                            self.set_download_state(DownloadState::Pending)
                        };
                    }
                } else {
                    tracing::warn!(current_state = %self.state, "Received validation progress while not in validation state");
                }
                self.scheduler
                    .handle_piece_validation_result(piece, is_valid);
            }
        }
    }

    pub async fn handle_command(&mut self, command: DownloadMessage) {
        match command {
            DownloadMessage::SetStrategy(strategy) => self.scheduler.set_strategy(strategy),
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
            DownloadMessage::Validate => {
                if let DownloadState::Validation { .. } = self.state {
                    tracing::warn!("Ignoring redundant validation request");
                } else {
                    self.set_download_state(DownloadState::Validation {
                        validated_amount: 0,
                    });
                    self.storage.validate().await;
                }
            }
            DownloadMessage::Abort => {
                tracing::debug!("Aborting torrent download");
                self.cancellation_token.cancel();
            }
            DownloadMessage::Pause => {
                self.set_download_state(DownloadState::Paused);
            }
            DownloadMessage::Resume => {
                if self.scheduler.is_torrent_finished() {
                    self.set_download_state(DownloadState::Seeding);
                } else {
                    self.set_download_state(DownloadState::Pending);
                }
            }
            DownloadMessage::PostFullState { tx } => {
                tracing::debug!("Dispatching full torrent progress");
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
                client_name: p.client_name().to_string(),
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
        let percent = self.scheduler.downloaded_pieces_percent();
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
            state: self.state.into(),
            pending_pieces: self.scheduler.pending_pieces.clone(),
            tick_num,
        }
    }

    pub async fn handle_shutdown(&mut self) {
        tracing::info!("Gracefully shutting down download");
        // wait for peers to close
        while let Some(_) = self.peers_handles.join_next().await {}
    }

    fn shrink_pex_history(&mut self, min_tip: usize) {
        self.pex_history.shrink(min_tip);
        for peer in &mut self.scheduler.peers {
            peer.pex_idx -= min_tip;
        }
    }
}
