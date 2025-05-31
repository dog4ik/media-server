use std::time::Instant;

use bytes::Bytes;
use tokio::task::JoinSet;

use crate::utils;

/// Hash validation
#[derive(Debug, Default)]
pub struct Hasher {
    set: JoinSet<WorkResult>,
}

impl Hasher {
    pub fn new() -> Self {
        Self {
            set: JoinSet::new(),
        }
    }

    pub fn pend_job(&mut self, piece: Payload) {
        self.set.spawn_blocking(|| {
            let is_verified = piece.verify_hash();
            WorkResult {
                piece_i: piece.piece_i,
                is_verified,
                blocks: piece.data,
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
}

#[derive(Debug, Clone)]
pub struct Payload {
    // TODO: avoid hash copy
    pub hash: [u8; 20],
    pub piece_i: usize,
    pub data: Vec<Bytes>,
}

impl Payload {
    pub fn verify_hash(&self) -> bool {
        let start = Instant::now();
        let result = utils::verify_iter_sha1(&self.hash, self.data.iter());
        match result {
            true => {
                tracing::trace!(piece = self.piece_i, took = ?start.elapsed(), "Verified hash");
            }
            false => {
                tracing::error!(piece = self.piece_i, took = ?start.elapsed(), "Failed to verify hash");
            }
        }
        result
    }
}

#[derive(Debug, Clone)]
pub struct WorkResult {
    pub piece_i: usize,
    pub is_verified: bool,
    pub blocks: Vec<Bytes>,
}
