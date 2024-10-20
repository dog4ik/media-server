use std::{cmp::Reverse, collections::HashSet, fmt::Display, ops::Range};

use anyhow::{anyhow, ensure, Context};
use bytes::Bytes;
use uuid::Uuid;

use crate::{
    download::{ActivePeer, Block, BlockGlobalLocation, BlockPosition, DataBlock, Performance},
    peers::PeerLogicError,
    protocol::{
        extension::Extension,
        peer::PeerMessage,
        pex::PexMessage,
        ut_metadata::{UtMessage, UtMetadata},
        Info, OutputFile,
    },
};

#[derive(Debug, Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    Disabled = 0,
    Low = 1,
    #[default]
    Medium = 2,
    High = 3,
}

impl Priority {
    pub fn is_disabled(&self) -> bool {
        *self == Priority::Disabled
    }
}

impl TryFrom<usize> for Priority {
    type Error = anyhow::Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        let priority = match value {
            0 => Self::Disabled,
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            _ => return Err(anyhow!("expected value in range 0..4, got {}", value)),
        };
        Ok(priority)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PendingFile {
    pub priority: Priority,
    pub index: usize,
    pub start_piece: usize,
    pub end_piece: usize,
}

impl PendingFile {
    pub fn pieces_range(&self) -> Range<usize> {
        self.start_piece..self.end_piece + 1
    }
}

#[derive(Debug, Clone)]
pub struct PendingFiles {
    files: Vec<PendingFile>,
}

impl PendingFiles {
    pub fn from_output_files(
        piece_length: u32,
        output_files: &[OutputFile],
        enabled_files: Vec<usize>,
    ) -> Self {
        let mut offset = 0;
        let mut files = Vec::with_capacity(output_files.len());
        for (i, file) in output_files.iter().enumerate() {
            let length = file.length();
            let end = offset + length;
            let start_piece = offset / piece_length as u64;
            let end_piece = end / piece_length as u64;
            let priority = if enabled_files.contains(&i) {
                Priority::default()
            } else {
                Priority::Disabled
            };
            files.push(PendingFile {
                priority,
                start_piece: start_piece as usize,
                index: i,
                end_piece: end_piece as usize,
            });
            offset += length;
        }
        files.sort_unstable_by_key(|x| x.priority);
        Self { files }
    }

    /// Change file priority returning previous priority and changed file
    /// `None` if priority is the same or file index out of bounds
    pub fn change_file_priority(
        &mut self,
        idx: usize,
        new_priority: Priority,
    ) -> Option<(Priority, PendingFile)> {
        if let Some(file) = self.files.iter_mut().find(|f| f.index == idx) {
            if file.priority != new_priority {
                let old_priority = file.priority;
                file.priority = new_priority;
                let file_copy = *file;
                self.files.sort_unstable_by_key(|x| x.priority);
                return Some((old_priority, file_copy));
            }
            return None;
        };
        tracing::warn!(
            "File index is out of bounds, got {idx} expected < {}",
            self.files.len()
        );
        None
    }

    /// Iterator over enabled files in Priority order
    pub fn enabled_files(&self) -> impl Iterator<Item = PendingFile> + '_ {
        self.files.iter().copied().rev()
    }
}

#[derive(Debug, Default, Clone)]
pub enum ScheduleStrategy {
    #[default]
    Linear,
    RareFirst,
    PieceRequest {
        piece: usize,
    },
}

impl ScheduleStrategy {
    /// Allow expanding pending pieces list to satifsfy scheduler
    pub fn allow_expansion(&self) -> bool {
        match self {
            ScheduleStrategy::PieceRequest { .. } => false,
            _ => true,
        }
    }

    pub fn create_piece_queue(&self, table: &Vec<SchedulerPiece>) -> Vec<usize> {
        match self {
            ScheduleStrategy::Linear => (0..table.len()).rev().collect(),
            ScheduleStrategy::RareFirst => todo!("rare first strategy"),
            ScheduleStrategy::PieceRequest { piece: _ } => todo!("piece request strategy"),
        }
    }
}

impl Display for ScheduleStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduleStrategy::Linear => write!(f, "Linear"),
            ScheduleStrategy::RareFirst => write!(f, "Rare first"),
            ScheduleStrategy::PieceRequest { piece } => write!(f, "Piece request: {}", piece),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingPieceV2 {
    pub piece: Vec<Option<Bytes>>,
    pub piece_length: u32,
    pub participants: HashSet<Uuid>,
    pub blocks: Vec<BlockPosition>,
    /// Amount of blocks are saved in piece
    pub saved_amount: u8,
}

impl PendingPieceV2 {
    pub fn new(piece_size: usize) -> Self {
        let piece_size = piece_size as u32;
        // same as (piece_size + BLOCK_LENGTH - 1) / BLOCK_LENGTH;
        let blocks_amount = piece_size.div_ceil(BLOCK_LENGTH);
        let piece = vec![None; blocks_amount as usize];
        let blocks: Vec<_> = (0..blocks_amount)
            .into_iter()
            .map(|i| {
                let offset = BLOCK_LENGTH * i;
                let length = if i == blocks_amount - 1 {
                    piece_size - offset
                } else {
                    BLOCK_LENGTH
                };
                BlockPosition { offset, length }
            })
            .rev()
            .collect();
        Self {
            piece,
            piece_length: piece_size,
            participants: HashSet::new(),
            blocks,
            saved_amount: 0,
        }
    }

    pub fn is_filled(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.saved_amount == self.piece.len() as u8
    }

    /// Panics if piece is not full
    pub fn as_bytes(self) -> Vec<Bytes> {
        self.piece.into_iter().map(|x| x.unwrap()).collect()
    }

    pub fn pend_block(&mut self) -> Option<BlockPosition> {
        self.blocks.pop()
    }

    /// Position of the first None block Does not affect the block queue.
    pub fn pend_blocks_endgame(&self, take: usize) -> impl IntoIterator<Item = BlockPosition> + '_ {
        self.piece
            .iter()
            .enumerate()
            .filter_map(|(idx, x)| {
                if x.is_some() {
                    return None;
                }
                let offset = idx as u32 * BLOCK_LENGTH;
                let length = if idx == self.piece.len() - 1 {
                    self.piece_length - offset
                } else {
                    BLOCK_LENGTH
                };
                Some(BlockPosition { offset, length })
            })
            .take(take)
    }

    pub fn unpend_block(&mut self, block: BlockPosition) {
        self.blocks.push(block);
    }

    pub fn save_block(&mut self, data_block: DataBlock) -> anyhow::Result<()> {
        ensure!(data_block.offset + data_block.len() as u32 <= self.piece_length);

        let index = data_block.offset / BLOCK_LENGTH;
        let block = &mut self.piece[index as usize];
        if block.is_none() {
            *block = Some(data_block.block);
            self.saved_amount += 1;
        };
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct SchedulerPiece {
    pub rarity: u8,
    pub is_finished: bool,
    pub is_saving: bool,
    pub priority: Priority,
    pub pending_blocks: Option<PendingPieceV2>,
}

impl SchedulerPiece {
    pub fn can_schedule(&self) -> bool {
        self.priority != Priority::Disabled
            && !self.is_finished
            && self.pending_blocks.is_none()
            && !self.is_saving
    }
}

impl PartialEq for SchedulerPiece {
    fn eq(&self, other: &Self) -> bool {
        self.rarity == other.rarity
            && self.priority == other.priority
            && self.is_saving == other.is_saving
            && self.is_finished == other.is_finished
    }
}

impl Eq for SchedulerPiece {}

impl PartialOrd for SchedulerPiece {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SchedulerPiece {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.priority == other.priority {
            Reverse(self.rarity).cmp(&Reverse(other.rarity))
        } else {
            self.priority.cmp(&other.priority)
        }
    }
}

#[derive(Debug)]
pub struct Scheduler {
    piece_size: u32,
    /// Low latency mode
    stream_mode: bool,
    /// Full ut_metadata used to share it
    ut_metadata: UtMetadata,
    pub max_pending_pieces: usize,
    total_length: u64,
    pub peers: Vec<ActivePeer>,
    pub schedule_strategy: ScheduleStrategy,
    pub pending_files: PendingFiles,
    pub piece_table: Vec<SchedulerPiece>,
    pub pending_pieces: Vec<usize>,
    /// Must be reversed so we can pop with O(1)
    pub piece_queue: Vec<usize>,
    pub downloaded_pieces: usize,
}

pub const BLOCK_LENGTH: u32 = 16 * 1024;

impl Scheduler {
    pub fn new(t: Info, pending_files: PendingFiles) -> Self {
        let ut_metadata = UtMetadata::full_from_info(&t);
        let total_pieces = t.pieces.len();
        let mut piece_table = vec![SchedulerPiece::default(); total_pieces];
        for file in &pending_files.files {
            for p in file.pieces_range() {
                piece_table[p].priority = file.priority;
            }
        }
        let schedule_strategy = ScheduleStrategy::default();
        let piece_queue = schedule_strategy.create_piece_queue(&piece_table);
        Self {
            piece_size: t.piece_length,
            stream_mode: false,
            ut_metadata,
            total_length: t.total_size(),
            max_pending_pieces: 40,
            peers: Vec::new(),
            schedule_strategy,
            pending_files,
            piece_table,
            pending_pieces: Vec::new(),
            piece_queue,
            downloaded_pieces: 0,
        }
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        crate::utils::piece_size(piece_i, self.piece_size, self.total_length) as u32
    }

    fn schedule_next(&mut self) -> Option<usize> {
        let new_piece = self.piece_queue.pop();
        if let Some(new_piece) = new_piece {
            let piece_len = self.piece_length(new_piece);
            self.piece_table[new_piece].pending_blocks =
                Some(PendingPieceV2::new(piece_len as usize));
            self.pending_pieces.push(new_piece);
        }
        new_piece
    }

    /// Get next scheduled piece without scheduling it
    fn schedule_peek(&self) -> Option<usize> {
        self.piece_queue.last().copied()
    }

    pub fn save_block(&mut self, sender_idx: usize, data_block: DataBlock) {
        let piece = data_block.piece as usize;
        let scheduler_piece = &mut self.piece_table[piece];
        let Some(pending_blocks) = scheduler_piece.pending_blocks.as_mut() else {
            tracing::error!(
                "Peer sent block of piece that is not pending: {}",
                data_block
            );
            return;
        };

        let peer = &mut self.peers[sender_idx];
        peer.downloaded += data_block.block.len() as u64;
        let block = data_block.block();
        let global_location = BlockGlobalLocation::from_block(&block, self.piece_size);
        if !peer.pending_blocks.remove(&global_location) {
            tracing::error!("Peer sent block we did'n ask: {}", data_block);
        }
        if let Err(e) = pending_blocks.save_block(data_block) {
            // peer logic error
            tracing::error!("{e}");
        }
    }

    /// Schedules next batch for peer
    pub fn schedule(&mut self, peer_idx: usize, tick_duration: &std::time::Duration) {
        let peer = &mut self.peers[peer_idx];

        let performance_kb = peer.performance_history.avg_down_speed_sec(tick_duration) / 1024;
        // currently it is 32 Mb (2048 blocks) in pipeline if peer uploading 10MB/s
        let rate = if performance_kb < 20 {
            performance_kb + 2
        } else {
            performance_kb / 5 + 18
        };
        let schedule_amount = rate.saturating_sub(peer.pending_blocks.len());
        if schedule_amount == 0 {
            return;
        }

        let mut assigned = self.assign_n_blocks(schedule_amount, peer_idx);

        // ISSUE: Check wheather we couldn't assign piece to peer because no more blocks available, not because
        // he just doesn't have these pieces
        // If any peer have single piece available he will force scheduling pending_piece

        if assigned < schedule_amount {
            tracing::warn!(
                "Couldn't fulfill peer's rate {}/{}",
                assigned,
                schedule_amount
            );
            // Add more pending blocks to fulfill peer rate
            match self.schedule_next() {
                Some(_) => {}
                // If no more pieces in queue we can run endgame mode
                None => {
                    // this is the implementation of endgame mode.
                    // we are just taking the first None blocks we see
                    let peer = &self.peers[peer_idx];
                    for piece_idx in self
                        .pending_pieces
                        .iter()
                        .filter(|p| peer.bitfield.has(**p))
                    {
                        let pending_piece = &self.piece_table[*piece_idx];
                        let pending_blocks = pending_piece.pending_blocks.as_ref().unwrap();
                        for position in
                            pending_blocks.pend_blocks_endgame(schedule_amount - assigned)
                        {
                            let block = Block::from_position(*piece_idx as u32, position);
                            let global_location =
                                BlockGlobalLocation::from_block(&block, self.piece_size);
                            if !peer.pending_blocks.contains(&global_location) {
                                match peer.message_tx.try_send(PeerMessage::request(block)) {
                                    Ok(_) => {
                                        assigned += 1;
                                    }
                                    Err(e) => {
                                        tracing::error!("Send error: {e}");
                                        return;
                                    }
                                    Err(e) => {
                                        tracing::error!("Send error: {e}");
                                        return;
                                    }
                                }
                            }
                        }
                        if assigned == schedule_amount {
                            break;
                        }
                    }
                }
            };
        }
    }

    pub fn send_block_to_peer(&mut self, peer_id: &Uuid, block: Block, bytes: Bytes) {
        if let Some(idx) = self.get_peer_idx(peer_id) {
            let peer = &mut self.peers[idx];
            let start = block.offset as usize;
            let end = (block.offset + block.length) as usize;
            peer.message_tx
                .try_send(PeerMessage::Piece {
                    begin: block.offset,
                    index: block.piece,
                    block: bytes.slice(start..end),
                })
                .unwrap();
            peer.uploaded += block.length as u64;
        }
    }

    /// Add and announce piece to everyone
    pub fn add_piece(&mut self, piece: usize) {
        self.piece_table[piece].is_finished = true;
        self.piece_table[piece].is_saving = false;
        self.downloaded_pieces += 1;
        for peer in &mut self.peers {
            if peer.bitfield.has(piece) {
                peer.interested_pieces -= 1;
            }
            let _ = peer.message_tx.try_send(PeerMessage::Have {
                index: piece as u32,
            });
        }
    }

    /// Handle failed piece save
    pub fn fail_piece(&mut self, piece_idx: usize) {
        tracing::warn!("Failed to save piece {piece_idx}");
        let piece_len = self.piece_length(piece_idx);
        let piece = &mut self.piece_table[piece_idx];
        piece.is_saving = false;
        piece.pending_blocks = Some(PendingPieceV2::new(piece_len as usize));
        self.piece_queue.push(piece_idx);
    }

    pub fn handle_peer_choke(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        for pending_block in peer.pending_blocks.iter() {
            let pending_block = pending_block.block(self.piece_size, self.total_length);
            let piece_idx = pending_block.piece as usize;
            if let Some(pending_blocks) = &mut self.piece_table[piece_idx].pending_blocks {
                pending_blocks.unpend_block(pending_block.position());
            }
        }
        peer.pending_blocks.clear();
        peer.in_status.set_choke(true);
    }

    pub fn handle_peer_unchoke(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        peer.in_status.set_choke(false);
    }

    pub fn handle_peer_have_msg(&mut self, peer_idx: usize, piece: usize) {
        if piece >= self.piece_table.len() {
            tracing::warn!("peer have piece out of bounds");
            return;
        }
        let peer = &mut self.peers[peer_idx];
        if peer.bitfield.has(piece) {
            tracing::warn!("peer sending have message with piece that is already in his bitfield");
            // logic error
        }
        peer.bitfield.add(piece).expect("bounds checked above");
        let piece = &mut self.piece_table[piece];
        piece.rarity += 1;
        if !piece.is_finished && piece.priority != Priority::Disabled {
            peer.interested_pieces += 1;
        }
    }

    pub fn handle_peer_interest(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        peer.in_status.set_interest(true);
    }

    pub fn handle_peer_uninterest(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        peer.in_status.set_interest(false);
    }

    pub fn handle_peer_extension(
        &mut self,
        peer_idx: usize,
        ext_id: u8,
        payload: Bytes,
    ) -> Result<(), PeerLogicError> {
        let peer = &mut self.peers[peer_idx];
        match ext_id {
            PexMessage::CLIENT_ID => {
                let pex_message = PexMessage::from_bytes(&payload).context("parse pex message")?;
            }
            UtMessage::CLIENT_ID => {
                let ut_message =
                    UtMessage::from_bytes(&payload).context("parse ut_metadata message")?;
                match ut_message {
                    UtMessage::Request { piece } => {
                        if let Some(block) = self.ut_metadata.get_piece(piece) {
                            peer.send_ut_metadata_block(
                                UtMessage::Data {
                                    piece,
                                    total_size: self.ut_metadata.size,
                                },
                                block,
                            )?
                        } else {
                            peer.send_extension_message(UtMessage::Reject { piece })?
                        };
                    }
                    _ => {}
                }
            }
            _ => {
                // unknown extension
            }
        }
        Ok(())
    }

    pub fn remove_peer(&mut self, peer_idx: usize) -> Option<std::net::SocketAddr> {
        let peer = self.peers.swap_remove(peer_idx);
        for pending_block in peer.pending_blocks.into_iter() {
            let pending_block = pending_block.block(self.piece_size, self.total_length);
            let piece_idx = pending_block.piece as usize;
            if let Some(pending_blocks) = &mut self.piece_table[piece_idx].pending_blocks {
                // BUG: this will break during endgame mode!
                pending_blocks.unpend_block(pending_block.position());
            }
        }
        for piece in peer.bitfield.pieces() {
            self.piece_table[piece].rarity -= 1;
        }
        Some(peer.ip)
    }

    pub fn add_peer(&mut self, mut peer: ActivePeer) {
        let interseted_pieces = self.calculate_interested_amount(&peer);
        peer.interested_pieces = interseted_pieces;
        if interseted_pieces > 0 {
            peer.set_out_choke(false).unwrap();
            peer.set_out_interset(true).unwrap();
        }
        for piece in peer.bitfield.pieces() {
            self.piece_table[piece].rarity += 1;
        }
        self.peers.push(peer);
    }

    pub async fn start(&mut self) {
        for _ in 0..self.max_pending_pieces {
            self.schedule_next();
        }
        tracing::info!("Started scheduler");
    }

    pub fn register_performance(&mut self) {
        for peer in self.peers.iter_mut() {
            let newest_performance = Performance::new(peer.downloaded, peer.uploaded);
            peer.performance_history.update(newest_performance);
        }
    }

    pub fn calculate_interested_amount(&self, peer: &ActivePeer) -> usize {
        self.piece_table
            .iter()
            .enumerate()
            .filter(|(i, p)| {
                p.priority != Priority::Disabled && !p.is_finished && peer.bitfield.has(*i)
            })
            .count()
    }

    pub fn change_file_priority(&mut self, idx: usize, new_priority: Priority) {
        if let Some((old, file)) = self.pending_files.change_file_priority(idx, new_priority) {
            let is_disabled = !old.is_disabled() && new_priority.is_disabled();
            for piece in file.pieces_range() {
                self.piece_table[piece].priority = file.priority;
            }
            if is_disabled {
                self.pending_pieces.retain(|p| {
                    let disabled = !file.pieces_range().contains(p);
                    if disabled {
                        self.piece_table[*p].pending_blocks = None;
                    }
                    disabled
                });
            }

            // TODO: rebuild piece queue
        };
    }

    fn assign_n_blocks(&mut self, take: usize, peer_idx: usize) -> usize {
        let mut took = 0;
        let peer = &mut self.peers[peer_idx];
        for i in self
            .pending_pieces
            .iter()
            .filter(|p| peer.bitfield.has(**p))
        {
            let blocks = self.piece_table[*i].pending_blocks.as_mut().unwrap();
            while let Some(position) = blocks.pend_block() {
                let block = Block::from_position(*i as u32, position);
                match peer.message_tx.try_send(PeerMessage::request(block)) {
                    Ok(_) => {
                        took += 1;
                        let global_position =
                            BlockGlobalLocation::from_block(&block, self.piece_size);
                        peer.pending_blocks.insert(global_position);
                        if took == take {
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Send error: {e}");
                        blocks.unpend_block(position);
                        return took;
                    }
                }
            }
            if took == take {
                break;
            }
        }
        took
    }

    pub fn get_peer_idx(&self, peer_id: &Uuid) -> Option<usize> {
        self.peers.iter().position(|p| p.id == *peer_id)
    }

    pub fn is_torrent_finished(&self) -> bool {
        self.pending_pieces.is_empty()
    }

    /// Get progress percent and the amount of pending_pieces
    pub fn percent_pending_pieces(&self) -> (f64, usize) {
        let downloaded_pieces = self.downloaded_pieces;
        let total_pieces = self.piece_queue.len() + self.pending_pieces.len() + downloaded_pieces;
        let pending_pieces = self.pending_pieces.len();
        (
            downloaded_pieces as f64 / total_pieces as f64 * 100.,
            pending_pieces,
        )
    }

    pub fn total_pieces(&self) -> usize {
        self.piece_table.len()
    }
}
