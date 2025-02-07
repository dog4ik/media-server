use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::{sync::broadcast, task::JoinSet};
use torrent::{DownloadHandle, DownloadParams, Info, MagnetLink, OutputFile};

use crate::{
    db::{Db, DbActions, DbTorrentFile},
    library::{
        is_format_supported, movie::MovieIdentifier, show::ShowIdentifier, ContentIdentifier, Media,
    },
    metadata::{
        metadata_stack::MetadataProvidersStack, ContentType, EpisodeMetadata, MetadataProvider,
        MovieMetadata, ShowMetadata,
    },
    progress::{ProgressChunk, ProgressStatus, TaskResource, TaskTrait},
    utils,
};

#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema)]
pub struct Status {
    choked: bool,
    interested: bool,
}

impl From<torrent::Status> for Status {
    fn from(value: torrent::Status) -> Self {
        Self {
            choked: value.is_choked(),
            interested: value.is_interested(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    Disabled = 0,
    Low = 1,
    #[default]
    Medium = 2,
    High = 3,
}

impl TryFrom<usize> for Priority {
    type Error = anyhow::Error;

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Disabled),
            1 => Ok(Self::Low),
            2 => Ok(Self::Medium),
            3 => Ok(Self::High),
            _ => Err(anyhow::format_err!(
                "expected priority number to be 0..4, got {value}"
            )),
        }
    }
}

impl From<torrent::Priority> for Priority {
    fn from(value: torrent::Priority) -> Self {
        match value {
            torrent::Priority::Disabled => Self::Disabled,
            torrent::Priority::Low => Self::Low,
            torrent::Priority::Medium => Self::Medium,
            torrent::Priority::High => Self::High,
        }
    }
}

impl Into<torrent::Priority> for Priority {
    fn into(self) -> torrent::Priority {
        match self {
            Self::Disabled => torrent::Priority::Disabled,
            Self::Low => torrent::Priority::Low,
            Self::Medium => torrent::Priority::Medium,
            Self::High => torrent::Priority::High,
        }
    }
}

#[derive(Debug, Serialize, Clone, utoipa::ToSchema)]
pub struct StateFile {
    pub path: Vec<String>,
    pub start_piece: usize,
    pub end_piece: usize,
    pub size: u64,
    pub index: usize,
    pub priority: Priority,
}

impl From<torrent::FullStateFile> for StateFile {
    fn from(value: torrent::FullStateFile) -> Self {
        Self {
            index: value.index,
            size: value.size,
            start_piece: value.start_piece,
            end_piece: value.end_piece,
            path: path_components(value.path),
            priority: value.priority.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DownloadError {
    MissingFile,
}

impl From<torrent::DownloadError> for DownloadError {
    fn from(value: torrent::DownloadError) -> Self {
        match value {
            torrent::DownloadError::MissingFile => Self::MissingFile,
        }
    }
}

#[derive(Debug, Serialize, Clone, Copy, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Error(DownloadError),
    Validation,
    Paused,
    Pending,
    Seeding,
}

impl From<torrent::DownloadState> for DownloadState {
    fn from(value: torrent::DownloadState) -> Self {
        match value {
            torrent::DownloadState::Error(e) => Self::Error(e.into()),
            torrent::DownloadState::Validation => Self::Validation,
            torrent::DownloadState::Paused => Self::Paused,
            torrent::DownloadState::Pending => Self::Pending,
            torrent::DownloadState::Seeding => Self::Seeding,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StatePeer {
    pub addr: String,
    pub uploaded: u64,
    pub upload_speed: u64,
    pub downloaded: u64,
    pub download_speed: u64,
    pub in_status: Status,
    pub out_status: Status,
    pub interested_amount: usize,
    pub pending_blocks_amount: usize,
    pub client_name: &'static str,
}

impl From<torrent::FullStatePeer> for StatePeer {
    fn from(value: torrent::FullStatePeer) -> Self {
        Self {
            addr: value.addr.to_string(),
            uploaded: value.uploaded,
            upload_speed: value.upload_speed,
            downloaded: value.downloaded,
            download_speed: value.download_speed,
            in_status: value.in_status.into(),
            out_status: value.out_status.into(),
            interested_amount: value.interested_amount,
            pending_blocks_amount: value.pending_blocks_amount,
            client_name: value.client_name,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", tag = "status")]
pub enum TrackerStatus {
    Working,
    NotContacted,
    Error { message: String },
}

impl From<torrent::TrackerStatus> for TrackerStatus {
    fn from(value: torrent::TrackerStatus) -> Self {
        match value {
            torrent::TrackerStatus::Working => Self::Working,
            torrent::TrackerStatus::NotContacted => Self::NotContacted,
            torrent::TrackerStatus::Error(message) => Self::Error { message },
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct StateTracker {
    pub url: String,
    #[schema(value_type = crate::server::SerdeDuration)]
    pub announce_interval: Duration,
    #[serde(flatten)]
    pub status: TrackerStatus,
}

impl From<torrent::FullStateTracker> for StateTracker {
    fn from(value: torrent::FullStateTracker) -> Self {
        Self {
            url: value.url,
            announce_interval: value.announce_interval,
            status: value.status.into(),
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TorrentState {
    pub info_hash: String,
    pub name: String,
    pub total_pieces: usize,
    pub percent: f32,
    pub total_size: u64,
    pub trackers: Vec<StateTracker>,
    pub peers: Vec<StatePeer>,
    pub files: Vec<StateFile>,
    pub downloaded_pieces: Vec<bool>,
    pub state: DownloadState,
    pub pending_pieces: Vec<usize>,
    pub tick_num: usize,
}

impl From<torrent::FullState> for TorrentState {
    fn from(value: torrent::FullState) -> Self {
        let downloaded_pieces = value
            .bitfield
            .all_pieces(value.total_pieces)
            .into_iter()
            .collect();
        Self {
            info_hash: utils::stringify_info_hash(&value.info_hash),
            name: value.name,
            total_pieces: value.total_pieces,
            percent: value.percent,
            total_size: value.total_size,
            trackers: value.trackers.into_iter().map(Into::into).collect(),
            peers: value.peers.into_iter().map(Into::into).collect(),
            files: value.files.into_iter().map(Into::into).collect(),
            downloaded_pieces,
            state: value.state.into(),
            pending_pieces: value.pending_pieces,
            tick_num: value.tick_num,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "change_type", content = "value", rename_all = "lowercase")]
pub enum PeerStateChange {
    Connect,
    Disconnect,
    InChoke(bool),
    OutChoke(bool),
    InInterested(bool),
    OutInterested(bool),
}

impl From<torrent::PeerStateChange> for PeerStateChange {
    fn from(value: torrent::PeerStateChange) -> Self {
        match value {
            torrent::PeerStateChange::Connect => Self::Connect,
            torrent::PeerStateChange::Disconnect => Self::Disconnect,
            torrent::PeerStateChange::InChoke(v) => Self::InChoke(v),
            torrent::PeerStateChange::OutChoke(v) => Self::OutChoke(v),
            torrent::PeerStateChange::InInterested(v) => Self::InInterested(v),
            torrent::PeerStateChange::OutInterested(v) => Self::OutInterested(v),
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "type", content = "change", rename_all = "lowercase")]
pub enum StateChange {
    PeerStateChange {
        #[schema(value_type = String)]
        ip: std::net::SocketAddr,
        peer_change: PeerStateChange,
    },
    FinishedPiece(usize),
    DownloadStateChange(DownloadState),
    TrackerAnnounce(String),
    FilePriorityChange {
        file_idx: usize,
        priority: Priority,
    },
    ValidationResult {
        bitfield: Vec<u8>,
    },
}

impl From<torrent::StateChange> for StateChange {
    fn from(value: torrent::StateChange) -> Self {
        match value {
            torrent::StateChange::PeerStateChange { ip, change } => Self::PeerStateChange {
                ip,
                peer_change: change.into(),
            },
            torrent::StateChange::FinishedPiece(p) => Self::FinishedPiece(p),
            torrent::StateChange::DownloadStateChange(s) => Self::DownloadStateChange(s.into()),
            torrent::StateChange::TrackerAnnounce(t) => Self::TrackerAnnounce(t),
            torrent::StateChange::FilePriorityChange { file_idx, priority } => {
                Self::FilePriorityChange {
                    file_idx,
                    priority: priority.into(),
                }
            }
            torrent::StateChange::ValidationResult { bitfield } => {
                Self::ValidationResult { bitfield }
            }
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct PeerDownloadStats {
    #[schema(value_type = String)]
    pub ip: std::net::SocketAddr,
    pub downloaded: u64,
    pub uploaded: u64,
    pub download_speed: u64,
    pub upload_speed: u64,
    pub interested_amount: usize,
    pub pending_blocks_amount: usize,
}

impl From<torrent::PeerDownloadStats> for PeerDownloadStats {
    fn from(value: torrent::PeerDownloadStats) -> Self {
        Self {
            ip: value.ip,
            downloaded: value.downloaded,
            uploaded: value.uploaded,
            download_speed: value.download_speed,
            upload_speed: value.upload_speed,
            interested_amount: value.interested_amount,
            pending_blocks_amount: value.pending_blocks_amount,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DownloadProgress {
    pub peers: Vec<PeerDownloadStats>,
    pub percent: f32,
    pub tick_num: usize,
    pub changes: Vec<StateChange>,
}

impl DownloadProgress {
    pub fn download_speed(&self) -> u64 {
        self.peers.iter().map(|p| p.download_speed).sum()
    }
}

impl From<torrent::DownloadProgress> for DownloadProgress {
    fn from(value: torrent::DownloadProgress) -> Self {
        let changes = value.changes.into_iter().map(Into::into).collect();
        let peers = value.peers.into_iter().map(Into::into).collect();
        Self {
            peers,
            percent: value.percent,
            tick_num: value.tick_num,
            changes,
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PendingTorrent {
    pub info_hash: [u8; 20],
    #[serde(skip)]
    pub download_handle: DownloadHandle,
    pub torrent_info: TorrentInfo,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema, PartialEq)]
pub struct TorrentTask {
    /// Hex encoded info hash
    pub info_hash: String,
}

impl TorrentTask {
    pub fn new(info_hash: &[u8; 20]) -> Self {
        Self {
            info_hash: utils::stringify_info_hash(info_hash),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, utoipa::ToSchema, PartialEq)]
pub struct CompactTorrentProgress {
    percent: f32,
    peers_amount: usize,
    download_speed: u64,
}

impl CompactTorrentProgress {
    pub fn new(progress: &DownloadProgress) -> Self {
        Self {
            percent: progress.percent as f32,
            peers_amount: progress.peers.len(),
            download_speed: progress.download_speed(),
        }
    }
}

impl TaskTrait for PendingTorrent {
    type Identifier = TorrentTask;

    type Progress = CompactTorrentProgress;

    fn identifier(&self) -> Self::Identifier {
        TorrentTask::new(&self.info_hash)
    }

    fn into_progress(chunk: crate::progress::ProgressChunk<Self>) -> crate::progress::TaskProgress
    where
        Self: Sized,
    {
        crate::progress::TaskProgress::Torrent(chunk)
    }
}

impl PartialEq for PendingTorrent {
    fn eq(&self, other: &Self) -> bool {
        self.info_hash == other.info_hash
    }
}

impl PendingTorrent {
    pub fn handle(&self) -> TorrentHandle {
        TorrentHandle {
            download_handle: self.download_handle.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TorrentHandle {
    pub download_handle: DownloadHandle,
}

#[allow(async_fn_in_trait)]
pub trait TorrentManager {
    async fn create_torrent(&self, params: DownloadParams) -> anyhow::Result<()>;
    async fn read_torrents(&self) -> anyhow::Result<Vec<DownloadParams>>;
    async fn update_torrent(&self, hash: &[u8; 20], new_pieces: &[usize]) -> anyhow::Result<()>;
    async fn update_pieces(&self, hash: &[u8; 20], bitfield: &[u8]) -> anyhow::Result<()>;
    async fn delete_torrent(&self, hash: &[u8; 20]) -> anyhow::Result<()>;
    async fn update_file_priority(
        &self,
        hash: &[u8; 20],
        file_idx: usize,
        priority: torrent::Priority,
    ) -> anyhow::Result<()>;
}

impl TorrentManager for Db {
    async fn create_torrent(&self, params: DownloadParams) -> anyhow::Result<()> {
        let mut tx = self.begin().await?;
        let torrent_id = tx.insert_torrent(params.clone().into()).await?;
        for (i, file) in params.info.output_files("").iter().enumerate() {
            let path = file.path().to_string_lossy();
            let db_file = DbTorrentFile {
                id: None,
                torrent_id,
                priority: params.files[i] as usize as i64,
                idx: i as i64,
                relative_path: path.to_string(),
            };
            tx.insert_torrent_file(db_file).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn read_torrents(&self) -> anyhow::Result<Vec<DownloadParams>> {
        let mut downloads = Vec::new();
        for torrent in self.all_torrents(100).await? {
            let files = self.torrent_files(torrent.id.unwrap()).await?;
            let bitfield = torrent::BitField::from(torrent.bitfield);
            tracing::debug!("Loaded bitfield with {} pieces", bitfield.pieces().count());
            let info = torrent::Info::from_bytes(&torrent.bencoded_info)
                .expect("we don't screw up saving it");
            let trackers = torrent
                .trackers
                .split(',')
                .filter_map(|t| t.parse().ok())
                .collect();
            downloads.push(DownloadParams {
                bitfield,
                info,
                trackers,
                files: files
                    .iter()
                    .map(|f| Priority::try_from(f.priority as usize).unwrap().into())
                    .collect(),
                save_location: torrent.save_location.into(),
            })
        }
        Ok(downloads)
    }

    async fn update_torrent(&self, hash: &[u8; 20], new_pieces: &[usize]) -> anyhow::Result<()> {
        // BUG: This code introduces race condition.
        // ensure that it is called not in parallel
        let torrent = self.get_torrent_by_info_hash(hash).await?;
        let mut bf = torrent::BitField(torrent.bitfield);
        for piece in new_pieces {
            bf.add(*piece).unwrap();
        }
        tracing::debug!("Applying {} pieces to bitfield", new_pieces.len());
        self.update_torrent_by_info_hash(hash, &bf.0).await?;
        Ok(())
    }

    async fn update_pieces(&self, hash: &[u8; 20], bitfield: &[u8]) -> anyhow::Result<()> {
        tracing::debug!("Saving torrent bitfield");
        self.update_torrent_by_info_hash(hash, bitfield).await?;
        Ok(())
    }

    async fn delete_torrent(&self, hash: &[u8; 20]) -> anyhow::Result<()> {
        self.remove_torrent(hash).await?;
        Ok(())
    }

    async fn update_file_priority(
        &self,
        hash: &[u8; 20],
        file_idx: usize,
        priority: torrent::Priority,
    ) -> anyhow::Result<()> {
        let hash = &hash[..];
        let torrent_id = sqlx::query!("SELECT id FROM torrents WHERE info_hash = ?;", hash)
            .fetch_one(&self.pool)
            .await?
            .id;
        let priority = priority as usize as i64;
        let idx = file_idx as i64;
        sqlx::query!(
            "UPDATE torrent_files SET priority = ? WHERE torrent_id = ? AND idx = ?",
            priority,
            torrent_id,
            idx,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TorrentProgress {
    pub torrent_hash: [u8; 20],
    #[serde(flatten)]
    pub progress: Progress,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", tag = "type")]
pub enum Progress {
    Start,
    Pending(DownloadProgress),
    Delete,
}

#[derive(Debug, Clone)]
pub struct TorrentProgressChannel(broadcast::Sender<Arc<TorrentProgress>>);

impl TorrentProgressChannel {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(20);
        Self(tx)
    }

    pub fn send(&self, progress: TorrentProgress) {
        let _ = self.0.send(Arc::new(progress));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<TorrentProgress>> {
        self.0.subscribe()
    }
}

#[derive(Debug)]
pub struct TorrentClient {
    pub client: torrent::Client,
    resolved_magnet_links: Mutex<HashMap<[u8; 20], Info>>,
    torrents: Arc<Mutex<Vec<PendingTorrent>>>,
    pub progress_broadcast: TorrentProgressChannel,
    manager: Db,
}

pub fn progress_handler(
    torrent_hash: [u8; 20],
    broadcast: TorrentProgressChannel,
) -> impl torrent::ProgressConsumer {
    move |progress: torrent::DownloadProgress| {
        //if progress.changes.is_empty() {
        //    return;
        //}
        broadcast.send(TorrentProgress {
            torrent_hash,
            progress: Progress::Pending(progress.into()),
        });
    }
}

async fn handle_progress(
    progress_broadcast: TorrentProgressChannel,
    tasks: &'static TaskResource,
    torrents: Arc<Mutex<Vec<PendingTorrent>>>,
    manager: impl TorrentManager,
) {
    let mut sub = progress_broadcast.subscribe();
    while let Ok(chunk) = sub.recv().await.as_deref() {
        let TorrentProgress {
            torrent_hash,
            progress: Progress::Pending(progress),
        } = chunk
        else {
            continue;
        };
        let mut new_pieces = Vec::new();
        for change in &progress.changes {
            match change {
                StateChange::FinishedPiece(piece) => new_pieces.push(*piece),
                StateChange::ValidationResult { bitfield } => {
                    if let Err(e) = manager.update_pieces(&torrent_hash, bitfield).await {
                        tracing::error!("Failed to save torrent validation result: {e}");
                        continue;
                    }
                }
                _ => {}
            }
        }

        if !new_pieces.is_empty() {
            if let Err(e) = manager.update_torrent(&torrent_hash, &new_pieces).await {
                tracing::error!("Failed to update torrent state: {e}");
                continue;
            };
        }
        let mut torrents = torrents.lock().unwrap();
        let Some(pending_torrent) = torrents.iter_mut().find(|t| t.info_hash == *torrent_hash)
        else {
            tracing::error!(
                "Torrent with info_hash {} is not found",
                utils::stringify_info_hash(&torrent_hash)
            );
            continue;
        };
        let ident = pending_torrent.identifier();
        let progress = CompactTorrentProgress::new(&progress);
        let progress_chunk = ProgressChunk {
            identifier: ident,
            status: ProgressStatus::Pending { progress },
        };
        tasks
            .torrent_tasks
            // fix this crap
            .send_progress(uuid::Uuid::new_v4(), progress_chunk);
    }
}

impl TorrentClient {
    pub async fn new(tasks: &'static TaskResource, manager: Db) -> anyhow::Result<Self> {
        let config = torrent::ClientConfig {
            cancellation_token: Some(tasks.parent_cancellation_token.clone()),
            ..Default::default()
        };
        let client = torrent::Client::new(config).await?;
        let progress_broadcast = TorrentProgressChannel::new();
        let torrents = Arc::new(Mutex::new(Vec::new()));
        tokio::spawn(handle_progress(
            progress_broadcast.clone(),
            tasks,
            torrents.clone(),
            manager.clone(),
        ));
        Ok(Self {
            client,
            resolved_magnet_links: Default::default(),
            progress_broadcast,
            torrents,
            manager,
        })
    }

    pub async fn load_torrents(&self) -> anyhow::Result<()> {
        let start = Instant::now();
        let mut count = 0;
        for torrent in self.manager.read_torrents().await? {
            let progress_handler =
                progress_handler(torrent.info.hash(), self.progress_broadcast.clone());

            let mut files = Vec::new();
            let mut file_offset = 0;
            for (i, file) in torrent
                .info
                .output_files(&torrent.save_location)
                .iter()
                .enumerate()
            {
                let mut resolved_file = ResolvedTorrentFile::from_output_file(&file, file_offset);
                resolved_file.priority = torrent.files[i].into();
                files.push(resolved_file);
                file_offset += file.length();
            }

            let torrent_info = TorrentInfo {
                name: torrent.info.name.clone(),
                contents: TorrentContents::without_content(files),
                piece_length: torrent.info.piece_length,
                pieces_amount: torrent.info.pieces.len(),
                total_size: torrent.info.total_size(),
            };
            let info_hash = torrent.info.hash();

            match self.client.open(torrent, progress_handler).await {
                Ok(download_handle) => {
                    let torrent = PendingTorrent {
                        info_hash,
                        download_handle,
                        torrent_info,
                    };
                    self.torrents.lock().unwrap().push(torrent);
                    count += 1;
                }
                Err(e) => {
                    tracing::error!("Failed to open torrent: {e}");
                    continue;
                }
            };
        }

        tracing::info!(took = ?start.elapsed(), "Loaded {count} torrents");
        Ok(())
    }

    pub async fn resolve_magnet_link(&self, magnet_link: &MagnetLink) -> anyhow::Result<Info> {
        let hash = magnet_link.hash();
        if let Ok(Some(info)) = self
            .resolved_magnet_links
            .lock()
            .map(|s| s.get(&hash).cloned())
        {
            tracing::debug!("Resolved cached magnet link: {}", magnet_link.to_string());
            return Ok(info);
        };
        let info = self.client.resolve_magnet_link(magnet_link).await?;
        tracing::debug!("Resolved magnet link: {}", magnet_link.to_string());

        self.resolved_magnet_links
            .lock()
            .unwrap()
            .insert(hash, info.clone());
        Ok(info)
    }

    pub async fn add_torrent(
        &self,
        params: DownloadParams,
        torrent_info: TorrentInfo,
    ) -> anyhow::Result<TorrentHandle> {
        self.manager.create_torrent(params.clone()).await?;
        let info_hash = params.info.hash();
        let progress_handler = progress_handler(info_hash, self.progress_broadcast.clone());

        let download_handle = self.client.open(params.clone(), progress_handler).await?;

        let torrent = PendingTorrent {
            info_hash,
            download_handle,
            torrent_info,
        };
        let handle = torrent.handle();
        self.torrents.lock().unwrap().push(torrent);
        self.progress_broadcast.send(TorrentProgress {
            torrent_hash: info_hash,
            progress: Progress::Start,
        });
        Ok(handle)
    }

    pub async fn remove_download(&self, info_hash: [u8; 20]) -> Option<PendingTorrent> {
        let download = {
            let mut downloads = self.torrents.lock().unwrap();
            downloads
                .iter()
                .position(|x| x.info_hash == info_hash)
                .map(|idx| downloads.swap_remove(idx))
        };
        if let Some(download) = &download {
            download.download_handle.abort();
            self.progress_broadcast.send(TorrentProgress {
                torrent_hash: info_hash,
                progress: Progress::Delete,
            });
            if let Err(e) = self.manager.delete_torrent(&info_hash).await {
                tracing::error!("Failed to remove torrent: {e}");
            };
        }
        download
    }

    pub fn get_download(&self, info_hash: &[u8; 20]) -> Option<PendingTorrent> {
        self.torrents
            .lock()
            .unwrap()
            .iter()
            .find(|x| x.info_hash == *info_hash)
            .cloned()
    }

    pub async fn update_file_priority(
        &self,
        info_hash: &[u8; 20],
        idx: usize,
        priority: torrent::Priority,
    ) -> anyhow::Result<()> {
        self.manager
            .update_file_priority(info_hash, idx, priority)
            .await?;
        let mut torrents = self.torrents.lock().unwrap();
        let torrent = torrents
            .iter_mut()
            .find(|x| x.info_hash == *info_hash)
            .context("get torrent")?;

        let file = torrent
            .torrent_info
            .contents
            .files
            .get_mut(idx)
            .context("get torrent file")?;
        file.priority = priority.into();
        Ok(())
    }

    pub async fn all_downloads(&self) -> Vec<TorrentState> {
        use tokio::sync::oneshot;
        let recvs: Vec<_> = self
            .torrents
            .lock()
            .unwrap()
            .iter()
            .filter_map(|d| {
                let (tx, rx) = oneshot::channel();
                d.download_handle
                    .download_tx
                    .try_send(torrent::DownloadMessage::PostFullState { tx })
                    .ok()?;
                Some(rx)
            })
            .collect();
        let mut torrents = Vec::new();
        for rx in recvs {
            if let Ok(val) = rx.await {
                torrents.push(val.into());
            }
        }
        torrents
    }

    pub async fn full_progress(&self, info_hash: &[u8; 20]) -> Option<TorrentState> {
        let download = self.get_download(info_hash)?;
        download
            .download_handle
            .full_state()
            .await
            .ok()
            .map(Into::into)
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TorrentInfo {
    pub name: String,
    pub contents: TorrentContents,
    pub piece_length: u32,
    pub pieces_amount: usize,
    pub total_size: u64,
}

impl TorrentInfo {
    pub async fn new(
        info: &Info,
        content_type_hint: Option<DownloadContentHint>,
        providers_stack: &'static MetadataProvidersStack,
    ) -> Self {
        let all_files = info.output_files("");
        let files = parse_torrent_files(providers_stack, &all_files, content_type_hint).await;

        TorrentInfo {
            contents: files,
            name: info.name.clone(),
            piece_length: info.piece_length,
            pieces_amount: info.pieces.len(),
            total_size: info.total_size(),
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct DownloadContentHint {
    pub content_type: ContentType,
    pub metadata_provider: MetadataProvider,
    pub metadata_id: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TorrentDownloadPayload {
    pub save_location: Option<String>,
    pub content_hint: Option<DownloadContentHint>,
    pub enabled_files: Option<Vec<usize>>,
    pub magnet_link: String,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct ResolveMagnetLinkPayload {
    pub magnet_link: String,
    pub hint: Option<DownloadContentHint>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ResolvedTorrentFile {
    pub offset: u64,
    pub size: u64,
    pub path: Vec<String>,
    pub priority: Priority,
}

impl ResolvedTorrentFile {
    pub fn from_output_file(output_file: &OutputFile, offset: u64) -> Self {
        Self {
            offset,
            size: output_file.length(),
            path: path_components(output_file.path()),
            priority: Priority::Disabled,
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TorrentMovie {
    pub file_idx: usize,
    pub metadata: MovieMetadata,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TorrentEpisode {
    pub file_idx: usize,
    pub metadata: EpisodeMetadata,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TorrentShow {
    pub show_metadata: ShowMetadata,
    pub seasons: HashMap<u16, Vec<TorrentEpisode>>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TorrentContent {
    Show(TorrentShow),
    Movie(Vec<TorrentMovie>),
}

impl TorrentContent {
    pub fn content_type(&self) -> ContentType {
        match self {
            TorrentContent::Show(_) => ContentType::Show,
            TorrentContent::Movie(_) => ContentType::Movie,
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct TorrentContents {
    pub files: Vec<ResolvedTorrentFile>,
    pub content: Option<TorrentContent>,
}

impl TorrentContents {
    pub fn without_content(other_files: Vec<ResolvedTorrentFile>) -> Self {
        Self {
            content: None,
            files: other_files,
        }
    }
}

fn path_components(path: impl AsRef<Path>) -> Vec<String> {
    let mut out = Vec::new();
    for component in path.as_ref().components() {
        if let std::path::Component::Normal(component) = component {
            out.push(component.to_string_lossy().to_string())
        }
    }
    out
}

async fn parse_torrent_files(
    providers_stack: &'static MetadataProvidersStack,
    files: &[OutputFile],
    content_hint: Option<DownloadContentHint>,
) -> TorrentContents {
    let mut all_files: Vec<ResolvedTorrentFile> = Vec::new();
    let mut show_identifiers: Vec<(usize, ShowIdentifier)> = Vec::new();
    let mut movie_identifiers: Vec<(usize, MovieIdentifier)> = Vec::new();
    let mut file_offset = 0;
    for (i, output_file) in files.iter().enumerate() {
        let path = output_file.path().to_path_buf();
        let resolved_file = ResolvedTorrentFile::from_output_file(output_file, file_offset);
        let Some(file_name) = path.file_stem() else {
            tracing::warn!("Torrent file contains .dotfile: {}", path.display());
            all_files.push(resolved_file);
            file_offset += output_file.length();
            continue;
        };
        if is_format_supported(&path) {
            let content_identifier = match content_hint.as_ref().map(|h| h.content_type) {
                None => ShowIdentifier::from_path(file_name)
                    .map(Into::into)
                    .or_else(|_| MovieIdentifier::from_path(file_name).map(Into::into))
                    .ok(),
                Some(ContentType::Movie) => {
                    MovieIdentifier::from_path(file_name).map(Into::into).ok()
                }
                Some(ContentType::Show) => {
                    ShowIdentifier::from_path(file_name).map(Into::into).ok()
                }
            };
            match content_identifier {
                Some(ContentIdentifier::Show(s)) => show_identifiers.push((i, s)),
                Some(ContentIdentifier::Movie(m)) => movie_identifiers.push((i, m)),
                None => {}
            }
        }
        all_files.push(resolved_file);
        file_offset += output_file.length();
    }

    if show_identifiers.is_empty() && movie_identifiers.is_empty() {
        return TorrentContents::without_content(all_files);
    };

    let content_type = if show_identifiers.is_empty() {
        ContentType::Movie
    } else {
        ContentType::Show
    };

    match content_type {
        ContentType::Show => {
            let show_title = show_identifiers.first().unwrap().1.title();
            let mut seasons_map: HashMap<u16, Vec<TorrentEpisode>> = HashMap::new();
            let show = match &content_hint {
                Some(hint) => {
                    match providers_stack
                        .get_show(&hint.metadata_id, hint.metadata_provider)
                        .await
                    {
                        Ok(show) => show,
                        Err(_) => {
                            tracing::warn!("Failed to fetch show from content_hint");
                            let Ok(Some(show)) = providers_stack
                                .search_show(show_title)
                                .await
                                .map(|r| r.into_iter().next())
                            else {
                                tracing::error!("Could not find show: {}", show_title);
                                return TorrentContents::without_content(all_files);
                            };
                            show
                        }
                    }
                }
                None => {
                    let Ok(Some(show)) = providers_stack
                        .search_show(show_title)
                        .await
                        .map(|x| x.into_iter().next())
                    else {
                        tracing::error!("Could not find show: {}", show_title);
                        return TorrentContents::without_content(all_files);
                    };
                    show
                }
            };

            // NOTE: We need external provider because not all episodes can be available locally
            let (show_id, show_metadata_provider) = if show.metadata_provider
                == MetadataProvider::Local
            {
                let Ok(external_ids) = providers_stack
                    .get_external_ids(&show.metadata_id, ContentType::Show, show.metadata_provider)
                    .await
                else {
                    tracing::error!("External ids are not found while resolving local entry");
                    return TorrentContents::without_content(all_files);
                };
                let Some(tmdb_id) = external_ids
                    .into_iter()
                    .find(|x| matches!(x.provider, MetadataProvider::Tmdb))
                else {
                    tracing::error!("External tmdb id is not found while resolving local entry");
                    return TorrentContents::without_content(all_files);
                };
                (tmdb_id.id, tmdb_id.provider)
            } else {
                (show.metadata_id.clone(), show.metadata_provider)
            };

            show_identifiers.sort_by_key(|x| x.1.season);
            let mut season_set = JoinSet::new();
            for chunk in show_identifiers
                .chunk_by(|(_, a), (_, b)| a.season == b.season)
                .map(Vec::from)
            {
                let season = chunk.first().unwrap().1.season;
                seasons_map.insert(season, Vec::new());
                let show_id = show_id.clone();
                season_set.spawn(async move {
                    let resolved_season = providers_stack
                        .get_season(&show_id, season as usize, show_metadata_provider)
                        .await;
                    (resolved_season, chunk)
                });
            }
            while let Some(Ok((resolved_season, chunk))) = season_set.join_next().await {
                let season = chunk.first().unwrap().1.season;
                for (file_idx, episode) in chunk.into_iter() {
                    let metadata = resolved_season
                        .as_ref()
                        .ok()
                        .and_then(|s| {
                            s.episodes
                                .iter()
                                .find(|e| e.number == episode.episode as usize)
                                .cloned()
                        })
                        .unwrap_or(EpisodeMetadata {
                            metadata_id: uuid::Uuid::new_v4().to_string(),
                            metadata_provider: MetadataProvider::Local,
                            number: episode.episode as usize,
                            title: episode.title,
                            season_number: episode.season as usize,
                            ..Default::default()
                        });
                    let episodes = seasons_map.get_mut(&season).expect("Map to be populated");
                    episodes.push(TorrentEpisode { file_idx, metadata })
                }
            }
            for episodes in seasons_map.values_mut() {
                episodes.sort_unstable_by_key(|x| x.metadata.number);
            }
            TorrentContents {
                files: all_files,
                content: Some(TorrentContent::Show(TorrentShow {
                    show_metadata: show,
                    seasons: seasons_map,
                })),
            }
        }
        ContentType::Movie => {
            let mut resolved_movies = Vec::new();
            for (file_idx, movie) in movie_identifiers {
                if let Some(movie) = providers_stack
                    .search_movie(movie.title())
                    .await
                    .ok()
                    .and_then(|r| r.into_iter().next())
                {
                    resolved_movies.push(TorrentMovie {
                        file_idx,
                        metadata: movie,
                    });
                };
            }
            if resolved_movies.is_empty() {
                TorrentContents {
                    files: all_files,
                    content: None,
                }
            } else {
                TorrentContents {
                    files: all_files,
                    content: Some(TorrentContent::Movie(resolved_movies)),
                }
            }
        }
    }
}
