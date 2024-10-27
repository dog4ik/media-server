use std::{cmp::Reverse, fmt::Display, ops::Range};

use anyhow::ensure;
use bytes::Bytes;
use uuid::Uuid;

use crate::{
    download::{ActivePeer, Block, BlockPosition, DataBlock, Performance},
    piece_picker::{PiecePicker, Priority, ScheduleStrategy},
    protocol::{peer::PeerMessage, ut_metadata::UtMetadata, Info, OutputFile},
    utils,
};

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
        files.sort_unstable_by_key(|file| file.priority);
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

#[derive(Debug, Clone)]
pub struct PendingPieceV2 {
    piece: Vec<PendingBlockV2>,
    piece_length: u32,
    blocks_queue: Vec<BlockPosition>,
    /// Amount of blocks are saved in piece
    saved_amount: u16,
    is_sub_rational: bool,
}

#[derive(Debug, Clone)]
pub struct PendingBlockV2 {
    pub scheduled_to: Vec<Uuid>,
    pub data: Option<Bytes>,
}

impl Default for PendingBlockV2 {
    fn default() -> Self {
        Self {
            scheduled_to: Vec::with_capacity(1),
            data: None,
        }
    }
}

impl PendingPieceV2 {
    pub fn new(piece_size: u32) -> Self {
        // same as (piece_size + BLOCK_LENGTH - 1) / BLOCK_LENGTH;
        let blocks_amount = piece_size.div_ceil(BLOCK_LENGTH);
        let piece = vec![PendingBlockV2::default(); blocks_amount as usize];
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
            blocks_queue: blocks,
            saved_amount: 0,
            is_sub_rational: false,
        }
    }

    pub fn new_sub_rational(piece_size: u32) -> Self {
        let mut this = Self::new(piece_size);
        this.is_sub_rational = true;
        this
    }

    pub fn is_filled(&self) -> bool {
        self.blocks_queue.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.saved_amount == self.piece.len() as u16
    }

    /// Panics if piece is not full
    pub fn as_bytes(self) -> Vec<Bytes> {
        self.piece.into_iter().map(|x| x.data.unwrap()).collect()
    }

    pub fn pend_block(&mut self, sender: Uuid) -> Option<BlockPosition> {
        let block = self.blocks_queue.pop()?;
        let index = block.offset / BLOCK_LENGTH;
        let pending_block = &mut self.piece[index as usize];
        pending_block.scheduled_to.push(sender);
        Some(block)
    }

    /// Position of the first None block Does not affect the block queue.
    pub fn pend_blocks_endgame(
        &mut self,
        take: usize,
        peer_id: Uuid,
    ) -> impl IntoIterator<Item = (BlockPosition, &mut PendingBlockV2)> + '_ {
        let p_length = self.piece_length;
        let amount = self.piece.len();
        self.piece
            .iter_mut()
            .enumerate()
            .filter_map(move |(idx, x)| {
                if x.data.is_some() {
                    return None;
                }
                if x.scheduled_to.contains(&peer_id) {
                    return None;
                }
                let offset = idx as u32 * BLOCK_LENGTH;
                let length = if idx == amount - 1 {
                    p_length - offset
                } else {
                    BLOCK_LENGTH
                };
                Some((BlockPosition { offset, length }, x))
            })
            .take(take)
    }

    pub fn unpend_block(&mut self, block: BlockPosition) {
        self.blocks_queue.push(block);
    }

    pub fn save_block(&mut self, data_block: DataBlock) -> anyhow::Result<()> {
        ensure!(data_block.offset + data_block.len() as u32 <= self.piece_length);

        let index = data_block.offset / BLOCK_LENGTH;
        let block = &mut self.piece[index as usize];
        if block.data.is_none() {
            block.data = Some(data_block.block);
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

#[derive(Debug, Clone, Copy, Default)]
pub struct ScheduleStat {
    pub rational: usize,
    pub sub_rational: usize,
    pub endgame: usize,
}

impl ScheduleStat {
    pub fn total(&self) -> usize {
        self.rational + self.sub_rational + self.endgame
    }
}

impl Display for ScheduleStat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.total())
    }
}

#[derive(Debug)]
pub struct Scheduler {
    piece_size: u32,
    /// Full ut_metadata used to share it
    pub ut_metadata: UtMetadata,
    pub max_pending_pieces: usize,
    total_length: u64,
    pub peers: Vec<ActivePeer>,
    pub picker: PiecePicker,
    pub pending_files: PendingFiles,
    /// Amount of pending pieces that are considered sub rational (do not play well with
    /// scheduling strategy but improve download performance and peer utilization)
    pub sub_rational_amount: usize,
    pub piece_table: Vec<SchedulerPiece>,
    pub pending_pieces: Vec<usize>,
    pub downloaded_pieces: usize,
}

pub const BLOCK_LENGTH: u32 = 16 * 1024;
/// Maximum amount of peers allowed to schedule one block.
/// 4 will be good fit because vec reallocates with capacity 4 after first push.
const MAX_SCHEDULED_TO: usize = 4;

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
        let picker = PiecePicker::new(&piece_table);
        Self {
            piece_size: t.piece_length,
            ut_metadata,
            total_length: t.total_size(),
            max_pending_pieces: 40,
            peers: Vec::new(),
            picker,
            pending_files,
            piece_table,
            pending_pieces: Vec::new(),
            downloaded_pieces: 0,
            sub_rational_amount: 0,
        }
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        crate::utils::piece_size(piece_i, self.piece_size, self.total_length) as u32
    }

    fn schedule_next(&mut self, peer_idx: usize, schedule_amount: usize) -> ScheduleStat {
        let mut took = 0;
        let mut stat = ScheduleStat::default();
        let peer = &mut self.peers[peer_idx];

        // First we try to schedule pending pieces.
        // NOTE: We can merge it with endgame mode logic
        for i in self
            .pending_pieces
            .iter()
            .filter(|p| peer.bitfield.has(**p))
        {
            let blocks = self.piece_table[*i]
                .pending_blocks
                .as_mut()
                .expect("index is from pending pieces");
            while let Some(position) = blocks.pend_block(peer.id) {
                let block = Block::from_position(*i as u32, position);
                match peer.message_tx.try_send(PeerMessage::request(block)) {
                    Ok(_) => {
                        took += 1;
                        stat.rational += 1;
                        peer.pending_blocks += 1;
                        if took == schedule_amount {
                            return stat;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Send error: {e}");
                        blocks.unpend_block(position);
                        return stat;
                    }
                }
            }
        }

        // We know that pending pieces are not enough to fulfill peer capabalities.
        // We should try to add new pending piece for this peer.

        loop {
            if took == schedule_amount {
                return stat;
            }
            match self.picker.peek_next() {
                // Next piece is rational and peer can share it
                Some(new_piece) if peer.bitfield.has(new_piece) => {
                    let new_piece = self.picker.pop_next().expect("we peeking above");
                    let piece_len =
                        utils::piece_size(new_piece, self.piece_size, self.total_length) as u32;
                    let pending_piece = PendingPieceV2::new(piece_len);
                    self.pending_pieces.push(new_piece);
                    self.piece_table[new_piece].pending_blocks = Some(pending_piece);
                    let pending_piece = self.piece_table[new_piece]
                        .pending_blocks
                        .as_mut()
                        .expect("just inserted it");

                    while let Some(position) = pending_piece.pend_block(peer.id) {
                        let block = Block::from_position(new_piece as u32, position);
                        match peer.message_tx.try_send(PeerMessage::request(block)) {
                            Ok(_) => {
                                took += 1;
                                stat.rational += 1;
                                peer.pending_blocks += 1;
                                if took == schedule_amount {
                                    return stat;
                                }
                            }
                            Err(e) => {
                                tracing::error!("Send error: {e}");
                                pending_piece.unpend_block(position);
                                return stat;
                            }
                        }
                    }
                }
                // Peer does not have next piece we should schedule sub-optional blocks
                // TODO: use configurable f32 threshold
                Some(_)
                    if self.sub_rational_amount as f32 / self.pending_pieces.len() as f32
                        <= 0.3 =>
                {
                    let Some(new_piece) = self.picker.pop_closest_for_bitfield(&peer.bitfield)
                    else {
                        return stat;
                    };
                    let piece_len =
                        utils::piece_size(new_piece, self.piece_size, self.total_length) as u32;
                    let pending_piece = PendingPieceV2::new_sub_rational(piece_len);
                    self.sub_rational_amount += 1;
                    self.pending_pieces.push(new_piece);
                    self.piece_table[new_piece].pending_blocks = Some(pending_piece);
                    let pending_piece = self.piece_table[new_piece]
                        .pending_blocks
                        .as_mut()
                        .expect("just inserted it");

                    while let Some(position) = pending_piece.pend_block(peer.id) {
                        let block = Block::from_position(new_piece as u32, position);
                        match peer.message_tx.try_send(PeerMessage::request(block)) {
                            Ok(_) => {
                                took += 1;
                                stat.sub_rational += 1;
                                peer.pending_blocks += 1;
                                if took == schedule_amount {
                                    return stat;
                                }
                            }
                            Err(e) => {
                                tracing::error!("Send error: {e}");
                                pending_piece.unpend_block(position);
                                return stat;
                            }
                        }
                    }
                }
                // Endgame mode
                None => {
                    for piece_idx in self
                        .pending_pieces
                        .iter()
                        .filter(|p| peer.bitfield.has(**p))
                    {
                        let pending_piece = &mut self.piece_table[*piece_idx];
                        let pending_blocks = pending_piece.pending_blocks.as_mut().unwrap();
                        for (position, pending_block) in
                            pending_blocks.pend_blocks_endgame(schedule_amount, peer.id)
                        {
                            let block = Block::from_position(*piece_idx as u32, position);
                            match peer.message_tx.try_send(PeerMessage::request(block)) {
                                Ok(_) => {
                                    took += 1;
                                    stat.endgame += 1;
                                    pending_block.scheduled_to.push(peer.id);
                                    peer.pending_blocks += 1;
                                }
                                Err(e) => {
                                    tracing::error!("Send error: {e}");
                                    return stat;
                                }
                            }
                        }
                        if took == schedule_amount {
                            return stat;
                        }
                    }
                    // We tried everything at this point
                    return stat;
                }
                // Peer don't have next piece and no sub-optional slots are available
                // There is nothing we can do but let it go
                Some(_) => {
                    return stat;
                }
            }
        }
    }

    pub fn save_block(&mut self, sender_idx: usize, data_block: DataBlock) {
        let piece = data_block.piece as usize;
        let scheduler_piece = &mut self.piece_table[piece];
        let Some(pending_blocks) = scheduler_piece.pending_blocks.as_mut() else {
            tracing::trace!(
                "Peer sent block of piece that is not pending: {}",
                data_block
            );
            return;
        };

        let peer = &mut self.peers[sender_idx];
        peer.downloaded += data_block.len() as u64;
        peer.pending_blocks -= 1;
        match pending_blocks.save_block(data_block) {
            Err(e) => {
                // peer logic error
                peer.cancel_peer();
                tracing::error!("{e}");
            }
            Ok(_) => {}
        }
    }

    /// Schedules next batch for peer
    pub fn schedule(&mut self, peer_idx: usize, tick_duration: &std::time::Duration) {
        let peer = &mut self.peers[peer_idx];

        debug_assert!(peer.out_status.is_interested());
        debug_assert!(!peer.in_status.is_choked());

        let performance_kb = peer.performance_history.avg_down_speed_sec(tick_duration) / 1024;
        // currently it is 32 Mb (2048 blocks) in pipeline if peer uploading 10MB/s
        let rate = if performance_kb < 20 {
            performance_kb + 2
        } else {
            performance_kb / 5 + 18
        };
        let schedule_amount = rate.saturating_sub(peer.pending_blocks);
        if schedule_amount == 0 {
            return;
        }

        // ISSUE: Check whether we couldn't assign piece to peer because no more blocks available, not because
        // he just doesn't have these pieces
        // If any peer have single piece available he will force scheduling pending_piece
        let assigned = self.schedule_next(peer_idx, schedule_amount);

        tracing::debug!(
            "Assigned {} rational | {} sub-rational | {} endgame",
            assigned.rational,
            assigned.sub_rational,
            assigned.endgame
        );

        if assigned.total() < schedule_amount {
            tracing::warn!("Cannot fulfill peer's rate: {assigned}/{schedule_amount}");
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
                peer.remove_interested();
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
        piece.pending_blocks = Some(PendingPieceV2::new(piece_len));
        self.picker.put_back(piece_idx);
    }

    pub fn drain_peer_blocks(&mut self, peer_id: Uuid) {
        for pending_piece_idx in &self.pending_pieces {
            let piece_size = self.piece_length(*pending_piece_idx);
            let pending_piece = self.piece_table[*pending_piece_idx]
                .pending_blocks
                .as_mut()
                .unwrap();
            let blocks_amount = pending_piece.piece.len();
            for (block_idx, block) in pending_piece.piece.iter_mut().enumerate() {
                if block.data.is_none() {
                    if let Some(peer_idx) = block.scheduled_to.iter().position(|p| p == &peer_id) {
                        block.scheduled_to.swap_remove(peer_idx);
                        if block.scheduled_to.is_empty() {
                            let offset = block_idx as u32 * BLOCK_LENGTH;
                            let length = if block_idx == blocks_amount - 1 {
                                piece_size - offset
                            } else {
                                BLOCK_LENGTH
                            };
                            pending_piece
                                .blocks_queue
                                .push(BlockPosition { offset, length });
                        }
                    }
                }
            }
        }
    }

    pub fn handle_peer_choke(&mut self, peer_idx: usize) {
        let peer = &mut self.peers[peer_idx];
        let id = peer.id;
        peer.in_status.set_choke(true);
        self.drain_peer_blocks(id);
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
            peer.add_interested();
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

    pub fn remove_peer(&mut self, peer_idx: usize) -> Option<std::net::SocketAddr> {
        let peer = self.peers.swap_remove(peer_idx);
        let id = peer.id;
        self.drain_peer_blocks(id);
        for piece in peer.bitfield.pieces() {
            self.piece_table[piece].rarity -= 1;
        }
        Some(peer.ip)
    }

    pub fn add_peer(&mut self, mut peer: ActivePeer) {
        let interested_pieces = self.calculate_interested_amount(&peer);
        for _ in 0..interested_pieces {
            peer.add_interested();
        }
        if interested_pieces > 0 {
            peer.set_out_choke(false).unwrap();
            peer.set_out_interset(true).unwrap();
        }
        for piece in peer.bitfield.pieces() {
            self.piece_table[piece].rarity += 1;
        }
        self.peers.push(peer);
    }

    pub async fn start(&mut self) {
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
            self.picker.rebuild_queue(&self.piece_table);
        };
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
        let total_pieces = self.picker.len() + self.pending_pieces.len() + downloaded_pieces;
        let pending_pieces = self.pending_pieces.len();
        (
            downloaded_pieces as f64 / total_pieces as f64 * 100.,
            pending_pieces,
        )
    }

    pub fn peers(&self) -> &Vec<ActivePeer> {
        &self.peers
    }

    pub fn peers_mut(&mut self) -> &mut Vec<ActivePeer> {
        &mut self.peers
    }

    pub fn strategy(&self) -> ScheduleStrategy {
        self.picker.strategy()
    }

    pub fn set_strategy(&mut self, strategy: ScheduleStrategy) {
        self.picker.set_strategy(strategy);
        self.picker.rebuild_queue(&self.piece_table);
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        protocol::{Hashes, Info, SizeDescriptor},
        scheduler::Scheduler,
    };

    use super::PendingFiles;

    const FAKE_PIECE_SIZE: usize = 3;
    const FAKE_PIECES_AMOUNT: usize = 10;

    fn fake_info() -> Info {
        let pieces = Hashes(
            (0..FAKE_PIECES_AMOUNT as u8)
                .into_iter()
                .map(|i| [i; 20])
                .collect(),
        );
        Info {
            name: "Fake torrent".into(),
            pieces,
            piece_length: 3,
            file_descriptor: SizeDescriptor::Length(
                FAKE_PIECES_AMOUNT as u64 * FAKE_PIECE_SIZE as u64 - 2,
            ),
        }
    }

    #[test]
    fn scheduler_interested() {
        let info = fake_info();
        let output_files = info.output_files("");
        let enabled_files = (0..output_files.len()).collect();
        let pending_files =
            PendingFiles::from_output_files(info.piece_length, &output_files, enabled_files);
        let _scheduler = Scheduler::new(info, pending_files);
    }
}
