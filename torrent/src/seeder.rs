use std::{collections::HashMap, num::NonZeroUsize};

use bytes::Bytes;

use crate::{download::Block, protocol::peer::PeerMessage, storage::StorageHandle};

const CACHE_SIZE: usize = 10;

#[derive(Debug)]
pub struct Seeder {
    pending_retrieves: HashMap<u32, Vec<flume::Sender<PeerMessage>>>,
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

    pub async fn request_block(&mut self, block: Block) -> Option<Bytes> {
        if let Some(cache_piece) = self.piece_cache.get(&block.piece) {
            Some(cache_piece.slice(block.range()))
        } else {
            let _ = self.storage.retrieve_piece(block.piece as usize).await;
            None
        }
    }
}
