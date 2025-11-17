use bytes::Bytes;
use tokio::task::JoinSet;

use crate::{protocol::Hashes, utils};

/// Hash validation
#[derive(Debug)]
pub struct Hasher {
    pub hashes: Hashes,
    set: JoinSet<WorkResult>,
}

impl Hasher {
    pub fn new(hashes: Hashes) -> Self {
        Self {
            set: JoinSet::new(),
            hashes,
        }
    }

    pub fn pend_job(&mut self, piece: usize, data: Vec<Bytes>) {
        let payload = Payload {
            hash: self.hashes[piece],
            data,
        };
        self.set.spawn_blocking(move || {
            let is_verified = payload.verify_hash();
            WorkResult {
                piece_i: piece,
                is_verified,
                blocks: payload.data,
            }
        });
    }

    /// Cancellation safe
    pub async fn join_next(&mut self) -> Option<WorkResult> {
        self.set
            .join_next()
            .await
            .map(|v| v.expect("join task never panic"))
    }

    pub fn len(&self) -> usize {
        self.set.len()
    }
}

#[derive(Debug, Clone)]
pub struct Payload {
    // TODO: avoid hash copy
    pub hash: [u8; 20],
    pub data: Vec<Bytes>,
}

impl Payload {
    pub fn verify_hash(&self) -> bool {
        utils::verify_iter_sha1(&self.hash, self.data.iter())
    }
}

#[derive(Debug, Clone)]
pub struct WorkResult {
    pub piece_i: usize,
    pub is_verified: bool,
    pub blocks: Vec<Bytes>,
}
