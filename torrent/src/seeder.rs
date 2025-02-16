use std::{collections::HashMap, num::NonZeroUsize};

use bytes::Bytes;

use crate::{download::Block, protocol::peer::PeerMessage, storage::StorageHandle};

const CACHE_SIZE: usize = 4;

#[derive(Debug)]
struct Retrieve {
    block_offset: u32,
    block_length: u32,
    sender: flume::Sender<PeerMessage>,
}

impl Retrieve {
    fn new(block: Block, sender: flume::Sender<PeerMessage>) -> Self {
        Self {
            block_offset: block.offset,
            block_length: block.length,
            sender,
        }
    }
}

#[derive(Debug)]
pub struct Seeder {
    pending_retrieves: HashMap<usize, Vec<Retrieve>>,
    piece_cache: lru::LruCache<u32, Bytes>,
    storage: StorageHandle,
}

impl Seeder {
    pub fn new(storage: StorageHandle) -> Self {
        Self {
            pending_retrieves: HashMap::new(),
            piece_cache: lru::LruCache::new(NonZeroUsize::new(CACHE_SIZE).unwrap()),
            storage,
        }
    }

    pub fn request_block(
        &mut self,
        block: Block,
        sender: flume::Sender<PeerMessage>,
    ) -> Option<Bytes> {
        if let Some(cache_piece) = self.piece_cache.get(&block.piece) {
            return Some(cache_piece.slice(block.range()));
        }
        self.pending_retrieves
            .entry(block.piece as usize)
            .or_default()
            .push(Retrieve::new(block, sender));
        let _ = self.storage.try_retrieve_piece(block.piece as usize);
        None
    }

    pub fn handle_retrieve(&mut self, piece_i: usize, piece: Bytes) {
        let index = piece_i as u32;
        for retrieve in self
            .pending_retrieves
            .get_mut(&piece_i)
            .iter_mut()
            .flat_map(|retrieves| retrieves.drain(..))
        {
            let block_offset = retrieve.block_offset as usize;
            let block = piece.slice(block_offset..block_offset + retrieve.block_length as usize);
            let _ = retrieve.sender.try_send(PeerMessage::Piece {
                index,
                begin: retrieve.block_offset,
                block,
            });
        }
    }

    pub fn handle_retrieve_error(&mut self, piece_i: usize) {
        let index = piece_i as u32;
        self.pending_retrieves.remove(&piece_i);
    }

    #[allow(unused)]
    pub fn cancel_retrieve(&self, piece_i: usize, sender: &flume::Sender<PeerMessage>) {
        let find_peer = |r: &Vec<Retrieve>| r.iter().any(|c| c.sender.same_channel(sender));
        if self.pending_retrieves.get(&piece_i).is_some_and(find_peer) {}
    }
}
