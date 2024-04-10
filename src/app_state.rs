use std::{fmt::Display, num::ParseIntError, path::PathBuf, str::FromStr, sync::Mutex};

use anyhow::anyhow;
use axum::{extract::FromRef, http::StatusCode, response::IntoResponse, Json};
use tokio::{fs, task::JoinSet};
use tokio_util::sync::CancellationToken;
use torrent::Torrent;

use crate::{
    config::ServerConfiguration,
    db::{Db, DbExternalId, DbSubtitles},
    library::{movie::MovieIdentifier, Library, LibraryFile, Source, TranscodePayload, Video},
    metadata::{
        tmdb_api::TmdbApi, ContentType, DiscoverMetadataProvider, ExternalIdMetadata,
        MetadataProvider, MetadataProvidersStack, ShowMetadataProvider,
    },
    progress::{TaskKind, TaskResource},
    torrent_index::tpb::TpbApi,
    utils,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub library: &'static Mutex<Library>,
    pub db: &'static Db,
    pub tasks: TaskResource,
    pub configuration: &'static Mutex<ServerConfiguration>,
    pub tmdb_api: &'static TmdbApi,
    pub tpb_api: &'static TpbApi,
    pub providers_stack: &'static MetadataProvidersStack,
    pub torrent_client: &'static torrent::Client,
    pub cancelation_token: CancellationToken,
}

#[derive(Debug, Clone)]
pub struct AppError {
    pub message: String,
    pub kind: AppErrorKind,
}

#[derive(Debug, Clone)]
pub enum AppErrorKind {
    InternalError,
    NotFound,
    Duplicate,
    BadRequest,
}

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            AppErrorKind::InternalError => write!(f, "Internal Error: {}", self.message),
            AppErrorKind::NotFound => write!(f, "Not Found Error: {}", self.message),
            AppErrorKind::Duplicate => write!(f, "Dublicate Error: {}", self.message),
            AppErrorKind::BadRequest => write!(f, "Bad Request: {}", self.message),
        }
    }
}

impl Into<StatusCode> for AppErrorKind {
    fn into(self) -> StatusCode {
        match self {
            AppErrorKind::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            AppErrorKind::NotFound => StatusCode::NOT_FOUND,
            AppErrorKind::Duplicate => StatusCode::BAD_REQUEST,
            AppErrorKind::BadRequest => StatusCode::BAD_REQUEST,
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            message: err.to_string(),
            kind: AppErrorKind::InternalError,
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(value: sqlx::Error) -> Self {
        match value {
            sqlx::Error::RowNotFound => AppError {
                message: format!("Database row not found"),
                kind: AppErrorKind::NotFound,
            },
            rest => AppError {
                message: format!("{}", rest),
                kind: AppErrorKind::InternalError,
            },
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        match value.kind() {
            std::io::ErrorKind::NotFound => AppError {
                message: value.to_string(),
                kind: AppErrorKind::NotFound,
            },
            _ => AppError {
                message: value.to_string(),
                kind: AppErrorKind::InternalError,
            },
        }
    }
}

impl From<ParseIntError> for AppError {
    fn from(value: ParseIntError) -> Self {
        AppError {
            message: value.to_string(),
            kind: AppErrorKind::BadRequest,
        }
    }
}

impl AppError {
    pub fn new(message: impl AsRef<str>, kind: AppErrorKind) -> Self {
        Self {
            message: message.as_ref().into(),
            kind,
        }
    }

    pub fn not_found(msg: impl AsRef<str>) -> AppError {
        AppError {
            message: msg.as_ref().into(),
            kind: AppErrorKind::NotFound,
        }
    }

    pub fn bad_request(msg: impl AsRef<str>) -> AppError {
        AppError {
            message: msg.as_ref().into(),
            kind: AppErrorKind::BadRequest,
        }
    }

    pub fn internal_error(msg: impl AsRef<str>) -> AppError {
        AppError {
            message: msg.as_ref().into(),
            kind: AppErrorKind::InternalError,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let response_json = serde_json::json!({
        "message": &self.message,
        });
        let status: StatusCode = self.kind.into();
        (status, Json(response_json)).into_response()
    }
}

impl AppState {
    pub async fn get_file_by_id(&self, id: i64) -> Result<Source, AppError> {
        let video_path = sqlx::query!("SELECT path FROM videos WHERE id = ?", id)
            .fetch_one(&self.db.pool)
            .await?
            .path;
        let library = self.library.lock().unwrap();
        library
            .find_source(&video_path)
            .ok_or(AppError::not_found("file with path from db is not found"))
            .cloned()
    }

    pub async fn remove_video(&self, id: i64) -> Result<(), AppError> {
        let source = self.get_file_by_id(id).await?;
        source
            .delete()
            .map_err(|_| AppError::internal_error("Failed to remove video"))?;
        self.db.remove_video(id).await?;
        let mut library = self.library.lock().unwrap();
        library.remove_file(source.source_path());
        Ok(())
    }

    pub async fn remove_variant(&self, video_id: i64, variant_id: &str) -> Result<(), AppError> {
        let video_path = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id)
            .fetch_one(&self.db.pool)
            .await?
            .path;
        let mut library = self.library.lock().unwrap();
        let source = library
            .find_source_mut(&video_path)
            .ok_or(AppError::not_found("file with path from db is not found"))?;
        source.delete_variant(&variant_id);
        Ok(())
    }

    pub async fn add_show<T>(
        &self,
        video_path: PathBuf,
        metadata_provider: &T,
    ) -> Result<(), AppError>
    where
        T: DiscoverMetadataProvider + ShowMetadataProvider,
    {
        let show = LibraryFile::from_path(video_path).await?;
        {
            let mut library = self.library.lock().unwrap();
            library.add_show(show);
        }
        // self.handle_show(show, metadata_provider).await
        todo!();
        Ok(())
    }

    pub async fn add_movie(
        &self,
        video_path: PathBuf,
        metadata_provider: &impl DiscoverMetadataProvider,
    ) -> Result<(), AppError> {
        let movie = LibraryFile::from_path(video_path).await?;
        // the heck is going on here
        {
            let mut library = self.library.lock().unwrap();
            library.add_movie(movie.clone());
        };
        self.handle_movie(movie, metadata_provider).await
    }

    #[tracing::instrument]
    pub async fn extract_subs(&self, video_id: i64) -> Result<(), AppError> {
        let source = self.get_file_by_id(video_id).await?;
        let mut jobs = Vec::new();
        let task_id = self
            .tasks
            .start_task(
                TaskKind::Subtitles {
                    target: source.source_path().to_path_buf(),
                },
                None,
            )
            .unwrap();
        let subtitles_path = source.subtitles_path();
        for stream in source.origin.subtitle_streams() {
            if stream.codec().supports_text() {
                let job = source
                    .generate_subtitles(stream.index)
                    .expect("track to support text decoding");
                jobs.push((job, stream));
            }
        }
        for (mut job, stream) in jobs {
            if let Ok(status) = job.wait().await {
                if status.success() {
                    let mut file_path = subtitles_path.clone();
                    file_path.push(format!("{}.srt", stream.language));
                    let mut file = std::fs::File::open(&file_path)?;
                    let metadata = fs::metadata(&file_path).await?;
                    let size = metadata.len();
                    let hash = utils::file_hash(&mut file)?;
                    let db_subtitles = DbSubtitles {
                        id: None,
                        language: stream.language.to_string(),
                        path: subtitles_path.to_str().unwrap().to_string(),
                        hash: hash.to_string(),
                        size: size as i64,
                        video_id,
                    };

                    self.db.insert_subtitles(db_subtitles).await?;
                }
            }
        }
        self.tasks.finish_task(task_id).expect("task to exist");
        Ok(())
    }

    pub async fn download_torrent(
        &self,
        torrent: Torrent,
        output: PathBuf,
    ) -> Result<(), AppError> {
        let _ = self
            .tasks
            .tracker
            .track_future(
                self.tasks
                    .observe_torrent_download(self.torrent_client, torrent, output),
            )
            .await;
        Ok(())
    }

    #[tracing::instrument]
    pub async fn transcode_video(
        &self,
        video_id: i64,
        payload: TranscodePayload,
    ) -> Result<(), AppError> {
        let source = self.get_file_by_id(video_id).await?;

        let job = source.transcode_video(payload)?;
        let output_path = job.job.output_path.clone();

        self.tasks
            .tracker
            .track_future(self.tasks.observe_ffmpeg_task(
                job,
                TaskKind::Transcode {
                    target: source.source_path().to_path_buf(),
                },
            ))
            .await?;
        let variant_video = Video::from_path(output_path)
            .await
            .expect("file to be done transcoding");

        let mut library = self.library.lock().unwrap();
        library.add_variant(source.source_path(), variant_video);

        Ok(())
    }

    #[tracing::instrument]
    pub async fn generate_previews(&self, video_id: i64) -> Result<(), AppError> {
        let file = self.get_file_by_id(video_id).await?;

        if (file.previews_count() as f64) < (file.duration().as_secs() as f64 / 10.0).round() {
            tracing::warn!("Rewriting existing previews")
        }

        let job = file.generate_previews()?;

        let run_result = self
            .tasks
            .tracker
            .track_future(self.tasks.observe_ffmpeg_task(
                job,
                TaskKind::Previews {
                    target: file.source_path().to_path_buf(),
                },
            ))
            .await;

        if let Err(err) = run_result {
            return Err(err.into());
        }
        Ok(())
    }

    pub async fn reconciliate_library(&self) -> Result<(), AppError> {
        let local_episodes = {
            let library = self.library.lock().unwrap();
            library.shows.clone()
        };

        let db_episodes_videos = sqlx::query!(
            r#"SELECT videos.*, episodes.id as "episode_id!" FROM videos
        JOIN episodes ON videos.id = episodes.video_id"#
        )
        .fetch_all(&self.db.pool)
        .await?;

        let missing_episodes: Vec<_> = db_episodes_videos
            .iter()
            .filter(|d| {
                local_episodes
                    .iter()
                    .find(|l| d.path == l.source.source_path().to_string_lossy().to_string())
                    .is_none()
            })
            .collect();

        let mut new: Vec<_> = local_episodes
            .into_iter()
            .filter(|l| {
                db_episodes_videos
                    .iter()
                    .find(|d| d.path == l.source.source_path().to_string_lossy().to_string())
                    .is_none()
            })
            .collect();
        new.sort_unstable_by_key(|x| x.identifier.title.clone());

        let mut show_scan_handles: JoinSet<Result<(), AppError>> = JoinSet::new();

        for mut new_episodes in new
            .chunk_by(|a, b| a.identifier.title == b.identifier.title)
            .map(Vec::from)
        {
            new_episodes.sort_unstable_by_key(|x| x.identifier.season);
            let db = self.db.clone();
            let providers_stack = self.providers_stack;
            let title = new_episodes.first().unwrap().identifier.title.clone();
            let discover_providers = providers_stack.discover_providers();
            let show_providers = providers_stack.show_providers();
            show_scan_handles.spawn(async move {
                let local_id = handle_series(&title, &db, discover_providers).await?;
                let external_ids = db.external_ids(&local_id, ContentType::Show).await.unwrap();
                for new_seasons_episodes in new_episodes
                    .chunk_by(|a, b| a.identifier.season == b.identifier.season)
                    .map(Vec::from)
                {
                    let season = new_seasons_episodes.first().unwrap().identifier.season;
                    let local_season_id = handle_season(
                        &local_id,
                        external_ids.clone(),
                        season.into(),
                        &db,
                        &show_providers,
                    )
                    .await
                    .unwrap();

                    for episode in new_seasons_episodes {
                        dbg!(episode.source.source_path());
                        let db_video = episode.source.into_db_video();
                        let Ok(video_id) = db.insert_video(db_video).await else {
                            continue;
                        };
                        if let Err(_) = handle_episode(
                            &local_id,
                            external_ids.clone(),
                            video_id,
                            local_season_id,
                            season.into(),
                            episode.identifier.episode.into(),
                            &db,
                            &show_providers,
                        )
                        .await
                        {
                            tracing::warn!(
                                "Failed to fetch metadata for episode: {}",
                                episode.source.source_path().display()
                            );
                        }
                    }
                }

                Ok(())
            });
        }

        for missing_episode in missing_episodes {
            let _ = self.db.remove_episode(missing_episode.id).await;
        }

        while let Some(result) = show_scan_handles.join_next().await {
            match result {
                Ok(Err(e)) => tracing::error!("Reconciliation task failed with err {}", e),
                Err(_) => tracing::error!("Reconciliation task paniced"),
                Ok(Ok(res)) => tracing::trace!("Joined reconciliation task"),
            }
        }

        tracing::info!("Finished library reconciliation");
        Ok(())
    }

    pub async fn handle_movie(
        &self,
        movie: LibraryFile<MovieIdentifier>,
        metadata_provider: &impl DiscoverMetadataProvider,
    ) -> Result<(), AppError> {
        let resources_folder = movie.source.resources_folder_name();
        let movie_query = sqlx::query!(
            r#"SELECT movies.id as "id!" FROM movies 
                    JOIN videos ON videos.id = movies.video_id
                    WHERE videos.resources_folder = ?;"#,
            resources_folder
        )
        .fetch_one(&self.db.pool)
        .await
        .map(|x| (x.id));

        match movie_query {
            Err(e) => {
                if let sqlx::Error::RowNotFound = e {
                    tracing::debug!(
                        "Movie {} is not found in local DB, fetching metadata from {}",
                        movie.identifier.title,
                        metadata_provider.provider_identifier()
                    );
                    let metadata = metadata_provider
                        .movie_search(&movie.identifier.title)
                        .await
                        .unwrap()
                        .into_iter()
                        .next()
                        .ok_or(anyhow!("results are empty"))?;
                    let db_video = movie.source.into_db_video();
                    let video_id = self.db.insert_video(db_video).await?;
                    let provider = metadata.metadata_provider.clone();
                    let metadata_id = metadata.metadata_id.clone();
                    let db_movie = metadata.into_db_movie(video_id).await;
                    (self.db.insert_movie(db_movie).await?, metadata_id, provider);
                } else {
                    tracing::error!("Unexpected database error when fetching movie: {}", e);
                    return Err(e.into());
                }
            }
            _ => (),
        };

        Ok(())
    }
}

async fn handle_series(
    title: &str,
    db: &Db,
    providers: Vec<&(dyn DiscoverMetadataProvider + Send + Sync)>,
) -> Result<String, AppError> {
    let shows = db.search_show(&title).await.unwrap();

    if shows.is_empty() {
        for provider in providers {
            if let Ok(show) = provider.show_search(&title).await {
                let Some(first) = show.into_iter().next() else {
                    continue;
                };
                let external_ids = provider
                    .external_ids(&first.metadata_id, ContentType::Show)
                    .await?;
                let metadata_id = first.metadata_id.clone();
                let metadata_provider = first.metadata_provider;
                let local_id = db.insert_show(first.into_db_show().await).await.unwrap();
                db.insert_external_id(DbExternalId {
                    metadata_provider: metadata_provider.to_string(),
                    metadata_id: metadata_id.clone(),
                    show_id: Some(local_id),
                    is_prime: true.into(),
                    ..Default::default()
                })
                .await
                .unwrap();

                for external_id in external_ids {
                    let db_external_id = DbExternalId {
                        metadata_provider: external_id.provider.to_string(),
                        metadata_id: external_id.id,
                        show_id: Some(local_id),
                        is_prime: false.into(),
                        ..Default::default()
                    };
                    let _ = db.insert_external_id(db_external_id).await;
                }
                return Ok(local_id.to_string());
            }
        }
        return Err(AppError::not_found("providers could not find the show"));
    }
    let top_search = shows.into_iter().next().expect("shows not empty");
    Ok(top_search.metadata_id)
}

async fn handle_season(
    local_show_id: &str,
    external_shows_ids: Vec<ExternalIdMetadata>,
    season: usize,
    db: &Db,
    providers: &Vec<&(dyn ShowMetadataProvider + Send + Sync)>,
) -> anyhow::Result<i64> {
    let Ok(local_season) = db.season(local_show_id, season).await else {
        for provider in providers {
            let p = MetadataProvider::from_str(provider.provider_identifier())
                .expect("all providers are known");
            if let Some(id) = external_shows_ids.iter().find(|id| id.provider == p) {
                let Ok(season) = provider.season(&id.id, season).await else {
                    continue;
                };
                let id = db
                    .insert_season(season.into_db_season(local_show_id.parse().unwrap()).await)
                    .await
                    .unwrap();
                return Ok(id);
            }
        }
        return Err(anyhow!("all providers failed to find season"));
    };
    Ok(local_season.metadata_id.parse().unwrap())
}

async fn handle_episode(
    local_show_id: &str,
    external_shows_ids: Vec<ExternalIdMetadata>,
    db_video_id: i64,
    local_season_id: i64,
    season: usize,
    episode: usize,
    db: &Db,
    providers: &Vec<&(dyn ShowMetadataProvider + Send + Sync)>,
) -> anyhow::Result<()> {
    let Ok(_) = db.episode(local_show_id, season, episode).await else {
        for provider in providers {
            let p = MetadataProvider::from_str(provider.provider_identifier())
                .expect("all providers are known");
            if let Some(id) = external_shows_ids.iter().find(|id| id.provider == p) {
                let Ok(episode) = provider.episode(&id.id, season, episode).await else {
                    continue;
                };
                db.insert_episode(episode.into_db_episode(local_season_id, db_video_id).await)
                    .await
                    .unwrap();
                return Ok(());
            }
        }
        return Err(anyhow!("all providers failed to find episode"));
    };
    Ok(())
}

impl FromRef<AppState> for &'static Mutex<Library> {
    fn from_ref(app_state: &AppState) -> &'static Mutex<Library> {
        app_state.library
    }
}

impl FromRef<AppState> for &'static torrent::Client {
    fn from_ref(app_state: &AppState) -> &'static torrent::Client {
        app_state.torrent_client
    }
}

impl FromRef<AppState> for Db {
    fn from_ref(app_state: &AppState) -> Db {
        app_state.db.clone()
    }
}

impl FromRef<AppState> for TaskResource {
    fn from_ref(app_state: &AppState) -> TaskResource {
        app_state.tasks.clone()
    }
}

impl FromRef<AppState> for &'static Mutex<ServerConfiguration> {
    fn from_ref(app_state: &AppState) -> &'static Mutex<ServerConfiguration> {
        app_state.configuration
    }
}

impl FromRef<AppState> for &'static TmdbApi {
    fn from_ref(app_state: &AppState) -> &'static TmdbApi {
        app_state.tmdb_api
    }
}

impl FromRef<AppState> for &'static TpbApi {
    fn from_ref(app_state: &AppState) -> &'static TpbApi {
        app_state.tpb_api
    }
}

impl FromRef<AppState> for &'static MetadataProvidersStack {
    fn from_ref(app_state: &AppState) -> &'static MetadataProvidersStack {
        app_state.providers_stack
    }
}
