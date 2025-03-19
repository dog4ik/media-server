use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use crate::{BitField, Priority, Status, TrackerStatus};

use super::DownloadState;

#[derive(Debug, Clone, Default)]
pub struct DownloadProgress {
    pub peers: Vec<PeerDownloadStats>,
    pub percent: f32,
    pub changes: Vec<StateChange>,
    pub tick_num: usize,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub enum PeerStateChange {
    Connect,
    Disconnect,
    InChoke(bool),
    OutChoke(bool),
    InInterested(bool),
    OutInterested(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub enum StateChange {
    FinishedPiece(usize),
    DownloadStateChange(DownloadState),
    TrackerAnnounce(String),
    FilePriorityChange {
        file_idx: usize,
        priority: Priority,
    },
    PeerStateChange {
        ip: SocketAddr,
        change: PeerStateChange,
    },
    ValidationResult {
        bitfield: Vec<u8>,
    },
}

#[derive(Debug)]
pub struct FullStateFile {
    pub path: std::path::PathBuf,
    pub size: u64,
    pub index: usize,
    pub start_piece: usize,
    pub end_piece: usize,
    pub priority: Priority,
}

#[derive(Debug)]
pub struct FullStatePeer {
    pub addr: SocketAddr,
    pub uploaded: u64,
    pub downloaded: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub in_status: Status,
    pub out_status: Status,
    pub interested_amount: usize,
    pub pending_blocks_amount: usize,
    pub client_name: String,
}

#[derive(Debug)]
pub struct FullStateTracker {
    pub url: String,
    pub last_announced_at: Instant,
    pub status: TrackerStatus,
    pub announce_interval: Duration,
}

#[derive(Debug)]
pub struct FullState {
    pub name: String,
    pub total_pieces: usize,
    pub percent: f32,
    pub total_size: u64,
    pub info_hash: [u8; 20],
    pub trackers: Vec<FullStateTracker>,
    pub peers: Vec<FullStatePeer>,
    pub files: Vec<FullStateFile>,
    pub bitfield: BitField,
    pub state: DownloadState,
    pub pending_pieces: Vec<usize>,
    pub tick_num: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PeerDownloadStats {
    pub ip: SocketAddr,
    pub downloaded: u64,
    pub uploaded: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub interested_amount: usize,
    pub pending_blocks_amount: usize,
}

#[derive(Debug, serde::Serialize, Default)]
pub struct TrackerStats {
    pub url: String,
    pub announce_interval: Duration,
    pub peers: Option<usize>,
    pub leechers: Option<usize>,
}

impl DownloadProgress {
    pub fn download_speed(&self) -> u64 {
        self.peers.iter().map(|p| p.download_speed).sum()
    }
}
pub trait ProgressConsumer: Send + 'static {
    fn consume_progress(&mut self, progress: DownloadProgress);
}

impl<F> ProgressConsumer for F
where
    F: FnMut(DownloadProgress) + Send + 'static,
{
    fn consume_progress(&mut self, progress: DownloadProgress) {
        self(progress);
    }
}

impl ProgressConsumer for std::sync::mpsc::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for tokio::sync::mpsc::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.try_send(progress);
    }
}

impl ProgressConsumer for tokio::sync::broadcast::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for tokio::sync::watch::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for flume::Sender<DownloadProgress> {
    fn consume_progress(&mut self, progress: DownloadProgress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for () {
    fn consume_progress(&mut self, _progress: DownloadProgress) {}
}
