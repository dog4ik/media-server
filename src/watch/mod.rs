use std::time::Duration;

pub mod torrent_stream;
pub mod transcode_stream;

#[derive(Debug, Clone, utoipa::ToSchema, serde::Serialize)]
pub struct WatchProgress {
    now_at: Duration,
    total_duration: Duration,
}

#[derive(Debug, Clone, utoipa::ToSchema, serde::Serialize)]
pub struct WatchTask {
    total_duration: Duration,
}
