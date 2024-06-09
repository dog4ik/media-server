use std::{ops::Range, path::PathBuf};

use torrent::download::DownloadHandle;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TorrentFile {
    pub range: Range<u64>,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct TorrentDownload {
    pub uuid: Uuid,
    pub info_hash: [u8; 20],
    pub piece_size: u32,
    pub download_handle: DownloadHandle,
    pub files: Vec<TorrentFile>,
}
