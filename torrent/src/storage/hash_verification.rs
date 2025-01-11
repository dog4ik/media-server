use std::time::Instant;

use bytes::Bytes;
use tokio::sync::mpsc;

use crate::utils;

#[derive(Debug)]
pub struct Worker {
    sender: mpsc::Sender<Payload>,
    load: usize,
}

impl Worker {
    pub fn new(idx: usize, result_tx: mpsc::Sender<WorkResult>) -> Self {
        let (tx, rx) = mpsc::channel(100);
        tokio::task::spawn_blocking(move || worker(idx, rx, result_tx));

        Self {
            sender: tx,
            load: 0,
        }
    }

    pub async fn assign(&mut self, payload: Payload) {
        self.sender.send(payload).await.expect("worker is alive");
        self.load += 1;
    }
}

#[derive(Debug)]
pub struct Hasher {
    workers: Vec<Worker>,
    result_rx: mpsc::Receiver<WorkResult>,
}

impl Hasher {
    pub fn new(workers_amount: usize) -> Self {
        debug_assert!(workers_amount > 0);
        tracing::info!("Spawning {} hasher workers", workers_amount);
        let (result_tx, result_rx) = mpsc::channel(100);
        let workers = (0..workers_amount)
            .into_iter()
            .map(|i| Worker::new(i, result_tx.clone()))
            .collect();
        Self { workers, result_rx }
    }

    pub async fn pend_job(&mut self, piece: Payload) {
        let worker = self
            .workers
            .iter_mut()
            .min_by_key(|w| w.load)
            .expect("workers are never empty");
        worker.assign(piece).await
    }

    /// Cancellation safe
    pub async fn recv(&mut self) -> WorkResult {
        let result = self.result_rx.recv().await.unwrap();
        let worker = &mut self.workers[result.worker_idx];
        worker.load -= 1;
        result
    }
}

#[derive(Debug, Clone)]
pub struct Payload {
    pub hash: [u8; 20],
    pub piece_i: usize,
    pub data: Vec<Bytes>,
}

impl Payload {
    pub fn verify_hash(&self, idx: usize) -> bool {
        let start = Instant::now();
        let result = utils::verify_iter_sha1(&self.hash, self.data.iter());
        match result {
            true => {
                tracing::trace!(piece = self.piece_i, took = ?start.elapsed(), "Worker {idx} Verified hash");
            }
            false => {
                tracing::error!(piece = self.piece_i, took = ?start.elapsed(), "Worker {idx} failed to verify hash");
            }
        }
        result
    }
}

#[derive(Debug, Clone)]
pub struct WorkResult {
    pub piece_i: usize,
    worker_idx: usize,
    pub is_verified: bool,
    pub piece: Vec<Bytes>,
}

fn worker(idx: usize, mut work_rx: mpsc::Receiver<Payload>, result_tx: mpsc::Sender<WorkResult>) {
    while let Some(work) = work_rx.blocking_recv() {
        let is_verified = work.verify_hash(idx);
        let _ = result_tx.try_send(WorkResult {
            worker_idx: idx,
            piece_i: work.piece_i,
            is_verified,
            piece: work.data,
        });
    }
}
