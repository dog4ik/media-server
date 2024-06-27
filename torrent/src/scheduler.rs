use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
    ops::Range,
    time::Instant,
};

use anyhow::anyhow;
use bytes::{BufMut, Bytes, BytesMut};
use uuid::Uuid;

use crate::{
    download::{ActivePeer, Block, PeerCommand, Performance},
    peers::BitField,
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

#[derive(Debug, Clone, Copy)]
pub struct PendingFile {
    pub priority: Priority,
    pub index: usize,
    pub start_piece: usize,
    pub end_piece: usize,
}

#[derive(Debug, Clone)]
pub struct PendingFiles {
    pub files: Vec<PendingFile>,
}

impl PendingFiles {
    pub fn from_output_files(
        piece_length: u32,
        output_files: &Vec<OutputFile>,
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
        Self { files }
    }

    pub fn change_file_priority(&mut self, idx: usize, new_priority: Priority) {
        if let Some(file) = self.files.iter_mut().find(|f| f.index == idx) {
            if file.priority != new_priority {
                file.priority = new_priority;
                self.files.sort_unstable_by_key(|x| x.priority);
            }
        };
    }

    // Iterate over file pieces with repect to their priorities
    pub fn piece_iterator(&self) -> impl Iterator<Item = usize> + '_ {
        self.files
            .iter()
            .rev()
            .filter_map(|file| {
                if Priority::Disabled == file.priority {
                    return None;
                } else {
                    Some(file.start_piece..=file.end_piece)
                }
            })
            .flatten()
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
    Ranges {
        ranges: Vec<Range<u64>>,
    },
}

impl Display for ScheduleStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduleStrategy::Linear => write!(f, "Linear"),
            ScheduleStrategy::RareFirst => write!(f, "Rare first"),
            ScheduleStrategy::PieceRequest { piece } => write!(f, "Piece request: {}", piece),
            ScheduleStrategy::Ranges { ranges } => {
                write!(f, "Ranged ")?;
                if ranges.len() > 10 {
                    write!(f, "with {} ranges", ranges.len())
                } else {
                    write!(f, "{:?}", ranges)
                }
            }
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
pub struct PendingPiece {
    pub piece_i: usize,
    pub created_at: Instant,
    pub piece_len: u32,
    pub blocks: Vec<PendingBlock>,
}

impl PendingPiece {
    pub fn new(piece_i: usize, piece_len: u32) -> Self {
        Self {
            piece_i,
            created_at: Instant::now(),
            piece_len,
            blocks: Vec::new(),
        }
    }

    pub fn pending_block_mut(&mut self, offset: u32, length: u32) -> Option<&mut PendingBlock> {
        self.blocks
            .iter_mut()
            .find(|b| b.offset == offset && b.length == length && b.bytes.is_none())
    }

    pub fn is_full(&self) -> bool {
        let mut total_len = 0;
        for block in &self.blocks {
            let Some(bytes) = &block.bytes else {
                return false;
            };
            assert_eq!(block.length as usize, bytes.len());
            total_len += block.length;
        }
        total_len == self.piece_len
    }

    pub fn is_filled(&self) -> bool {
        let mut total_len = 0;
        for block in &self.blocks {
            total_len += block.length;
        }
        total_len == self.piece_len
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

    pub fn pend_block(&mut self) -> Option<Block> {
        let insert_offset = self.blocks.last().map(|x| x.length + x.offset).unwrap_or(0);
        let length = std::cmp::min(self.piece_len - insert_offset, BLOCK_LENGTH);
        if length == 0 {
            return None;
        }

        self.blocks.push(PendingBlock::new(insert_offset, length));
        let block = Block {
            piece: self.piece_i as u32,
            offset: insert_offset,
            length,
        };
        Some(block)
    }
}

#[derive(Debug)]
pub struct Scheduler {
    piece_size: usize,
    pub max_pending_pieces: usize,
    total_length: u64,
    pieces: Hashes,
    failed_blocks: Vec<Block>,
    pub bitfield: BitField,
    pub pending_pieces: HashMap<usize, PendingPiece>,
    pub pending_saved_pieces: HashSet<usize>,
    pub peers: Vec<ActivePeer>,
    pub schedule_stategy: ScheduleStrategy,
    pub pending_files: PendingFiles,
}

const BLOCK_LENGTH: u32 = 16 * 1024;

impl Scheduler {
    pub fn new(t: Info, pending_files: PendingFiles) -> Self {
        let total_pieces = t.pieces.len();
        let bitfield = BitField::empty(total_pieces);
        Self {
            piece_size: t.piece_length as usize,
            total_length: t.total_size(),
            pieces: t.pieces,
            pending_pieces: HashMap::new(),
            failed_blocks: Vec::new(),
            max_pending_pieces: 40,
            peers: Vec::new(),
            pending_saved_pieces: HashSet::new(),
            bitfield,
            schedule_stategy: ScheduleStrategy::default(),
            pending_files,
        }
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        crate::utils::piece_size(piece_i, self.piece_size, self.total_length as usize) as u32
    }

    fn can_scheudle_piece(&self, i: usize) -> bool {
        !self.bitfield.has(i)
            && !self.pending_pieces.contains_key(&i)
            && !self.pending_saved_pieces.contains(&i)
    }

    /// Schedules next piece linearly (the next missing piece from start)
    /// returing `None` if no more pieces left
    fn linear_next(&self) -> Option<usize> {
        for i in self.pending_files.piece_iterator() {
            if self.can_scheudle_piece(i) {
                tracing::debug!("Assigning next linear piece {i}");
                return Some(i);
            }
        }
        None
    }

    fn rare_first_next(&self) -> Option<usize> {
        todo!()
    }

    fn request_piece_next(&self, piece: usize) -> Option<usize> {
        for i in piece..self.pieces.len() {
            if self.can_scheudle_piece(i) {
                tracing::debug!("Assigning next request({piece}) piece {i}");
                return Some(i);
            }
        }
        for i in 0..piece {
            if self.can_scheudle_piece(i) {
                tracing::debug!("Assigning fallback request({piece}) piece {i}");
                return Some(i);
            }
        }
        None
    }

    fn ranged_next(&self, ranges: &Vec<Range<u64>>) -> Option<usize> {
        let offset_piece = |offset: usize| offset / self.piece_size;
        for range in ranges {
            if range.is_empty() {
                tracing::error!("Encountered empty range: {:?}", range);
            }
            let start_piece = offset_piece(range.start as usize);
            let end_piece = offset_piece(range.end as usize - 1);
            for piece in start_piece..=end_piece {
                if self.can_scheudle_piece(piece) {
                    tracing::debug!("Assigned ranged({:?}) piece: {piece}", range);
                    return Some(piece);
                }
            }
        }
        None
    }

    fn schedule_next(&mut self) -> Option<usize> {
        let new_piece = match &self.schedule_stategy {
            ScheduleStrategy::Linear => self.linear_next(),
            ScheduleStrategy::RareFirst => self.rare_first_next(),
            ScheduleStrategy::PieceRequest { piece } => self.request_piece_next(*piece),
            ScheduleStrategy::Ranges { ranges } => self.ranged_next(ranges),
        };
        if let Some(new_piece) = new_piece {
            let piece_len = self.piece_length(new_piece) as u32;
            self.pending_pieces
                .insert(new_piece, PendingPiece::new(new_piece, piece_len));
        }
        new_piece
    }

    /// Save block
    pub async fn save_block(
        &mut self,
        sender_idx: usize,
        insert_block: Block,
        data: Bytes,
    ) -> anyhow::Result<Option<(usize, Bytes)>> {
        let sender = &mut self.peers[sender_idx];
        let sender_id = sender.id;
        if sender.in_status.is_choked() {
            tracing::warn!("Choked peer ({}) is sending blocks", sender_id);
        }
        if let Some(idx) = sender
            .pending_blocks
            .iter()
            .position(|b| *b == insert_block)
        {
            sender.pending_blocks.swap_remove(idx);
        } else {
            tracing::error!("Peer cant find his own block: {}", insert_block);
        }

        let pending_piece = self
            .pending_pieces
            .get_mut(&(insert_block.piece as usize))
            .ok_or(anyhow!("pending piece {} is not found", insert_block.piece))?;

        if let Some(block) =
            pending_piece.pending_block_mut(insert_block.offset, insert_block.length)
        {
            block.bytes = Some(data);
        }

        sender.downloaded += insert_block.length as u64;

        if pending_piece.is_full() {
            let piece = self
                .pending_pieces
                .remove(&(insert_block.piece as usize))
                .unwrap();
            self.pending_saved_pieces.insert(piece.piece_i);
            let bytes = piece.as_bytes();
            if self.max_pending_pieces > self.pending_pieces.len() {
                _ = self.schedule_next();
            }
            return Ok(Some((insert_block.piece as usize, bytes)));
        }
        Ok(None)
    }

    /// Schedules next batch for peer
    pub fn schedule(&mut self, peer_idx: usize) {
        let available_pieces = self.available_pieces();
        let peer = &mut self.peers[peer_idx];
        if !peer.out_status.is_interested() {
            peer.out_status.interest();
            peer.command.try_send(PeerCommand::Interested).unwrap();
        }
        let performance_kb = peer.performance_history.avg_speed() as usize / 1024;
        let rate = if performance_kb < 20 {
            performance_kb + 2
        } else {
            performance_kb / 5 + 18
        };
        let schedule_amount = rate as isize - peer.pending_blocks.len() as isize;
        if schedule_amount <= 0 {
            return;
        }
        let schedule_amount = schedule_amount as usize;
        let mut assigned_blocks = Vec::with_capacity(schedule_amount);
        for block in self.failed_blocks.iter() {
            if peer.bitfield.has(block.piece as usize) {
                assigned_blocks.push(*block);
                if assigned_blocks.len() == schedule_amount {
                    break;
                }
            };
        }
        self.failed_blocks.retain(|x| !assigned_blocks.contains(&x));

        if assigned_blocks.len() < schedule_amount {
            for piece in available_pieces.iter().copied() {
                if peer.bitfield.has(piece) {
                    let pending_piece = self.pending_pieces.get_mut(&piece).unwrap();
                    while let Some(new_block) = pending_piece.pend_block() {
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
        }

        if assigned_blocks.len() < schedule_amount {
            tracing::trace!(
                "Couldn't fulfill peer's rate {}/{}",
                assigned_blocks.len(),
                schedule_amount
            );
            // Add more pending blocks to fulfil peer rate if strategy is not piece request
            match self.schedule_stategy {
                ScheduleStrategy::PieceRequest { .. } => {
                    let stolen_blocks =
                        self.steal_work(peer_idx, schedule_amount - assigned_blocks.len());
                    assigned_blocks.extend(stolen_blocks);
                }
                _ => {
                    if self.schedule_next().is_none() {
                        let stolen_blocks =
                            self.steal_work(peer_idx, schedule_amount - assigned_blocks.len());
                        assigned_blocks.extend(stolen_blocks);
                    };
                }
            }
        }

        let peer = &mut self.peers[peer_idx];
        for block in assigned_blocks {
            if let Ok(_) = peer.command.try_send(PeerCommand::Start { block }) {
                peer.pending_blocks.push(block);
            } else {
                self.failed_blocks.push(block);
            };
        }
    }

    pub fn best_peer(&self) -> Option<usize> {
        let mut max = 0;
        let mut best_peer = None;
        for (i, peer) in self.peers.iter().enumerate() {
            let performance = peer.performance_history.avg_speed();
            if performance > max {
                best_peer = Some(i);
                max = performance;
            }
        }
        best_peer
    }

    /// Steal blocks from less productive peers
    pub fn steal_work(&mut self, thief_idx: usize, amount: usize) -> Vec<Block> {
        let mut out = Vec::with_capacity(amount);
        let thief = &self.peers[thief_idx];
        let thief_performance = thief.performance_history.avg_speed();
        let thief_pending_blocks = thief.pending_blocks.clone();
        let thief_bf = thief.bitfield.clone();

        let mut worse_peers: Vec<_> = self
            .peers
            .iter_mut()
            .enumerate()
            .filter(|(idx, p)| {
                !p.pending_blocks.is_empty()
                    && *idx != thief_idx
                    && p.performance_history.avg_speed() < thief_performance
            })
            .collect();

        worse_peers.sort_by_key(|(_, p)| p.performance_history.avg_speed());

        for (_, peer) in worse_peers {
            for block in peer
                .pending_blocks
                .iter()
                .filter(|b| thief_bf.has(b.piece as usize) && !thief_pending_blocks.contains(b))
                .rev()
                .take(amount - out.len())
            {
                if out.len() < amount {
                    out.push(*block);
                } else {
                    panic!("Took more blocks({}) then expected({})", out.len(), amount);
                }
            }
            if out.len() == amount {
                break;
            }
        }
        out
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
        self.bitfield.add(piece).unwrap();
        for peer in &self.peers {
            let _ = peer.command.try_send(PeerCommand::Have {
                piece: piece as u32,
            });
        }
    }

    pub fn remove_piece(&mut self, piece: usize) {
        self.bitfield.remove(piece).unwrap();
        self.schedule_next()
            .expect("removed piece to be rescheduled");
    }

    pub async fn handle_peer_choke(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        self.failed_blocks.extend(peer.pending_blocks.drain(..));
        peer.in_status.choke();
        self.schedule_all();
    }

    pub async fn handle_peer_unchoke(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        peer.in_status.unchoke();
        self.schedule(peer_idx);
    }

    pub fn remove_peer(&mut self, peer_idx: usize) -> Option<ActivePeer> {
        let mut peer = self.peers.swap_remove(peer_idx);
        self.failed_blocks.extend(peer.pending_blocks.drain(..));
        self.schedule_all();
        Some(peer)
    }

    pub fn choke_peer(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        self.failed_blocks.extend(peer.pending_blocks.drain(..));
        peer.out_status.choke();
        self.schedule_all();
    }

    pub fn schedule_all(&mut self) {
        let idxs: Vec<_> = self
            .peers
            .iter()
            .enumerate()
            .filter(|(_, x)| !x.in_status.is_choked())
            .map(|x| x.0)
            .collect();
        for idx in idxs {
            self.schedule(idx);
        }
    }

    pub fn add_peer(&mut self, mut peer: ActivePeer) {
        peer.out_status.unchoke();
        peer.out_status.interest();
        peer.command.try_send(PeerCommand::Unchoke).unwrap();
        peer.command.try_send(PeerCommand::Interested).unwrap();
        self.peers.push(peer);
    }

    pub async fn start(&mut self) {
        for _ in 0..self.max_pending_pieces {
            self.schedule_next();
        }
        self.schedule_all();
        tracing::info!("Started scheduler");
    }

    pub fn register_performance(&mut self) {
        for peer in self.peers.iter_mut() {
            let newest_performance = Performance::new(peer.downloaded, peer.uploaded);
            peer.performance_history.update(newest_performance);
        }
    }

    /// Pending pieces that are not filled
    fn available_pieces(&self) -> Vec<usize> {
        self.pending_pieces
            .iter()
            .filter(|(_, pending_piece)| !pending_piece.is_filled())
            .map(|x| *x.0)
            .collect()
    }

    /// Iterator over peers that are choked
    fn choked_peers(&self) -> impl Iterator<Item = &ActivePeer> {
        self.peers.iter().filter(|peer| peer.out_status.is_choked())
    }

    pub fn get_peer_idx(&self, peer_id: &Uuid) -> Option<usize> {
        self.peers.iter().position(|p| p.id == *peer_id)
    }

    pub fn is_torrent_finished(&self) -> bool {
        self.bitfield.is_full(self.pieces.len())
    }

    pub fn total_pieces(&self) -> usize {
        self.pieces.len()
    }
}
