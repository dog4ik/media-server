use std::{
    fmt::Display,
    net::SocketAddr,
    ops::Range,
    time::{Duration, Instant},
};

use anyhow::Context;
use bytes::Bytes;
use tokio::{sync::mpsc, task::JoinSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    DownloadParams, FullState, FullStateFile, FullStateTracker,
    bitfield::BitField,
    metric,
    peer_listener::NewPeer,
    peer_storage::PeerStorage,
    peers::{Peer, PeerError, PeerIPC},
    piece_picker::{Priority, ScheduleStrategy},
    progress::{self, events},
    protocol::{
        extension::Extension,
        peer::PeerMessage,
        pex::{PexEntry, PexHistory, PexHistoryEntry, PexMessage},
        ut_metadata::UtMessage,
    },
    scheduler::{PendingFiles, Scheduler},
    seeder::Seeder,
    session::tick_context::TickContext,
    storage::{StorageError, StorageFeedback, StorageHandle},
    tracker::{DownloadStat, DownloadTracker},
};

pub mod peer;

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
    pub info_hash: [u8; 20],
    peers_handles: JoinSet<(Uuid, Result<(), PeerError>)>,
    commands: (
        mpsc::Sender<DownloadMessage>,
        mpsc::Receiver<DownloadMessage>,
    ),
    storage_rx: mpsc::Receiver<StorageFeedback>,
    new_peers: mpsc::Receiver<NewPeer>,
    trackers: Vec<DownloadTracker>,
    scheduler: Scheduler,
    storage: StorageHandle,
    pex_history: PexHistory,
    cancellation_token: CancellationToken,
    state: DownloadState,
    last_optimistic_unchoke: Instant,
    last_choke: Instant,
    stat: DownloadStat,
    seeder: Seeder,
    running_performance: metric::RollingSpeedMeter,
    info: crate::Info,
    peer_storage: PeerStorage,
}

impl Download {
    pub fn performance(&self) -> &metric::RollingSpeedMeter {
        &self.running_performance
    }

    pub fn state(&self) -> DownloadState {
        self.state.clone()
    }

    pub fn connections_count(&self) -> usize {
        self.scheduler.peers.len()
    }

    pub fn total_download(&self) -> u64 {
        self.stat.downloaded
    }

    pub fn total_uploaded(&self) -> u64 {
        self.stat.uploaded
    }
}

impl Download {
    pub fn new(
        storage_feedback: mpsc::Receiver<StorageFeedback>,
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
        let peer_storage = PeerStorage::new(vec![], client_external_ip);

        let commands = mpsc::channel(10);

        Self {
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
            last_optimistic_unchoke: Instant::now(),
            last_choke: Instant::now(),
            stat,
            seeder,
            info,
            commands,
            peer_storage,
            running_performance: metric::RollingSpeedMeter::new(),
        }
    }

    pub fn make_handle(&self) -> DownloadHandle {
        DownloadHandle {
            download_tx: self.commands.0.clone(),
            cancellation_token: self.cancellation_token.clone(),
        }
    }

    fn handle_peer_messages(&mut self, peer_idx: usize, ctx: &mut TickContext) {
        // This single clone holds the entire codebase in piece.
        let peer_rx = self.scheduler.peers[peer_idx].message_rx.clone();
        let ip = self.scheduler.peers[peer_idx].ip;
        let state_before = self.scheduler.peers[peer_idx].current_progress_state();

        while let Ok(peer_msg) = peer_rx.try_recv() {
            // let mut add_peer_change = |change: PeerStateChange| {
            //     self.changes
            //         .push(StateChange::PeerStateChange { ip, change })
            // };
            match peer_msg {
                PeerMessage::Choke => {
                    self.scheduler.handle_peer_choke(peer_idx);
                    // add_peer_change(PeerStateChange::InChoke(true));
                }
                PeerMessage::Unchoke => {
                    self.scheduler.handle_peer_unchoke(peer_idx);
                    // add_peer_change(PeerStateChange::InChoke(false));
                }
                PeerMessage::Interested => {
                    self.scheduler.handle_peer_interest(peer_idx);
                    // add_peer_change(PeerStateChange::InInterested(true));
                }
                PeerMessage::NotInterested => {
                    self.scheduler.handle_peer_uninterest(peer_idx);
                    // add_peer_change(PeerStateChange::InInterested(false));
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
        let state_after = peer.current_progress_state();
        if state_after != state_before {
            ctx.events
                .emit_peer(peer.ip, events::PeerEventKind::StatUpdate(state_after));
        }
        if !peer.in_status.is_choked() && peer.out_status.is_interested() {
            self.scheduler.schedule(peer_idx, &ctx.tick_interval);
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

    #[tracing::instrument(
        level = "debug",
        name = "torrent_download_tick",
        skip_all,
        fields(info_hash = %hex::encode(self.info_hash), tick_num = %ctx.tick_num),
    )]
    pub fn tick(&mut self, ctx: &mut TickContext) {
        // 1. We must remove dropped clients.

        while let Some(peer) = self.peers_handles.try_join_next() {
            self.handle_peer_join(ctx, peer);
        }

        match self.state {
            DownloadState::Error(_) => self.process_paused_tick(),
            DownloadState::Validation { .. } => self.process_paused_tick(),
            DownloadState::Paused => self.process_paused_tick(),
            DownloadState::Pending | DownloadState::Seeding => self.process_active_tick(ctx),
        };

        while let Ok(storage_update) = self.storage_rx.try_recv() {
            self.handle_storage_feedback(ctx, storage_update);
        }

        self.scheduler.register_performance(&ctx);

        self.running_performance.update(
            ctx.tick_start,
            peer::Performance {
                downloaded: self.stat.downloaded,
                uploaded: self.stat.uploaded,
            },
        );

        self.handle_tracker_updates(ctx);
    }

    /// Announce tracker at the start
    pub fn initial_tracker_announce(&mut self) {
        for tracker in &mut self.trackers {
            tracker.announce(self.stat);
        }
    }

    fn handle_tracker_updates(&mut self, ctx: &mut TickContext) {
        for tracker in &mut self.trackers {
            if ctx.tick_start.duration_since(tracker.last_announced_at) > tracker.announce_interval
            {
                tracker.announce(self.stat);
            }

            for ip in tracker.handle_messages(ctx) {
                self.peer_storage.add(ip);
            }
        }
    }

    fn set_download_state(&mut self, ctx: &mut TickContext, new_state: DownloadState) {
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

        ctx.events.emit_state(events::TorrentStateChange(new_state));
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

    fn process_active_tick(&mut self, ctx: &mut TickContext) {
        // 2. We iterate over all peers, measure performance, schedule more blocks, save ready
        //    blocks, handle their messages

        let mut min_pex_tip = usize::MAX;

        //let prev_pending_amount = self.scheduler.pending_pieces.len();

        // 99% of time spent here
        let handle_peer_messages = Instant::now();
        for i in 0..self.scheduler.peers.len() {
            self.handle_peer_messages(i, ctx);
            let peer = &mut self.scheduler.peers[i];
            let pex_idx = peer.pex_idx;
            if peer.last_pex_message_time.duration_since(ctx.tick_start) > PEX_MESSAGE_INTERVAL {
                peer.send_pex_message(&self.pex_history);
            }
            if pex_idx < min_pex_tip {
                min_pex_tip = pex_idx
            }
        }
        tracing::trace!(
            peers_count = self.scheduler.peers.len(),
            took = ?handle_peer_messages.elapsed(),
            "Handled peer's messages",
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

        self.scheduler.tick_pending_pieces(|piece_idx, bytes| {
            self.storage.try_save_piece(piece_idx, bytes).unwrap();
        });

        // 3. Once we have everyone's performance up to date we change our choke status if
        //    it is time for optimistic unchoke/choke interval

        if ctx.tick_start.duration_since(self.last_optimistic_unchoke) > OPTIMISTIC_UNCHOKE_INTERVAL
        {
            self.last_optimistic_unchoke = ctx.tick_start;
            // do optimistic unchoke
        }

        if ctx.tick_start.duration_since(self.last_choke) > CHOKE_INTERVAL {
            self.last_choke = ctx.tick_start;
            self.scheduler.rechoke_peer();
            // choke someone
        }

        while let Some(peer) = self
            .peer_storage
            .join_connected_peer(self.scheduler.piece_table.len())
        {
            self.handle_new_peer(ctx, peer);
        }

        let mut allowed_new_connections = ctx
            .allowed_connections
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
                            self.handle_new_peer(ctx, peer);
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

    fn handle_new_peer(&mut self, ctx: &mut TickContext, peer: Peer) {
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
        ctx.events.emit_peer(
            active_peer.ip,
            events::PeerEventKind::Connect {
                state: Box::new(active_peer.state(&ctx)),
            },
        );
        // self.session.add_peer();
        self.scheduler.add_peer(active_peer);
    }

    fn handle_peer_join(
        &mut self,
        ctx: &mut TickContext,
        join_res: Result<(Uuid, Result<(), PeerError>), tokio::task::JoinError>,
    ) {
        // self.session.remove_peer();
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
                    ctx.events
                        .emit_peer(removed_peer, events::PeerEventKind::Disconnect);
                    self.pex_history
                        .push_value(PexHistoryEntry::dropped(removed_peer));
                };
            }
            Err(e) => {
                panic!("Peer task panicked: {e}");
            }
        };
    }

    fn handle_storage_feedback(&mut self, ctx: &mut TickContext, storage_update: StorageFeedback) {
        match storage_update {
            StorageFeedback::Saved { piece_i } => {
                let piece_length =
                    self.scheduler.piece_length_measurer.piece_length(piece_i) as u64;
                self.stat.downloaded += piece_length;
                self.stat.left -= piece_length;
                self.scheduler.add_piece(piece_i);
                ctx.events
                    .emit_piece(piece_i, events::StoragePieceEventKind::Finished);
                if self.scheduler.is_torrent_finished() {
                    self.set_download_state(ctx, DownloadState::Seeding);
                    for peer in &mut self.scheduler.peers {
                        peer.pending_blocks = 0;
                    }
                };
            }
            StorageFeedback::Error { piece_i, error } => {
                self.scheduler.fail_piece(piece_i);
                match error {
                    crate::storage::StorageError::Fs(_) => self.set_download_state(
                        ctx,
                        DownloadState::Error(DownloadError::Storage(error)),
                    ),
                    crate::storage::StorageError::Hash => {}
                    crate::storage::StorageError::Bounds => unreachable!(),
                    crate::storage::StorageError::MissingPiece => {
                        self.seeder.handle_retrieve_error(piece_i)
                    }
                }
            }
            StorageFeedback::Data { piece_i, bytes } => {
                self.seeder.handle_retrieve(piece_i, bytes);
            }
            StorageFeedback::ValidationProgress { piece, is_valid } => {
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
                            self.set_download_state(ctx, DownloadState::Seeding);
                        } else {
                            self.set_download_state(ctx, DownloadState::Pending)
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

    pub async fn handle_command(&mut self, ctx: &mut TickContext<'_>, command: DownloadMessage) {
        match command {
            DownloadMessage::SetStrategy(strategy) => self.scheduler.set_strategy(strategy),
            DownloadMessage::SetFilePriority { file_idx, priority } => {
                ctx.events.emit_file(
                    file_idx,
                    events::StorageFileEventKind::PriorityChange(priority),
                );
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
                    self.set_download_state(
                        ctx,
                        DownloadState::Validation {
                            validated_amount: 0,
                        },
                    );
                    self.storage.validate().await;
                }
            }
            DownloadMessage::Abort => {
                tracing::debug!("Aborting torrent download");
                self.cancellation_token.cancel();
            }
            DownloadMessage::Pause => {
                self.set_download_state(ctx, DownloadState::Paused);
            }
            DownloadMessage::Resume => {
                if self.scheduler.is_torrent_finished() {
                    self.set_download_state(ctx, DownloadState::Seeding);
                } else {
                    self.set_download_state(ctx, DownloadState::Pending);
                }
            }
            DownloadMessage::PostFullState { tx } => {
                tracing::debug!("Dispatching full torrent progress");
                let _ = tx.send(self.full_state(ctx));
            }
        };
    }

    pub fn full_state(&self, ctx: &mut TickContext) -> FullState {
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

        let peers = self.scheduler.peers.iter().map(|p| p.state(&ctx)).collect();
        let output_files = self.info.output_files("");
        let mut files: Vec<_> = self
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
        files.sort_by_key(|f| f.index);

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
        let percent = self.scheduler.total_downloaded_percent();
        let (download_speed, upload_speed) = self.running_performance.speed();

        FullState {
            name,
            total_pieces,
            percent,
            download_speed,
            upload_speed,
            total_size,
            info_hash,
            trackers,
            peers,
            files,
            bitfield,
            state: self.state.into(),
            pending_pieces: self.scheduler.pending_pieces.clone(),
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

    pub fn construct_torrent_update(
        &self,
        events: events::TorrentTickEvents,
    ) -> progress::TorrentUpdate {
        let (download_speed, upload_speed) = self.running_performance.speed();
        let peer::Performance {
            downloaded,
            uploaded,
        } = self.running_performance.total_downloaded();
        progress::TorrentUpdate {
            events,
            download_speed,
            upload_speed,
            total_downloaded: downloaded,
            total_uploaded: uploaded,
            state: self.state,
            info_hash: self.info_hash,
        }
    }
}
