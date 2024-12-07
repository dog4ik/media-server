use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use tokio::{sync::mpsc, task::JoinSet};
use torrent::{DownloadHandle, DownloadParams, DownloadProgress, Info, MagnetLink, OutputFile};

use crate::{
    db::{Db, DbActions},
    library::{
        is_format_supported, movie::MovieIdentifier, show::ShowIdentifier, ContentIdentifier, Media,
    },
    metadata::{
        metadata_stack::MetadataProvidersStack, ContentType, EpisodeMetadata, MetadataProvider,
        MovieMetadata, ShowMetadata,
    },
    progress::{TaskResource, TorrentTask},
};

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PendingTorrent {
    pub info_hash: [u8; 20],
    #[serde(skip)]
    pub download_handle: DownloadHandle,
    pub torrent_info: TorrentInfo,
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
    async fn update_torrent(&self, hash: [u8; 20], bitfield: Vec<u8>) -> anyhow::Result<()>;
    async fn delete_torrent(&self, hash: [u8; 20]) -> anyhow::Result<()>;
}

impl TorrentManager for Db {
    async fn create_torrent(&self, params: DownloadParams) -> anyhow::Result<()> {
        self.insert_torrent(params.into()).await?;
        Ok(())
    }

    async fn read_torrents(&self) -> anyhow::Result<Vec<DownloadParams>> {
        Ok(self
            .all_torrents(100)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn update_torrent(&self, hash: [u8; 20], bitfield: Vec<u8>) -> anyhow::Result<()> {
        todo!()
    }

    async fn delete_torrent(&self, hash: [u8; 20]) -> anyhow::Result<()> {
        todo!()
    }
}

#[derive(Debug)]
pub struct TorrentProgress {
    torrent_hash: [u8; 20],
    progress: DownloadProgress,
}

#[derive(Debug)]
pub struct TorrentClient {
    pub client: torrent::Client,
    resolved_magnet_links: Mutex<HashMap<[u8; 20], Info>>,
    torrents: Arc<Mutex<Vec<PendingTorrent>>>,
    progress_tx: mpsc::Sender<TorrentProgress>,
}

async fn handle_progress(
    mut progress_rx: mpsc::Receiver<TorrentProgress>,
    torrents: Arc<Mutex<Vec<PendingTorrent>>>,
    tasks: &'static TaskResource,
) {
    while let Some(progress) = progress_rx.recv().await {
        let torrents = torrents.lock().unwrap();
        let pending_torrent = torrents
            .iter()
            .find(|t| t.info_hash == progress.torrent_hash);
        match pending_torrent {
            Some(_) => {}
            None => {
                let torrent_task = TorrentTask {
                    info_hash: progress.torrent_hash,
                    content: None,
                };
                let id = tasks
                    .start_task(torrent_task, None)
                    .expect("torrent is new");
            }
        }

        todo!();
    }
}

impl TorrentClient {
    pub async fn new(tasks: &'static TaskResource) -> anyhow::Result<Self> {
        let config = torrent::ClientConfig {
            cancellation_token: Some(tasks.parent_cancellation_token.clone()),
            ..Default::default()
        };
        let client = torrent::Client::new(config).await?;
        let (progress_tx, progress_rx) = mpsc::channel(100);
        let torrents = Arc::new(Mutex::new(Vec::new()));
        {
            let torrents = torrents.clone();
            tokio::spawn(handle_progress(progress_rx, torrents, tasks));
        }
        Ok(Self {
            client,
            resolved_magnet_links: HashMap::new().into(),
            torrents,
            progress_tx,
        })
    }

    pub async fn load_torrents(&mut self, manager: impl TorrentManager) -> anyhow::Result<()> {
        for torrent in manager.read_torrents().await? {
            let progress_tx = self.progress_tx.clone();
            let torrent_hash = torrent.info.hash();
            let progress_handler = move |progress: DownloadProgress| {
                let torrent_progress = TorrentProgress {
                    torrent_hash,
                    progress,
                };
                let _ = progress_tx.blocking_send(torrent_progress);
            };

            let Ok(handle) = self.client.open(torrent, progress_handler).await else {
                continue;
            };
        }
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
        torrent_metadata: TorrentInfo,
    ) -> anyhow::Result<TorrentHandle> {
        let info_hash = params.info.hash();
        let progress_tx = self.progress_tx.clone();
        let progress_handler = move |progress: DownloadProgress| {
            let torrent_progress = TorrentProgress {
                torrent_hash: info_hash,
                progress,
            };
            let _ = progress_tx.blocking_send(torrent_progress);
        };

        let download_handle = self.client.open(params, progress_handler).await?;

        let torrent = PendingTorrent {
            info_hash,
            download_handle,
            torrent_info: torrent_metadata,
        };
        let handle = torrent.handle();
        self.torrents.lock().unwrap().push(torrent);
        Ok(handle)
    }

    pub fn remove_download(&self, info_hash: [u8; 20]) -> Option<PendingTorrent> {
        let mut downloads = self.torrents.lock().unwrap();
        let download = downloads
            .iter()
            .position(|x| x.info_hash == info_hash)
            .map(|idx| downloads.swap_remove(idx));
        if let Some(download) = &download {
            download.download_handle.abort();
        }
        download
    }

    pub fn get_download(&self, info_hash: &[u8; 20]) -> Option<PendingTorrent> {
        let downloads = self.torrents.lock().unwrap();
        downloads
            .iter()
            .find(|x| x.info_hash == *info_hash)
            .cloned()
    }

    pub fn all_downloads(&self) -> Vec<TorrentInfo> {
        let downloads = self.torrents.lock().unwrap();
        downloads.iter().map(|d| d.torrent_info.clone()).collect()
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
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ResolvedTorrentFile {
    pub offset: u64,
    pub size: u64,
    pub path: Vec<String>,
    pub enabled: bool,
}

impl ResolvedTorrentFile {
    pub fn from_output_file(output_file: &OutputFile, offset: u64) -> Self {
        Self {
            offset,
            size: output_file.length(),
            path: path_components(output_file.path()),
            enabled: false,
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
    let mut offset = 0;
    for (i, output_file) in files.iter().enumerate() {
        let path = output_file.path().to_path_buf();
        let resolved_file = ResolvedTorrentFile::from_output_file(output_file, offset);
        let Some(file_name) = path.file_stem() else {
            tracing::warn!("Torrent file contains .dotfile: {}", path.display());
            all_files.push(resolved_file);
            offset += output_file.length();
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
        offset += output_file.length();
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
