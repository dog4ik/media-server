use std::{cmp::Reverse, collections::HashSet, fmt::Display, ops::Range};

use anyhow::anyhow;
use bytes::{BufMut, Bytes, BytesMut};
use uuid::Uuid;

use crate::{
    download::{ActivePeer, Block, PeerCommand, Performance},
    protocol::{Hashes, Info, OutputFile},
};

#[derive(Debug, Clone, Copy, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    Disabled = 0,
    Low = 1,
    #[default]
    Medium = 2,
    High = 3,
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
    pub files: Vec<PendingFile>,
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

    pub fn change_file_priority(
        &mut self,
        idx: usize,
        new_priority: Priority,
    ) -> Option<PendingFile> {
        if let Some(file) = self.files.iter_mut().find(|f| f.index == idx) {
            if file.priority != new_priority {
                file.priority = new_priority;
                let file_copy = *file;
                self.files.sort_unstable_by_key(|x| x.priority);
                return Some(file_copy);
            }
            return None;
        };
        tracing::warn!(
            "File index is out of bounds, got {idx} expected < {}",
            self.files.len()
        );
        None
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
    /// Allow expanding pending blocks to satifsfy scheduler
    pub fn allow_expansion(&self) -> bool {
        match self {
            ScheduleStrategy::PieceRequest { .. } => false,
            _ => true,
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
pub struct PendingBlock {
    pub offset: u32,
    pub length: u32,
    pub bytes: Option<Bytes>,
}

impl PendingBlock {
    pub fn new(offset: u32, length: u32) -> Self {
        Self {
            offset,
            length,
            bytes: None,
        }
    }

    pub fn as_block(&self, piece_i: u32) -> Block {
        Block {
            piece: piece_i,
            offset: self.offset,
            length: self.length,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingBlocks {
    pub piece_len: u32,
    pub participants: HashSet<Uuid>,
    pub blocks: Vec<PendingBlock>,
}

impl PendingBlocks {
    pub fn new(piece_len: u32) -> Self {
        Self {
            piece_len,
            participants: HashSet::new(),
            blocks: Vec::new(),
        }
    }

    pub fn pending_block_mut(&mut self, offset: u32) -> Option<&mut PendingBlock> {
        let index = offset / BLOCK_LENGTH;
        self.blocks.get_mut(index as usize)
    }

    pub fn is_full(&self) -> bool {
        self.is_filled() && self.blocks.iter().all(|b| b.bytes.is_some())
    }

    pub fn is_filled(&self) -> bool {
        (self.piece_len + BLOCK_LENGTH - 1) / BLOCK_LENGTH == self.blocks.len() as u32
    }

    pub fn as_bytes(mut self) -> Bytes {
        self.blocks
            .drain(..)
            .fold(
                BytesMut::with_capacity(self.piece_len as usize),
                |mut acc, block| {
                    acc.put(&mut block.bytes.expect("block to be full"));
                    acc
                },
            )
            .into()
    }

    pub fn pend_block(&mut self, piece_i: u32) -> Option<Block> {
        let insert_offset = self.blocks.last().map(|x| x.length + x.offset).unwrap_or(0);
        let length = std::cmp::min(self.piece_len - insert_offset, BLOCK_LENGTH);
        if length == 0 {
            return None;
        }

        self.blocks.push(PendingBlock::new(insert_offset, length));
        let block = Block {
            piece: piece_i,
            offset: insert_offset,
            length,
        };
        Some(block)
    }
}

#[derive(Debug, Clone)]
pub struct SchedulerPiece {
    pub rarity: u8,
    pub is_finished: bool,
    pub is_saving: bool,
    pub priority: Priority,
    pub pending_blocks: Option<PendingBlocks>,
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
    piece_size: usize,
    pub max_pending_pieces: usize,
    total_length: u64,
    pieces: Hashes,
    failed_blocks: Vec<Block>,
    pub peers: Vec<ActivePeer>,
    pub schedule_strategy: ScheduleStrategy,
    pub pending_files: PendingFiles,
    pub piece_table: Vec<SchedulerPiece>,
}

const BLOCK_LENGTH: u32 = 16 * 1024;

impl Scheduler {
    pub fn new(t: Info, pending_files: PendingFiles) -> Self {
        let total_pieces = t.pieces.len();
        let mut piece_table = vec![
            SchedulerPiece {
                rarity: 0,
                priority: Priority::default(),
                is_finished: false,
                is_saving: false,
                pending_blocks: None,
            };
            total_pieces
        ];
        for file in &pending_files.files {
            for p in file.pieces_range() {
                piece_table[p].priority = file.priority;
            }
        }
        Self {
            piece_size: t.piece_length as usize,
            total_length: t.total_size(),
            pieces: t.pieces,
            failed_blocks: Vec::new(),
            max_pending_pieces: 40,
            peers: Vec::new(),
            schedule_strategy: ScheduleStrategy::default(),
            pending_files,
            piece_table,
        }
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        crate::utils::piece_size(piece_i, self.piece_size, self.total_length as usize) as u32
    }

    fn can_schedule_piece(&self, i: usize) -> bool {
        self.piece_table[i].can_schedule()
    }

    /// Schedules next piece linearly (the next missing piece from start)
    /// returning `None` if no more pieces left
    fn linear_next(&self) -> Option<usize> {
        self.piece_table
            .iter()
            .enumerate()
            .find(|(_, x)| x.can_schedule())
            .map(|(i, _)| i)
    }

    fn rare_first_next(&self) -> Option<usize> {
        todo!()
    }

    fn request_piece_next(&self, piece: usize) -> Option<usize> {
        for i in piece..self.pieces.len() {
            if self.can_schedule_piece(i) {
                tracing::debug!("Assigning next request({piece}) piece {i}");
                return Some(i);
            }
        }
        for i in 0..piece {
            if self.can_schedule_piece(i) {
                tracing::debug!("Assigning fallback request({piece}) piece {i}");
                return Some(i);
            }
        }
        None
    }

    fn schedule_next(&mut self) -> Option<usize> {
        let new_piece = match &self.schedule_strategy {
            ScheduleStrategy::Linear => self.linear_next(),
            ScheduleStrategy::RareFirst => self.rare_first_next(),
            ScheduleStrategy::PieceRequest { piece } => self.request_piece_next(*piece),
        };
        if let Some(new_piece) = new_piece {
            let piece_len = self.piece_length(new_piece);
            self.piece_table[new_piece].pending_blocks = Some(PendingBlocks::new(piece_len));
        }
        new_piece
    }

    pub fn save_blocks(
        &mut self,
        sender_idx: usize,
        blocks: Vec<(Block, Bytes)>,
    ) -> Vec<(usize, Bytes)> {
        let mut full_pieces = Vec::new();
        for (block, data) in blocks {
            let piece = block.piece as usize;
            let pending_piece = &mut self.piece_table[piece];
            let Some(pending_blocks) = pending_piece.pending_blocks.as_mut() else {
                continue;
            };

            if let Some(block) = pending_blocks.pending_block_mut(block.offset) {
                block.bytes = Some(data);
            }
            if pending_blocks.is_full() {
                let blocks = pending_piece.pending_blocks.take().unwrap();
                pending_piece.is_saving = true;
                let bytes = blocks.as_bytes();
                if self.max_pending_pieces > self.count_pending_pieces() {
                    _ = self.schedule_next();
                }
                full_pieces.push((piece, bytes));
            }
            self.peers[sender_idx].downloaded += block.length as u64;
        }
        full_pieces
    }

    pub fn count_pending_pieces(&self) -> usize {
        self.piece_table
            .iter()
            .filter(|x| x.pending_blocks.is_some())
            .count()
    }

    /// Schedules next batch for peer
    pub fn schedule(&mut self, peer_idx: usize, pending_blocks_amount: usize) {
        let available_pieces = self.available_pieces();
        let peer = &mut self.peers[peer_idx];

        let performance_kb = peer.performance_history.avg_down_speed() as usize / 1024;
        let rate = if performance_kb < 20 {
            performance_kb + 2
        } else {
            performance_kb / 5 + 18
        };
        let schedule_amount = rate as isize - pending_blocks_amount as isize;
        if schedule_amount <= 0 {
            return;
        }
        let schedule_amount = schedule_amount as usize;

        let mut assigned_blocks = Vec::with_capacity(schedule_amount);
        for block in self
            .failed_blocks
            .iter()
            .filter(|b| peer.bitfield.has(b.piece as usize))
            .take(schedule_amount - assigned_blocks.len())
        {
            assigned_blocks.push(*block);
        }
        self.failed_blocks.retain(|x| !assigned_blocks.contains(x));

        if assigned_blocks.len() < schedule_amount {
            for piece in available_pieces
                .into_iter()
                .filter(|p| peer.bitfield.has(*p))
            {
                let pending_blocks = self.piece_table[piece].pending_blocks.as_mut().unwrap();
                while let Some(new_block) = pending_blocks.pend_block(piece as u32) {
                    assigned_blocks.push(new_block);
                    if assigned_blocks.len() == schedule_amount {
                        break;
                    }
                }
                if assigned_blocks.len() == schedule_amount {
                    break;
                }
            }
        }

        if assigned_blocks.len() < schedule_amount {
            tracing::trace!(
                "Couldn't fulfill peer's rate {}/{}",
                assigned_blocks.len(),
                schedule_amount
            );
            // Add more pending blocks to fulfil peer rate if strategy is not piece request
            match self.schedule_strategy {
                ScheduleStrategy::PieceRequest { .. } => {
                    // self.steal_work(peer_idx, schedule_amount - assigned_blocks.len());
                }
                _ => {
                    if self.schedule_next().is_none() {
                        let available_pieces: Vec<_> = self
                            .piece_table
                            .iter()
                            .enumerate()
                            .filter(|p| p.1.pending_blocks.is_some())
                            .map(|p| p.0)
                            .collect();
                        // self.steal_work(peer_idx, schedule_amount - assigned_blocks.len());
                        for piece in available_pieces {
                            let pending_blocks =
                                self.piece_table[piece].pending_blocks.as_ref().unwrap();
                            let mut avialbale_blocks: Vec<_> = pending_blocks
                                .blocks
                                .iter()
                                .filter_map(|b| {
                                    let block = b.as_block(piece as u32);
                                    if b.bytes.is_none()
                                        && !assigned_blocks.contains(&b.as_block(piece as u32))
                                    {
                                        Some(block)
                                    } else {
                                        None
                                    }
                                })
                                .take(schedule_amount - assigned_blocks.len())
                                .collect();
                            assigned_blocks.append(&mut avialbale_blocks);
                            if assigned_blocks.len() == schedule_amount {
                                break;
                            }
                        }
                    };
                }
            }
        }

        if !assigned_blocks.is_empty() {
            let peer = &mut self.peers[peer_idx];
            if peer
                .command
                .try_send(PeerCommand::StartMany {
                    blocks: assigned_blocks.clone(),
                })
                .is_err()
            {
                self.failed_blocks.extend(assigned_blocks.iter());
            };
        }
    }

    /// Steal blocks from less productive peers
    pub fn steal_work(&mut self, thief_idx: usize, amount: usize) {
        let thief = &self.peers[thief_idx];
        let thief_performance = thief.performance_history.avg_down_speed();

        let mut worse_peers: Vec<_> = self
            .peers
            .iter_mut()
            .enumerate()
            .filter(|(idx, p)| {
                !p.in_status.is_choked()
                    && *idx != thief_idx
                    && p.performance_history.avg_down_speed() < thief_performance
            })
            .collect();

        worse_peers.sort_by_key(|(_, p)| p.performance_history.avg_down_speed());
        for (piece_i, blocks) in self
            .piece_table
            .iter()
            .enumerate()
            .filter_map(|p| Some((p.0, p.1.pending_blocks.as_ref()?)))
        {}
    }

    pub async fn send_block_to_peer(&mut self, peer_id: &Uuid, block: Block, bytes: Bytes) {
        if let Some(idx) = self.get_peer_idx(peer_id) {
            let peer = &mut self.peers[idx];
            let start = block.offset as usize;
            let end = (block.offset + block.length) as usize;
            peer.command
                .send(PeerCommand::Block {
                    block,
                    data: bytes.slice(start..end),
                })
                .await
                .unwrap();
            peer.uploaded += block.length as u64;
        }
    }

    /// Add and announce piece to everyone
    pub fn add_piece(&mut self, piece: usize) {
        self.piece_table[piece].is_finished = true;
        self.piece_table[piece].is_saving = false;
        for peer in &mut self.peers {
            if peer.bitfield.has(piece) {
                peer.interested_pieces -= 1;
            }
            let _ = peer.command.try_send(PeerCommand::Have {
                piece: piece as u32,
            });
        }
    }

    pub fn handle_peer_choke(&mut self, peer_idx: usize, mut peer_blocks: Vec<Block>) {
        let peer = &mut self.peers[peer_idx];
        self.failed_blocks.append(&mut peer_blocks);
        peer.in_status.choke();
    }

    pub fn handle_peer_unchoke(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        peer.in_status.unchoke();
        if peer.out_status.is_interested() {
            self.schedule(peer_idx, 0);
        }
    }

    pub fn handle_peer_interest(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        peer.in_status.interest();
    }

    pub fn handle_peer_uninterest(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        peer.in_status.uninterest();
    }

    pub fn remove_peer(
        &mut self,
        peer_idx: usize,
        mut peer_blocks: Vec<Block>,
    ) -> Option<ActivePeer> {
        let peer = self.peers.swap_remove(peer_idx);
        self.failed_blocks.append(&mut peer_blocks);
        for piece in peer.bitfield.pieces() {
            self.piece_table[piece].rarity -= 1;
        }
        Some(peer)
    }

    pub fn choke_peer(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        peer.command.try_send(PeerCommand::Choke).unwrap();
        peer.out_status.choke();
    }

    pub fn add_peer(&mut self, mut peer: ActivePeer) {
        peer.out_status.unchoke();
        peer.command.try_send(PeerCommand::Unchoke).unwrap();
        let interested_amount = self.calculate_interested_amount(&peer);
        peer.interested_pieces = interested_amount;
        if interested_amount > 0 {
            peer.out_status.interest();
            peer.command.try_send(PeerCommand::Interested).unwrap();
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
        let interested_amount = self
            .piece_table
            .iter()
            .enumerate()
            .filter(|(i, p)| {
                p.priority != Priority::Disabled && !p.is_finished && peer.bitfield.has(*i)
            })
            .count();
        interested_amount
    }

    pub fn change_file_priority(&mut self, idx: usize, new_priority: Priority) {
        self.pending_files.change_file_priority(idx, new_priority);
        for file in &self.pending_files.files {
            for p in file.pieces_range() {
                self.piece_table[p].priority = file.priority;
            }
        }
    }

    /// Pending pieces that are not filled
    fn available_pieces(&self) -> Vec<usize> {
        self.piece_table
            .iter()
            .enumerate()
            .filter_map(|(i, piece)| {
                piece
                    .pending_blocks
                    .as_ref()
                    .and_then(|p| (!p.is_filled()).then_some(i))
            })
            .collect()
    }

    pub fn get_peer_idx(&self, peer_id: &Uuid) -> Option<usize> {
        self.peers.iter().position(|p| p.id == *peer_id)
    }

    pub fn is_torrent_finished(&self) -> bool {
        self.piece_table
            .iter()
            .all(|p| p.is_finished || p.priority == Priority::Disabled)
    }

    /// Get progress percent and the amount of pending_pieces
    pub fn percent_pending_pieces(&self) -> (f64, usize) {
        let mut total_pieces = 0;
        let mut downloaded_pieces = 0;
        let mut pending_pieces = 0;
        for piece in self
            .piece_table
            .iter()
            .filter(|p| p.priority != Priority::Disabled)
        {
            if piece.pending_blocks.is_some() {
                pending_pieces += 1;
            }
            total_pieces += 1;
            if piece.is_finished {
                downloaded_pieces += 1;
            }
        }
        (
            downloaded_pieces as f64 / total_pieces as f64 * 100.,
            pending_pieces,
        )
    }

    pub fn total_pieces(&self) -> usize {
        self.pieces.len()
    }
}
