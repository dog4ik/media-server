use std::{collections::HashMap, path::Path};

use anyhow::anyhow;
use bytes::{Bytes, BytesMut};
use uuid::Uuid;

use crate::{
    download::{piece_size, ActivePeer, Block, PeerCommand},
    file::{Hashes, TorrentFile},
    peers::BitField,
    storage::TorrentStorage,
};

#[derive(Debug, Clone)]
pub struct Assignment {
    pub peer_id: Uuid,
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub enum ScheduleStrategy {
    Linear,
    RareFirst,
}

pub trait Schedule {
    fn assign_next(&mut self, bitfield: BitField) -> Option<usize>;
}

#[derive(Debug)]
pub struct LinearAssignment {
    pub current_piece: usize,
    pub total_pieces: usize,
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

pub const MAX_PENDING_BLOCKS: usize = 40;

#[derive(Debug)]
pub struct Scheduler {
    piece_size: usize,
    max_pending_pieces: usize,
    total_length: u64,
    pieces: Hashes,
    pub bitfield: BitField,
    pending_pieces: HashMap<usize, Vec<PendingBlock>>,
    pub active_peers: HashMap<Uuid, ActivePeer>,
    pub storage: TorrentStorage,
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
            piece_size: t.info.piece_length as usize,
            total_length: t.info.total_size(),
            pieces: t.info.pieces.clone(),
            pending_pieces: HashMap::new(),
            failed_blocks: Vec::new(),
            max_pending_pieces: 100,
            max_connections: 10,
            active_peers,
            storage,
            bitfield,
        }
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        piece_size(
            piece_i as u32,
            self.piece_size as u32,
            self.pieces.len() as u32,
            self.total_length as u32,
        )
    }

    /// Schedules next piece linearly (the next missing piece from start)
    /// returing `None` if no more pieces left
    pub fn linear_next(&mut self) -> Option<usize> {
        for i in 0..self.pieces.len() {
            if !self.bitfield.has(i) && self.pending_pieces.get(&i).is_none() {
                self.pending_pieces.insert(i, Vec::new());
                return Some(i);
            }
        }
        None
    }

    /// Save block
    pub async fn save_block(
        &mut self,
        sender_id: Uuid,
        insert_block: Block,
        data: Bytes,
    ) -> anyhow::Result<()> {
        let piece_length = self.piece_length(insert_block.piece as usize);
        let Some(blocks) = self.pending_pieces.get_mut(&(insert_block.piece as usize)) else {
            return Err(anyhow!("piece is not found in pending pieces"));
        };

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
            .expect("block sender to be here");
        dbg!(&sender_id, &sender.pending_blocks, &insert_block);
        sender
            .pending_blocks
            .iter()
            .position(|b| b.offset == insert_block.offset)
            .map(|idx| sender.pending_blocks.swap_remove(idx))
            .unwrap();

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
            let next = self.linear_next().ok_or(anyhow!("no more work"))?;
            tracing::debug!("Assigning next piece {}", next);
        }

        Ok(())
    }

    /// Schedule next block
    fn pend_block(&mut self, piece: usize, recommended_length: u32) -> Option<Block> {
        let piece_length = self.piece_length(piece);
        let blocks = self.pending_pieces.get_mut(&piece).unwrap();
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

    pub async fn schedule_next_block(&mut self) -> anyhow::Result<()> {
        'outer: for piece in self.available_pieces() {
            let Some(compatible_peer) = self
                .available_peers()
                .find(|(_, peer)| peer.bitfield.has(piece))
                .map(|(peer_id, _)| *peer_id)
            else {
                continue;
            };
            for _ in 0..5 {
                let Some(new_block) = self.pend_block(piece, BLOCK_LENGTH) else {
                    break 'outer;
                };
                let peer = self.get_peer_mut(&compatible_peer).unwrap();
                peer.pending_blocks.push(new_block);
                tracing::debug!(
                    "Scheduling block: {:?} to peer {}",
                    new_block,
                    compatible_peer
                );
                peer.command
                    .send(PeerCommand::Start { block: new_block })
                    .await
                    .unwrap();
            }
        }
        Ok(())
    }

    /// Resets unfinished blocks of the peer with given id
    pub fn cancel_peer(&mut self, peer_id: Uuid) {
        let peer = self.active_peers.get_mut(&peer_id).expect("peer to exist");
        tracing::debug!("Canceling peer with id {}", peer_id);
        for block in peer.pending_blocks.drain(..) {
            if let Some(pending_piece) = self.pending_pieces.get_mut(&(block.piece as usize)) {
                pending_piece
                    .iter()
                    .position(|x| x.offset == block.offset && x.bytes.is_none())
                    .map(|idx| pending_piece.remove(idx));
            };
        }
    }

    pub async fn handle_peer_unchoke(&mut self, peer_id: Uuid) {
        let peer = self.active_peers.get_mut(&peer_id).unwrap();
        peer.choke_status.unchoke();

        self.schedule_next_block().await.unwrap();
    }

    pub fn remove_peer(&mut self, peer_id: Uuid) {
        self.cancel_peer(peer_id);
        self.active_peers.remove(&peer_id).unwrap();
    }

    pub fn choke_peer(&mut self, peer_id: Uuid) {
        // BUG: this shit breaks shit
        // self.cancel_peer(peer_id);
        let peer = self.active_peers.get_mut(&peer_id).unwrap();
        peer.choke_status.choke();
    }

    pub async fn start(&mut self) {
        for (_, peer) in self.active_peers.iter_mut() {
            peer.command.send(PeerCommand::Interested).await.unwrap();
            break;
        }
        self.linear_next();
    }

    /// Iterator over peers that are ready for work
    pub fn available_peers(&self) -> impl Iterator<Item = (&Uuid, &ActivePeer)> {
        self.active_peers
            .iter()
            .filter(|(_, peer)| !peer.choke_status.is_choked() && peer.pending_blocks.is_empty())
    }

    /// Pending pieces that are not filled
    pub fn available_pieces(&self) -> Vec<usize> {
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
    pub fn choked_peers(&self) -> impl Iterator<Item = (&Uuid, &ActivePeer)> {
        self.active_peers
            .iter()
            .filter(|(_, peer)| peer.choke_status.is_choked())
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tracing_test::traced_test;

    use crate::{file::Hashes, peers::BitField, scheduler::Scheduler, storage::TorrentStorage};

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
        };
        scheduler.linear_next();
        assert!(dbg!(scheduler.pend_block(0, 3)).is_some());
        assert!(dbg!(scheduler.pend_block(0, 3)).is_some());
        assert!(dbg!(scheduler.pend_block(0, 4)).is_some());
        assert!(dbg!(scheduler.pend_block(0, 2)).is_none());
        assert!(dbg!(scheduler.pend_block(0, 3)).is_none());
    }
}
