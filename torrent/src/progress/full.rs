use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use crate::{BitField, Priority, Status, TrackerStatus};

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
pub struct FullSessionStats {
    pub download_speed: f64,
    pub upload_speed: f64,
    pub connected_peers: u16,
}

#[derive(Debug)]
pub struct FullSessionState {
    pub session_stats: FullSessionStats,
    pub torrents: Vec<FullState>,
}

#[derive(Debug)]
pub struct FullState {
    pub name: String,
    pub total_pieces: usize,
    pub percent: f32,
    pub download_speed: f64,
    pub upload_speed: f64,
    pub total_size: u64,
    pub info_hash: [u8; 20],
    pub trackers: Vec<FullStateTracker>,
    pub peers: Vec<FullStatePeer>,
    pub files: Vec<FullStateFile>,
    pub bitfield: BitField,
    pub state: crate::DownloadState,
    pub pending_pieces: Vec<usize>,
}
