use crate::progress::events::TorrentTickEvents;

pub mod consumer;
pub mod events;
pub mod full;

/// Stored progress from all active torrents dispatching the progress if any changed happened
#[derive(Debug)]
pub struct SessionTickProgressTracker {
    pub torrent_events: Vec<TorrentUpdate>,
}

#[derive(Debug)]
pub struct TorrentUpdate {
    pub events: TorrentTickEvents,
    /// Download speed per second
    pub download_speed: f64,
    /// Upload speed per second
    pub upload_speed: f64,
    pub total_downloaded: u64,
    pub total_uploaded: u64,
    pub state: crate::DownloadState,
    pub info_hash: [u8; 20],
}

#[derive(Debug, Default)]
pub struct Progress {
    pub session_update: Option<SessionUpdate>,
    pub changed_torrents: Vec<TorrentUpdate>,
    pub tick_num: usize,
}

#[derive(Debug, Default)]
pub struct SessionUpdate {
    pub connected_peers: u16,
    pub download_speed: f64,
    pub upload_speed: f64,
}
