use std::{
    fmt::Display,
    num::ParseIntError,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Mutex,
};

use anyhow::anyhow;
use axum::{extract::FromRef, http::StatusCode, response::IntoResponse, Json};
use tokio::{fs, task::JoinSet};
use tokio_util::sync::CancellationToken;
use torrent::Torrent;

use crate::{
    config::ServerConfiguration,
    db::{Db, DbExternalId, DbSubtitles},
    ffmpeg::{self, FFmpegTask, SubtitlesJob, TranscodeJob},
    library::{
        assets::{
            AssetDir, BackdropAsset, BackdropContentType, FileAsset, PosterAsset,
            PosterContentType, SubtitlesDirAsset, VariantAsset,
        },
        movie::MovieIdentifier,
        Library, LibraryFile, Source, TranscodePayload, Video,
    },
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
    pub async fn get_source_by_id(&self, id: i64) -> Result<Source, AppError> {
        let library = self.library.lock().unwrap();
        library
            .get_source(id)
            .ok_or(AppError::not_found("file with path from db is not found"))
            .cloned()
    }

    pub async fn remove_video(&self, id: i64) -> Result<(), AppError> {
        let source = self.get_source_by_id(id).await?;
        source
            .video
            .delete()
            .await
            .map_err(|_| AppError::internal_error("Failed to remove video"))?;
        let _ = source.delete_all_resources().await;
        self.db.remove_video(id).await?;
        let mut library = self.library.lock().unwrap();
        library.remove_file(id);
        Ok(())
    }

    pub async fn remove_variant(&self, video_id: i64, variant_id: &str) -> Result<(), AppError> {
        let asset = VariantAsset::new(video_id, variant_id.to_string());
        asset.delete_file().await?;
        if let Some(source) = self.library.lock().unwrap().get_source_mut(video_id) {
            source
                .variants
                .iter()
                .position(|x| x.path() == asset.path())
                .map(|idx| source.variants.swap_remove(idx));
        };
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
        let show = LibraryFile::from_path(video_path, &self.db).await?;
        {
            let mut library = self.library.lock().unwrap();
            library.add_show(show.source.id, show);
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
        let movie = LibraryFile::from_path(video_path, &self.db).await?;
        // the heck is going on here
        {
            let mut library = self.library.lock().unwrap();
            library.add_movie(movie.source.id, movie.clone());
        };
        self.handle_movie(movie, metadata_provider).await
    }

    pub async fn extract_subs(&self, video_id: i64) -> Result<(), AppError> {
        let source = self.get_source_by_id(video_id).await?;
        let mut jobs = Vec::new();
        let task_id = self
            .tasks
            .start_task(
                TaskKind::Subtitles {
                    target: source.video.path().to_path_buf(),
                },
                None,
            )
            .unwrap();
        let subtitles_dir = SubtitlesDirAsset::new(video_id);
        for stream in source.video.subtitle_streams() {
            if stream.codec().supports_text() {
                let job = SubtitlesJob::from_source(
                    &source.video,
                    subtitles_dir.prepare_path().await?,
                    stream.index,
                )?;
                let output_file = job.output_file_path.clone();
                let job = job.run(source.video.path().to_path_buf(), source.video.duration())?;
                jobs.push((job, stream, output_file));
            }
        }
        for (mut job, stream, output_file) in jobs {
            if let Ok(status) = job.wait().await {
                if status.success() {
                    let metadata = fs::metadata(&output_file).await?;
                    let size = metadata.len();
                    let mut file = std::fs::File::open(&output_file)?;
                    let hash = utils::file_hash(&mut file)?;
                    let db_subtitles = DbSubtitles {
                        id: None,
                        language: stream.language.map(|x| x.to_string()),
                        path: output_file.to_string_lossy().to_string(),
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

    /// Get subtitle track from video file without saving it. Takes some time to run ffmpeg
    pub async fn pull_subtitle_from_video(
        &self,
        video_id: i64,
        subs_track: usize,
    ) -> Result<String, AppError> {
        let video = self.get_source_by_id(video_id).await?.video;
        let track_number = {
            video
                .subtitle_streams()
                .get(subs_track)
                .ok_or(AppError::not_found(
                    "Specified subtitle track does not exists",
                ))?
                .index
        };
        let subtitle = ffmpeg::pull_subtitles(video.path(), track_number).await?;
        Ok(subtitle)
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

    pub async fn transcode_video(
        &self,
        video_id: i64,
        payload: TranscodePayload,
    ) -> Result<(), AppError> {
        let source = self.get_source_by_id(video_id).await?;
        let variant_assets = source.variants_dir();
        let output_path = variant_assets.prepare_path().await?;
        let hw_accel_enabled = self.configuration.lock().unwrap().hw_accel;
        let job = TranscodeJob::from_source(&source, output_path, payload, hw_accel_enabled)?;
        let variant_path = job.output_path.clone();
        let job = job.run(source.video.path().to_path_buf(), source.video.duration())?;

        self.tasks
            .tracker
            .track_future(self.tasks.observe_ffmpeg_task(
                job,
                TaskKind::Transcode {
                    target: source.video.path().to_path_buf(),
                },
            ))
            .await?;
        let variant = Video::from_path(variant_path).await?;
        if let Some(source) = self.library.lock().unwrap().get_source_mut(video_id) {
            source.variants.push(variant);
        };
        Ok(())
    }

    pub async fn generate_previews(&self, video_id: i64) -> Result<(), AppError> {
        let source = self.get_source_by_id(video_id).await?;
        let previews_dir = source.previews_dir();
        let count = previews_dir.previews_count();
        if (count as f64) < (source.video.duration().as_secs() as f64 / 10.0).round() {
            tracing::warn!("Rewriting existing previews")
        }
        let output_dir = previews_dir.prepare_path().await?;
        let job = ffmpeg::PreviewsJob::new(source.video.path(), output_dir);
        let job = ffmpeg::FFmpegRunningJob::new(
            job,
            source.video.path().to_path_buf(),
            source.video.duration(),
        )?;

        self.tasks
            .tracker
            .track_future(self.tasks.observe_ffmpeg_task(
                job,
                TaskKind::Previews {
                    target: source.video.path().to_path_buf(),
                },
            ))
            .await?;

        Ok(())
    }

    pub async fn reconciliate_library(&self) -> Result<(), AppError> {
        // TODO: refresh local library files
        let local_episodes = {
            let library = self.library.lock().unwrap();
            library.shows.clone()
        };

        let db_episodes_videos = sqlx::query!(
            r#"SELECT videos.id as "video_id!", episodes.id as "episode_id!" FROM videos
        JOIN episodes ON videos.id = episodes.video_id"#
        )
        .fetch_all(&self.db.pool)
        .await?;

        let missing_episodes: Vec<_> = db_episodes_videos
            .iter()
            .filter(|d| !local_episodes.contains_key(&d.video_id))
            .collect();
        debug_assert!(missing_episodes.is_empty());

        let mut new: Vec<_> = local_episodes
            .values()
            .filter(|l| {
                db_episodes_videos
                    .iter()
                    .find(|d| d.video_id == l.source.id)
                    .is_none()
            })
            .cloned()
            .collect();
        new.sort_unstable_by_key(|x| x.identifier.title.clone());

        let mut show_scan_handles: JoinSet<Result<(), AppError>> = JoinSet::new();

        for mut show_episodes in new
            .chunk_by(|a, b| a.identifier.title == b.identifier.title)
            .map(Vec::from)
        {
            show_episodes.sort_unstable_by_key(|x| x.identifier.season);
            let db = self.db.clone();
            let providers_stack = self.providers_stack;
            let title = show_episodes.first().unwrap().identifier.title.clone();
            let discover_providers = providers_stack.discover_providers();
            let show_providers = providers_stack.show_providers();
            show_scan_handles.spawn(async move {
                let local_id = handle_series(&title, &db, discover_providers).await?;
                let external_ids = db
                    .external_ids(&local_id.to_string(), ContentType::Show)
                    .await
                    .unwrap();
                let mut seasons_scan_handles: JoinSet<Result<(), AppError>> = JoinSet::new();
                for season_episodes in show_episodes
                    .chunk_by(|a, b| a.identifier.season == b.identifier.season)
                    .map(Vec::from)
                {
                    let external_ids = external_ids.clone();
                    let show_providers = show_providers.clone();
                    let db = db.clone();
                    seasons_scan_handles.spawn(async move {
                        let season = season_episodes.first().unwrap().identifier.season;
                        let local_season_id = handle_season(
                            local_id,
                            external_ids.clone(),
                            season.into(),
                            &db,
                            &show_providers,
                        )
                        .await?;
                        let mut episodes_scan_handles: JoinSet<anyhow::Result<()>> = JoinSet::new();

                        // we can spawn here one more time
                        for episode in season_episodes {
                            let db = db.clone();
                            let show_providers = show_providers.clone();
                            let external_ids = external_ids.clone();
                            episodes_scan_handles.spawn(async move {
                                handle_episode(
                                    local_id,
                                    external_ids.clone(),
                                    episode.source.id,
                                    local_season_id,
                                    season.into(),
                                    episode.identifier.episode.into(),
                                    &db,
                                    &show_providers,
                                )
                                .await
                            });
                        }
                        while let Some(result) = episodes_scan_handles.join_next().await {
                            match result {
                                Ok(Err(e)) => {
                                    tracing::error!("Episode reconciliation task failed with err {}", e)
                                }
                                Err(_) => tracing::error!("Episode reconciliation task paniced"),
                                Ok(Ok(_)) => tracing::trace!("Joined episode reconciliation task"),
                            }
                        }

                        Ok(())
                    });
                }

                while let Some(result) = seasons_scan_handles.join_next().await {
                    match result {
                        Ok(Err(e)) => tracing::error!("Season Reconciliation task failed with err {}", e),
                        Err(_) => tracing::error!("Season reconciliation task paniced"),
                        Ok(Ok(_)) => tracing::trace!("Joined season reconciliation task"),
                    }
                }
                Ok(())
            });
        }

        for missing_episode in missing_episodes {
            let _ = self.db.remove_episode(missing_episode.episode_id).await;
        }

        while let Some(result) = show_scan_handles.join_next().await {
            match result {
                Ok(Err(e)) => tracing::error!("Show reconciliation task failed with err {}", e),
                Err(_) => tracing::error!("Show reconciliation task paniced"),
                Ok(Ok(_)) => tracing::trace!("Joined show reconciliation task"),
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
        let movie_query = sqlx::query!(
            r#"SELECT movies.id as "id!" FROM movies 
                    JOIN videos ON videos.id = movies.video_id
                    WHERE videos.id = ?;"#,
            movie.source.id
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
                    let db_movie = metadata.into_db_movie(movie.source.id).await;
                    self.db.insert_movie(db_movie).await?;
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
) -> Result<i64, AppError> {
    let shows = db.search_show(&title).await.unwrap();

    if shows.is_empty() {
        for provider in providers {
            if let Ok(search_result) = provider.show_search(&title).await {
                let Some(first_result) = search_result.into_iter().next() else {
                    continue;
                };
                // save poster, backdrop and mutate result's urls to local,
                // leave original url in if saving fails
                let external_ids = provider
                    .external_ids(&first_result.metadata_id, ContentType::Show)
                    .await?;
                let metadata_id = first_result.metadata_id.clone();
                let metadata_provider = first_result.metadata_provider;
                let poster_url = first_result.poster.clone();
                let backdrop_url = first_result.backdrop.clone();
                let local_id = db
                    .insert_show(first_result.into_db_show().await)
                    .await
                    .unwrap();
                let poster_job = poster_url.map(|url| {
                    let poster_asset = PosterAsset::new(local_id, PosterContentType::Show);
                    save_asset_from_url(url.to_string(), poster_asset)
                });
                let backdrop_job = backdrop_url.map(|url| {
                    let backdrop_asset = BackdropAsset::new(local_id, BackdropContentType::Show);
                    save_asset_from_url(url.to_string(), backdrop_asset)
                });
                let insert_job = db.insert_external_id(DbExternalId {
                    metadata_provider: metadata_provider.to_string(),
                    metadata_id: metadata_id.clone(),
                    show_id: Some(local_id),
                    is_prime: true.into(),
                    ..Default::default()
                });
                match (poster_job, backdrop_job) {
                    (Some(poster_job), Some(backdrop_job)) => {
                        let _ = tokio::join!(poster_job, backdrop_job, insert_job);
                    }
                    (Some(poster_job), None) => {
                        let _ = tokio::join!(poster_job, insert_job);
                    }
                    (None, Some(backdrop_job)) => {
                        let _ = tokio::join!(backdrop_job, insert_job);
                    }
                    (None, None) => {
                        let _ = insert_job.await;
                    }
                }

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
                return Ok(local_id);
            }
        }
        return Err(AppError::not_found("providers could not find the show"));
    }
    let top_search = shows.into_iter().next().expect("shows not empty");
    Ok(top_search.metadata_id.parse().unwrap())
}

async fn handle_season(
    local_show_id: i64,
    external_shows_ids: Vec<ExternalIdMetadata>,
    season: usize,
    db: &Db,
    providers: &Vec<&(dyn ShowMetadataProvider + Send + Sync)>,
) -> anyhow::Result<i64> {
    let Ok(local_season) = db.season(&local_show_id.to_string(), season).await else {
        for provider in providers {
            let p = MetadataProvider::from_str(provider.provider_identifier())
                .expect("all providers are known");
            if let Some(id) = external_shows_ids.iter().find(|id| id.provider == p) {
                let Ok(season) = provider.season(&id.id, season).await else {
                    continue;
                };
                let id = db
                    .insert_season(season.into_db_season(local_show_id).await)
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
    local_show_id: i64,
    external_shows_ids: Vec<ExternalIdMetadata>,
    db_video_id: i64,
    local_season_id: i64,
    season: usize,
    episode: usize,
    db: &Db,
    providers: &Vec<&(dyn ShowMetadataProvider + Send + Sync)>,
) -> anyhow::Result<()> {
    let Ok(_) = db
        .episode(&local_show_id.to_string(), season, episode)
        .await
    else {
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

pub(crate) async fn save_asset_from_url(
    url: impl reqwest::IntoUrl,
    asset: impl FileAsset,
) -> anyhow::Result<()> {
    use std::io::{Error, ErrorKind};
    use tokio_stream::StreamExt;
    use tokio_util::io::StreamReader;

    let response = reqwest::get(url).await.unwrap();
    let stream = response
        .bytes_stream()
        .map(|data| data.map_err(|e| Error::new(ErrorKind::Other, e)));
    let mut stream_reader = StreamReader::new(stream);
    asset.save_from_reader(&mut stream_reader).await?;
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
