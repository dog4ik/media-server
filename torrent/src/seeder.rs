use std::collections::{BTreeMap, HashMap};

use bytes::Bytes;

use crate::{download::Block, protocol::peer::PeerMessage, storage::StorageHandle};

const CACHE_SIZE: usize = 10;

pub struct Seeder {
    pending_retrieves: HashMap<u32, Vec<flume::Sender<PeerMessage>>>,
    cache: BTreeMap<u32, Bytes>,
    storage: StorageHandle,
}

impl Seeder {
    pub fn new(storage: StorageHandle) -> Self {
        Self {
            pending_retrieves: HashMap::new(),
            cache: BTreeMap::new(),
            storage,
        }
    }

    pub fn request_block(&mut self, block: Block) -> Option<Bytes> {
        if let Some(cache_piece) = self.cache.get(&block.piece) {
            return Some(cache_piece.slice(block.range()));
        }

        None
    }
}
