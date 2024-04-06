use std::collections::HashMap;

use anyhow::anyhow;
use bytes::{Bytes, BytesMut};
use uuid::Uuid;

use crate::{
    download::{ActivePeer, Block, PeerCommand},
    file::{Hashes, Info},
    peers::BitField,
    storage::TorrentStorage,
};

#[derive(Debug, Clone)]
pub struct Assignment {
    pub peer_id: Uuid,
    pub blocks: Vec<Block>,
}

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
}

pub const MAX_PENDING_BLOCKS: usize = 15;

#[derive(Debug)]
pub struct Scheduler {
    piece_size: usize,
    max_pending_pieces: usize,
    max_connections: usize,
    total_length: u64,
    pieces: Hashes,
    failed_blocks: Vec<Block>,
    pub bitfield: BitField,
    pending_pieces: HashMap<usize, Vec<PendingBlock>>,
    pub active_peers: HashMap<Uuid, ActivePeer>,
    pub storage: TorrentStorage,
    schedule_stategy: ScheduleStrategy,
}

const BLOCK_LENGTH: u32 = 16 * 1024;

impl Scheduler {
    pub fn new(t: Info, active_peers: HashMap<Uuid, ActivePeer>) -> Scheduler {
        let total_pieces = t.pieces.len();
        let storage = TorrentStorage::new(&t);
        let bitfield = BitField::empty(total_pieces);
        Self {
            piece_size: t.piece_length as usize,
            total_length: t.total_size(),
            pieces: t.pieces.clone(),
            pending_pieces: HashMap::new(),
            failed_blocks: Vec::new(),
            max_pending_pieces: 100,
            max_connections: 10,
            active_peers,
            storage,
            bitfield,
            schedule_stategy: ScheduleStrategy::default(),
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

    fn most_performant_available_peer(&self) -> Option<Uuid> {
        let mut max = 0;
        let mut peer_id = None;
        for (id, peer) in self.available_peers() {
            let download_speed = peer.download_speed();
            if download_speed > max && peer.pending_blocks.len() <= MAX_PENDING_BLOCKS {
                max = download_speed;
                peer_id = Some(*id);
            }
        }
        peer_id
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
            let block = blocks.get_mut(idx).unwrap();
            block.bytes = Some(data);
        }

        let sender = self
            .active_peers
            .get_mut(&sender_id)
            .expect("block sender channel to be open");

        if sender.out_status.is_choked() {
            tracing::warn!("Choked peer ({}) is sending blocks", sender_id);
        }

        let downloaded_block = sender
            .pending_blocks
            .iter()
            .position(|b| b.offset == insert_block.offset)
            .map(|idx| sender.pending_blocks.swap_remove(idx))
            .ok_or(anyhow!(
                "could not found downloaded block, it might be reassigned"
            ))?;
        sender.downloaded += downloaded_block.length as u64;
        sender.update_performance();

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
            for (peer_id, peer) in self.active_peers.iter_mut() {
                if *peer_id == sender_id {
                    continue;
                }
                let _ = peer.command.try_send(PeerCommand::Have {
                    piece: insert_block.piece,
                });
            }
            self.schedule_next().ok_or(anyhow!("no more work"))?;
        }

        Ok(())
    }

    /// Schedules next block for peer returing `None` if peer cant help in download
    pub async fn schedule(&mut self, peer_id: &Uuid) {
        let available_pieces = self.available_pieces();
        let peer = self.active_peers.get_mut(peer_id).unwrap();

        if peer.pending_blocks.len() != 0 {
            tracing::warn!("Scheduling is not required");
            return;
        };

        'outer: for _ in 0..MAX_PENDING_BLOCKS {
            for (i, block) in self.failed_blocks.iter().enumerate() {
                if peer.bitfield.has(block.piece as usize) {
                    let block = self.failed_blocks.remove(i);
                    if !peer.out_status.is_interested() {
                        peer.out_status.interest();
                        peer.command.try_send(PeerCommand::Interested).unwrap();
                    }
                    peer.pending_blocks.push(block);
                    tracing::trace!("Scheduling FAILED block: {:?} to peer {}", block, peer_id,);
                    peer.command.try_send(PeerCommand::Start { block }).unwrap();
                    continue 'outer;
                };
            }

            for piece in available_pieces.clone() {
                if peer.bitfield.has(piece) {
                    if !peer.out_status.is_interested() {
                        peer.out_status.interest();
                        peer.command.try_send(PeerCommand::Interested).unwrap();
                    }
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
                    peer.pending_blocks.push(new_block);
                    tracing::trace!("Scheduling block: {:?} to peer {}", new_block, peer_id,);
                    peer.command
                        .try_send(PeerCommand::Start { block: new_block })
                        .unwrap();
                    continue 'outer;
                }
            }
        }
    }

    /// Resets unfinished blocks of the peer and appends them to failed blocks
    pub fn cancel_peer(&mut self, peer_id: Uuid) {
        let peer = self.active_peers.get_mut(&peer_id).expect("peer to exist");
        tracing::debug!("Canceling peer with id {}", peer_id);
        self.failed_blocks.extend(peer.pending_blocks.drain(..));
    }

    pub async fn handle_peer_choke(&mut self, peer_id: Uuid) {
        let peer = self.active_peers.get_mut(&peer_id).unwrap();
        peer.in_status.choke();
    }

    pub async fn handle_peer_unchoke(&mut self, peer_id: Uuid) {
        dbg!(self.available_pieces().len());
        let peer = self.active_peers.get_mut(&peer_id).unwrap();
        peer.in_status.unchoke();
        if peer.pending_blocks.len() < MAX_PENDING_BLOCKS {
            self.schedule(&peer_id).await;
        }
    }

    pub fn remove_peer(&mut self, peer_id: Uuid) {
        self.cancel_peer(peer_id);
        self.active_peers.remove(&peer_id).unwrap();
    }

    pub async fn choke_peer(&mut self, peer_id: Uuid) {
        self.cancel_peer(peer_id);
        let peers: Vec<_> = self
            .active_peers
            .iter()
            .map(|(id, peer)| (*id, peer.pending_blocks.len()))
            .collect();

        for (peer, pending_blocks) in peers {
            if peer == peer_id || pending_blocks == MAX_PENDING_BLOCKS {
                continue;
            }
            self.schedule(&peer).await;
        }

        let peer = self.active_peers.get_mut(&peer_id).unwrap();
        peer.out_status.choke();
    }

    pub async fn add_peer(&mut self, mut peer: ActivePeer, uuid: Uuid) {
        peer.out_status.unchoke();
        peer.out_status.interest();
        peer.command.try_send(PeerCommand::Unchoke).unwrap();
        peer.command.try_send(PeerCommand::Interested).unwrap();
        self.active_peers.insert(uuid, peer);
    }

    pub async fn start(&mut self) {
        for _ in 0..self.max_pending_pieces {
            self.schedule_next();
        }
        tracing::info!("Started scheduler");
    }

    /// Iterator over peers that are ready for work
    fn available_peers(&self) -> impl Iterator<Item = (&Uuid, &ActivePeer)> {
        self.active_peers.iter().filter(|(_, peer)| {
            !peer.in_status.is_choked() && peer.pending_blocks.len() < MAX_PENDING_BLOCKS
        })
    }

    /// Iterator over peers that have pending blocks
    fn busy_peers(&self) -> impl Iterator<Item = (&Uuid, &ActivePeer)> {
        self.active_peers
            .iter()
            .filter(|(_, peer)| !peer.pending_blocks.is_empty())
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
        self.active_peers
            .iter()
            .filter(|(_, peer)| peer.out_status.is_choked())
    }

    pub async fn retrieve_piece(&mut self, piece_i: usize) -> Option<Bytes> {
        self.storage.retrieve_piece(&self.bitfield, piece_i).await
    }

    pub fn get_peer(&self, peer_id: &Uuid) -> Option<&ActivePeer> {
        self.active_peers.get(peer_id)
    }

    pub fn get_peer_mut(&mut self, peer_id: &Uuid) -> Option<&mut ActivePeer> {
        self.active_peers.get_mut(peer_id)
    }

    pub fn is_torrent_finished(&self) -> bool {
        self.bitfield.pieces().count() == self.storage.pieces.len()
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
    let mut insert_idx = 0;
    let mut insert_offset = 0;
    for pending_block in blocks.iter() {
        if pending_block.offset + pending_block.length > insert_offset {
            insert_idx += 1;
            insert_offset = pending_block.offset + pending_block.length;
        } else {
            break;
        }
    }
    let prev_block_end = blocks
        .get(std::cmp::max(insert_idx as isize - 1, 0) as usize)
        .map_or(0, |last_block| last_block.offset + last_block.length);
    let next_block_start = blocks
        .get(insert_idx + 1)
        .map_or(piece_length, |next_block| next_block.offset);
    let length = std::cmp::min(next_block_start - prev_block_end, recommended_length);
    if insert_offset + length > piece_length || length == 0 {
        return None;
    }

    blocks.insert(insert_idx, PendingBlock::new(insert_offset, length));
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
        file::Hashes,
        peers::BitField,
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
            max_connections: 10,
            failed_blocks: Vec::new(),
            total_length: 10,
            pieces: hashes.clone(),
            pending_pieces: HashMap::new(),
            active_peers,
            storage: TorrentStorage {
                output_files: Vec::new(),
                piece_size: 2,
                total_length: 10,
                pieces: hashes,
            },
            bitfield,
            schedule_stategy: ScheduleStrategy::default(),
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
            max_connections: 10,
            total_length: 10,
            pieces: hashes.clone(),
            pending_pieces: HashMap::new(),
            active_peers,
            storage: TorrentStorage {
                output_files: Vec::new(),
                piece_size: 2,
                total_length: 10,
                pieces: hashes,
            },
            bitfield,
            schedule_stategy: ScheduleStrategy::default(),
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
