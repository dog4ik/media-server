use std::{collections::HashMap, path::Path};

use anyhow::{anyhow, Context};
use bytes::{Bytes, BytesMut};
use uuid::Uuid;

use crate::{
    download::{ActivePeer, Block, PeerCommand, Performance},
    peers::BitField,
    protocol::{Hashes, Info},
    storage::TorrentStorage,
    utils,
};

#[derive(Debug, Default)]
pub enum ScheduleStrategy {
    #[default]
    Linear,
    RareFirst,
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

#[derive(Debug)]
pub struct Scheduler {
    piece_size: usize,
    max_pending_pieces: usize,
    total_length: u64,
    pieces: Hashes,
    failed_blocks: Vec<Block>,
    pub bitfield: BitField,
    pub pending_pieces: HashMap<usize, Vec<PendingBlock>>,
    pub peers: HashMap<Uuid, ActivePeer>,
    pub storage: TorrentStorage,
    schedule_stategy: ScheduleStrategy,
    pub is_endgame: bool,
}

const BLOCK_LENGTH: u32 = 16 * 1024;

impl Scheduler {
    pub fn new(
        output_dir: impl AsRef<Path>,
        t: Info,
        active_peers: HashMap<Uuid, ActivePeer>,
    ) -> Scheduler {
        let total_pieces = t.pieces.len();
        let storage = TorrentStorage::new(&t, output_dir);
        let bitfield = BitField::empty(total_pieces);
        Self {
            piece_size: t.piece_length as usize,
            total_length: t.total_size(),
            pieces: t.pieces.clone(),
            pending_pieces: HashMap::new(),
            failed_blocks: Vec::new(),
            max_pending_pieces: 100,
            peers: active_peers,
            storage,
            bitfield,
            schedule_stategy: ScheduleStrategy::default(),
            is_endgame: false,
        }
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        crate::utils::piece_size(piece_i, self.piece_size, self.total_length as usize) as u32
    }

    /// Schedules next piece linearly (the next missing piece from start)
    /// returing `None` if no more pieces left
    fn linear_next(&mut self) -> Option<usize> {
        for i in 0..self.pieces.len() {
            if !self.bitfield.has(i) && self.pending_pieces.get(&i).is_none() {
                tracing::debug!("Assigning next linear piece {i}");
                self.pending_pieces.insert(i, Vec::new());
                return Some(i);
            }
        }
        None
    }

    fn rare_first_next(&mut self) -> Option<usize> {
        todo!()
    }

    fn schedule_next(&mut self) -> Option<usize> {
        match self.schedule_stategy {
            ScheduleStrategy::Linear => self.linear_next(),
            ScheduleStrategy::RareFirst => self.rare_first_next(),
        }
    }

    /// Save block
    pub async fn save_block(
        &mut self,
        sender_id: Uuid,
        insert_block: Block,
        data: Bytes,
    ) -> anyhow::Result<()> {
        let piece_length = self.piece_length(insert_block.piece as usize);
        let blocks = self
            .pending_pieces
            .get_mut(&(insert_block.piece as usize))
            .ok_or(anyhow!("pending piece {} is not found", insert_block.piece))?;

        if let Some(idx) = blocks
            .iter()
            .position(|x| x.offset == insert_block.offset && x.bytes.is_none())
        {
            let block = &mut blocks[idx];
            block.bytes = Some(data);
        }

        let sender = self.peers.get_mut(&sender_id).context("Get sender")?;

        if sender.out_status.is_choked() {
            tracing::warn!("Choked peer ({}) is sending blocks", sender_id);
        }

        let downloaded_block = sender
            .pending_blocks
            .iter()
            .position(|b| b.offset == insert_block.offset && b.piece == insert_block.piece)
            .map(|idx| sender.pending_blocks.swap_remove(idx))
            .ok_or(anyhow!(
                "could not found downloaded block, it might be reassigned"
            ))?;
        sender.downloaded += downloaded_block.length as u64;

        if piece_is_full(blocks, piece_length) {
            let bytes = blocks.drain(..).fold(
                BytesMut::with_capacity(piece_length as usize),
                |mut acc, block| {
                    acc.extend_from_slice(&block.bytes.expect("block to be full"));
                    acc
                },
            );
            self.storage
                .save_piece(&self.bitfield, insert_block.piece as usize, bytes.into())
                .await?;
            self.bitfield.add(insert_block.piece as usize).unwrap();
            self.pending_pieces.remove(&(insert_block.piece as usize));

            // Announce piece to everyone
            for (peer_id, peer) in self.peers.iter_mut() {
                if *peer_id == sender_id {
                    continue;
                }
                let _ = peer.command.try_send(PeerCommand::Have {
                    piece: insert_block.piece,
                });
            }
            match self.schedule_next() {
                Some(_) => {}
                None => {
                    if !self.is_endgame {
                        self.execute_end_game();
                    }
                }
            };
        }

        Ok(())
    }

    /// Schedules next batch for peer
    pub fn schedule(&mut self, peer_id: &Uuid) {
        let available_pieces = self.available_pieces();
        let peer = self.peers.get_mut(&peer_id);
        let Some(peer) = peer else {
            tracing::error!("Could not find peer with id: {peer_id}");
            return;
        };
        if !peer.out_status.is_interested() {
            peer.out_status.interest();
            peer.command.try_send(PeerCommand::Interested).unwrap();
        }
        let performance = peer.performance_history.avg_speed() / 1024;
        let performance = performance as usize;
        let rate = if performance < 20 {
            performance + 2
        } else {
            performance / 5 + 18
        };
        let schedule_amount = rate as isize - peer.pending_blocks.len() as isize;
        if schedule_amount < 0 {
            return;
        }
        let schedule_amount = schedule_amount as usize;
        let mut assigned_blocks = Vec::with_capacity(schedule_amount);
        'outer: for _ in 0..schedule_amount {
            for (i, block) in self.failed_blocks.iter().enumerate() {
                if peer.bitfield.has(block.piece as usize) {
                    let block = self.failed_blocks.swap_remove(i);
                    tracing::trace!("Scheduling FAILED block: {:?} to peer {}", block, peer_id,);
                    assigned_blocks.push(block);
                    continue 'outer;
                };
            }

            if self.is_endgame {
                for (piece_i, pending_blocks) in &self.pending_pieces {
                    for pending_block in pending_blocks {
                        let b = pending_block.as_block(*piece_i as u32);
                        if pending_block.bytes.is_none()
                            && !peer.pending_blocks.contains(&b)
                            && !assigned_blocks.contains(&b)
                        {
                            assigned_blocks.push(b);
                            continue 'outer;
                        }
                    }
                }
            } else {
                for piece in available_pieces.iter().copied() {
                    if peer.bitfield.has(piece) {
                        let p_length = crate::utils::piece_size(
                            piece,
                            self.piece_size,
                            self.total_length as usize,
                        );
                        let pending_blocks = self.pending_pieces.get_mut(&piece).unwrap();
                        let Some(new_block) =
                            pend_block(pending_blocks, piece, p_length as u32, BLOCK_LENGTH)
                        else {
                            continue;
                        };
                        assigned_blocks.push(new_block);
                        tracing::trace!("Scheduling block: {:?} to peer {}", new_block, peer_id,);
                        continue 'outer;
                    }
                }
            }
        }

        if assigned_blocks.len() != schedule_amount {
            tracing::warn!("Can't fulfill peer's rate");
        }

        for block in assigned_blocks {
            if let Ok(_) = peer.command.try_send(PeerCommand::Start { block }) {
                peer.pending_blocks.push(block);
            } else {
                if self.is_endgame {
                    panic!("failed to assign block {:?} in endgame", block);
                }
                self.failed_blocks.push(block);
            };
        }
    }

    pub fn best_peer(&self) -> Option<Uuid> {
        let mut max = 0;
        let mut best_peer = None;
        for (id, peer) in &self.peers {
            let performance = peer.performance_history.avg_speed();
            if performance > max {
                best_peer = Some(*id);
                max = performance;
            }
        }
        best_peer
    }

    pub fn execute_end_game(&mut self) {
        tracing::info!("Entered end game mode");
        println!("Entered end game mode");
        self.is_endgame = true;
        let piece_size = self.piece_size;
        let total_length = self.total_length;
        for (piece, blocks) in &mut self.pending_pieces {
            let p_len = utils::piece_size(*piece, piece_size, total_length as usize);
            while let Some(_) = pend_block(blocks, *piece, p_len as u32, BLOCK_LENGTH) {}
        }
        println!("Game mode preparation finished");
        self.schedule_all();
    }

    pub async fn handle_peer_choke(&mut self, peer_id: Uuid) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            self.failed_blocks.extend(peer.pending_blocks.drain(..));
            peer.in_status.choke();
            self.schedule_all();
        }
    }

    pub async fn handle_peer_unchoke(&mut self, peer_id: Uuid) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.in_status.unchoke();
            self.schedule(&peer_id);
        };
    }

    pub fn remove_peer(&mut self, peer_id: Uuid) -> Option<ActivePeer> {
        let mut peer = self.peers.remove(&peer_id)?;
        self.failed_blocks.extend(peer.pending_blocks.drain(..));
        self.schedule_all();
        Some(peer)
    }

    pub fn choke_peer(&mut self, peer_id: Uuid) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            self.failed_blocks.extend(peer.pending_blocks.drain(..));
            peer.out_status.choke();
            self.schedule_all();
        };
    }

    pub fn schedule_all(&mut self) {
        let ids: Vec<_> = self
            .peers
            .iter()
            .filter(|x| !x.1.in_status.is_choked())
            .map(|x| *x.0)
            .collect();
        for id in ids {
            self.schedule(&id);
        }
    }

    pub async fn add_peer(&mut self, mut peer: ActivePeer, uuid: Uuid) {
        peer.out_status.unchoke();
        peer.out_status.interest();
        peer.command.try_send(PeerCommand::Unchoke).unwrap();
        peer.command.try_send(PeerCommand::Interested).unwrap();
        self.peers.insert(uuid, peer);
    }

    pub async fn start(&mut self) {
        for _ in 0..self.max_pending_pieces {
            self.schedule_next();
        }
        tracing::info!("Started scheduler");
    }

    pub fn register_performance(&mut self) {
        for peer in self.peers.values_mut() {
            let newest_performance = Performance::new(peer.downloaded, peer.uploaded);
            peer.performance_history.update(newest_performance);
        }
    }

    /// Pending pieces that are not filled
    fn available_pieces(&self) -> Vec<usize> {
        self.pending_pieces
            .iter()
            .filter(|(piece_i, blocks)| {
                let p_len = self.piece_length(**piece_i);
                !piece_is_filled(blocks, p_len)
            })
            .map(|x| *x.0)
            .collect()
    }

    /// Iterator over peers that are choked
    fn choked_peers(&self) -> impl Iterator<Item = (&Uuid, &ActivePeer)> {
        self.peers
            .iter()
            .filter(|(_, peer)| peer.out_status.is_choked())
    }

    pub async fn retrieve_piece(&mut self, piece_i: usize) -> Option<Bytes> {
        self.storage.retrieve_piece(&self.bitfield, piece_i).await
    }

    pub fn get_peer(&self, peer_id: &Uuid) -> Option<&ActivePeer> {
        self.peers.get(peer_id)
    }

    pub fn get_peer_mut(&mut self, peer_id: &Uuid) -> Option<&mut ActivePeer> {
        self.peers.get_mut(peer_id)
    }

    pub fn is_torrent_finished(&self) -> bool {
        self.bitfield.pieces().count() == self.storage.pieces.len()
    }

    pub fn total_pieces(&self) -> usize {
        self.pieces.len()
    }
}

fn piece_is_full(blocks: &Vec<PendingBlock>, piece_len: u32) -> bool {
    let mut total_len = 0;
    for block in blocks {
        let Some(bytes) = &block.bytes else {
            return false;
        };
        assert_eq!(block.length as usize, bytes.len());
        total_len += block.length;
    }
    total_len == piece_len
}

fn piece_is_filled(blocks: &Vec<PendingBlock>, piece_len: u32) -> bool {
    let mut total_len = 0;
    for block in blocks {
        total_len += block.length;
    }
    total_len == piece_len
}

fn pend_block(
    blocks: &mut Vec<PendingBlock>,
    piece: usize,
    piece_length: u32,
    recommended_length: u32,
) -> Option<Block> {
    let insert_offset = blocks.last().map(|x| x.length + x.offset).unwrap_or(0);
    let length = std::cmp::min(piece_length - insert_offset, recommended_length);
    if length == 0 {
        return None;
    }

    blocks.push(PendingBlock::new(insert_offset, length));
    let block = Block {
        piece: piece as u32,
        offset: insert_offset,
        length,
    };
    Some(block)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tracing_test::traced_test;

    use crate::{
        peers::BitField,
        protocol::Hashes,
        scheduler::{pend_block, ScheduleStrategy, Scheduler},
        storage::TorrentStorage,
    };

    #[test]
    #[traced_test]
    fn assignemnt() {
        let pieces_amount = 3;
        let hashes = Hashes(std::iter::repeat_n([0; 20], pieces_amount).collect());
        let bitfield = BitField::empty(hashes.len());
        let active_peers = HashMap::new();
        let mut scheduler = Scheduler {
            piece_size: 2,
            max_pending_pieces: 20,
            failed_blocks: Vec::new(),
            total_length: 10,
            pieces: hashes.clone(),
            pending_pieces: HashMap::new(),
            peers: active_peers,
            storage: TorrentStorage {
                output_files: Vec::new(),
                piece_size: 2,
                total_length: 10,
                pieces: hashes,
            },
            bitfield,
            schedule_stategy: ScheduleStrategy::default(),
            is_endgame: false,
        };
        assert!(scheduler.linear_next().is_some());
        assert!(scheduler.linear_next().is_some());
        assert!(scheduler.linear_next().is_some());
        assert!(scheduler.linear_next().is_none());
    }

    #[test]
    #[traced_test]
    fn pending_blocks() {
        let pieces_amount = 3;
        let hashes = Hashes(std::iter::repeat_n([0; 20], pieces_amount).collect());
        let bitfield = BitField::empty(hashes.len());
        let active_peers = HashMap::new();
        let mut scheduler = Scheduler {
            piece_size: 10,
            max_pending_pieces: 20,
            failed_blocks: Vec::new(),
            total_length: 10,
            pieces: hashes.clone(),
            pending_pieces: HashMap::new(),
            peers: active_peers,
            storage: TorrentStorage {
                output_files: Vec::new(),
                piece_size: 2,
                total_length: 10,
                pieces: hashes,
            },
            bitfield,
            schedule_stategy: ScheduleStrategy::default(),
            is_endgame: false,
        };
        let p_len = 10;
        scheduler.linear_next();
        let blocks = scheduler.pending_pieces.get_mut(&0).unwrap();
        assert!(pend_block(blocks, 0, p_len, 3).is_some());
        assert!(pend_block(blocks, 0, p_len, 3).is_some());
        assert!(pend_block(blocks, 0, p_len, 4).is_some());
        let blocks = scheduler.pending_pieces.get_mut(&0).unwrap();
        blocks.remove(1);
        dbg!(&blocks);
        assert!(pend_block(blocks, 0, p_len, 2).is_none());
        assert!(pend_block(blocks, 0, p_len, 3).is_none());
    }
}
