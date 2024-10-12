use std::{
    collections::HashMap, error::Error, fmt::Display, num::ParseIntError, str::FromStr, sync::Mutex,
};

use axum::{extract::FromRef, http::StatusCode, response::IntoResponse, Json};
use tokio::{fs, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{self},
    db::{Db, DbActions, DbEpisode, DbExternalId, DbMovie, DbSeason, DbShow, DbSubtitles},
    ffmpeg::{self, FFmpegRunningJob, SubtitlesJob, TranscodeJob},
    library::{
        assets::{
            AssetDir, BackdropAsset, BackdropContentType, FileAsset, PosterAsset,
            PosterContentType, SubtitlesDirAsset, VariantAsset,
        },
        movie::MovieIdentifier,
        show::ShowIdentifier,
        ContentIdentifier, Library, LibraryItem, Source, TranscodePayload, Video,
    },
    metadata::{
        tmdb_api::TmdbApi, ContentType, DiscoverMetadataProvider, ExternalIdMetadata,
        MetadataProvider, MetadataProvidersStack, MovieMetadata, ShowMetadata,
        ShowMetadataProvider,
    },
    progress::{TaskKind, TaskResource, VideoTaskType},
    torrent::TorrentClient,
    torrent_index::tpb::TpbApi,
    utils,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub library: &'static Mutex<Library>,
    pub db: &'static Db,
    pub tasks: &'static TaskResource,
    pub tmdb_api: &'static TmdbApi,
    pub tpb_api: &'static TpbApi,
    pub providers_stack: &'static MetadataProvidersStack,
    pub torrent_client: &'static TorrentClient,
    pub cancelation_token: CancellationToken,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct AppError {
    pub message: String,
    #[serde(skip)]
    pub kind: AppErrorKind,
}

#[derive(Debug, Clone, utoipa::ToSchema, PartialEq)]
pub enum AppErrorKind {
    InternalError,
    NotFound,
    Duplicate,
    BadRequest,
}

impl Error for AppError {}

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

impl From<AppErrorKind> for StatusCode {
    fn from(val: AppErrorKind) -> Self {
        match val {
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
                message: "Database row not found".to_string(),
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
        let status: StatusCode = self.kind.clone().into();
        (status, Json(self)).into_response()
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
        let mut remove_tx = self.db.begin().await?;
        remove_tx.remove_video(id).await?;
        remove_tx.commit().await?;
        let mut library = self.library.lock().unwrap();
        library.remove_video(id);
        Ok(())
    }

    pub async fn remove_variant(&self, video_id: i64, variant_id: &str) -> Result<(), AppError> {
        let asset = VariantAsset::new(video_id, variant_id.to_string());
        asset.delete_file().await?;
        if let Some(source) = self.library.lock().unwrap().get_source_mut(video_id) {
            source
                .variants
                .iter()
                .position(|x| *x.path() == asset.path())
                .map(|idx| source.variants.swap_remove(idx));
        };
        Ok(())
    }

    pub async fn reset_show_metadata(&self, show_id: i64) -> Result<(), AppError> {
        self.partial_refresh().await;
        let orphans = sqlx::query!(
            r#"SELECT videos.id FROM videos 
JOIN episodes ON episodes.video_id = videos.id
JOIN seasons ON seasons.id = episodes.season_id
JOIN shows ON shows.id = seasons.show_id
WHERE shows.id = ? ORDER BY seasons.number;"#,
            show_id
        )
        .fetch_all(&self.db.pool)
        .await?;
        self.db.remove_show(show_id).await?;
        let mut orphans: Vec<_> = {
            let library = self.library.lock().unwrap();
            orphans
                .into_iter()
                .filter_map(|x| library.get_show(x.id))
                .collect()
        };

        orphans.sort_unstable_by_key(|x| x.identifier.title.clone());

        let mut show_scan_handles: JoinSet<Result<(), AppError>> = JoinSet::new();

        for mut show_episodes in orphans
            .chunk_by(|a, b| a.identifier.title == b.identifier.title)
            .map(Vec::from)
        {
            show_episodes.sort_unstable_by_key(|x| x.identifier.season);
            let db = self.db.clone();
            let providers_stack = self.providers_stack;
            let discover_providers = providers_stack.discover_providers();
            let show_providers = providers_stack.show_providers();
            show_scan_handles.spawn(async move {
                let identifier = show_episodes.first().unwrap();
                let local_show_id = handle_series(identifier, &db, discover_providers).await?;
                handle_seasons_and_episodes(&db, local_show_id, show_episodes, &show_providers)
                    .await?;
                Ok(())
            });
        }

        while let Some(result) = show_scan_handles.join_next().await {
            match result {
                Ok(Err(e)) => tracing::error!("Show reconciliation task failed with err {}", e),
                Err(_) => tracing::error!("Show reconciliation task paniced"),
                Ok(Ok(_)) => tracing::trace!("Joined show reconciliation task"),
            }
        }

        Ok(())
    }

    pub async fn reset_movie_metadata(&self, movie_id: i64) -> Result<(), AppError> {
        self.partial_refresh().await;
        let video = sqlx::query!(
            "SELECT videos.id FROM videos JOIN movies ON movies.video_id = videos.id WHERE movies.id = ?",
            movie_id
        )
            .fetch_one(&self.db.pool).await?;
        self.db.remove_movie(movie_id).await?;
        let movie = {
            let library = self.library.lock().unwrap();
            library.get_movie(video.id).unwrap()
        };
        let discover_providers = self.providers_stack.discover_providers();
        handle_movie(movie, self.db, discover_providers).await?;
        Ok(())
    }

    pub async fn fix_show_metadata(
        &self,
        show_id: i64,
        new_metadata: ShowMetadata,
    ) -> Result<(), AppError> {
        self.partial_refresh().await;
        let orphans = sqlx::query!(
            r#"SELECT videos.id FROM videos 
JOIN episodes ON episodes.video_id = videos.id
JOIN seasons ON seasons.id = episodes.season_id
JOIN shows ON shows.id = seasons.show_id
WHERE shows.id = ? ORDER BY seasons.number;"#,
            show_id
        )
        .fetch_all(&self.db.pool)
        .await?;
        self.db.remove_show(show_id).await?;
        let orphans: Vec<_> = {
            let library = self.library.lock().unwrap();
            orphans
                .into_iter()
                .filter_map(|x| library.get_show(x.id))
                .collect()
        };

        let local_show_id = {
            if new_metadata.metadata_provider == MetadataProvider::Local {
                new_metadata.metadata_id.parse()?
            } else {
                let mut external_ids = self
                    .providers_stack
                    .get_external_ids(
                        &new_metadata.metadata_id,
                        ContentType::Show,
                        new_metadata.metadata_provider,
                    )
                    .await?;
                let mut ids = Vec::with_capacity(external_ids.len() + 1);
                ids.push(ExternalIdMetadata {
                    provider: new_metadata.metadata_provider,
                    id: new_metadata.metadata_id.clone(),
                });
                ids.append(&mut external_ids);
                match self
                    .db
                    .external_to_local_ids(&ids)
                    .await
                    .and_then(|x| x.show_id)
                {
                    Some(id) => id,
                    None => {
                        // We have 2 options here.
                        // 1. Fetch external'id with given provider and obtain show metadata with
                        //    respect to providers order
                        // 2. Fetch show with given provider
                        let provider = self
                            .providers_stack
                            .discover_providers()
                            .into_iter()
                            .find(|p| {
                                p.provider_identifier()
                                    == new_metadata.metadata_provider.to_string()
                            })
                            .unwrap();
                        tracing::info!("Fetching show as new");
                        handle_show_metadata(self.db, new_metadata, provider).await?
                    }
                }
            }
        };

        let show_providers = self.providers_stack.show_providers();
        handle_seasons_and_episodes(self.db, local_show_id, orphans, &show_providers).await?;

        Ok(())
    }

    pub async fn fix_movie_metadata(
        &self,
        target_movie_id: i64,
        new_metadata: MovieMetadata,
    ) -> Result<(), AppError> {
        self.partial_refresh().await;
        let video = sqlx::query!(
            "SELECT videos.id FROM videos JOIN movies ON movies.video_id = videos.id WHERE movies.id = ?",
            target_movie_id
        )
            .fetch_one(&self.db.pool).await?;
        self.db.remove_movie(target_movie_id).await?;
        let movie = {
            let library = self.library.lock().unwrap();
            library.get_movie(video.id).unwrap()
        };
        let external_ids = self
            .providers_stack
            .get_external_ids(
                &new_metadata.metadata_id,
                ContentType::Movie,
                new_metadata.metadata_provider,
            )
            .await?;
        handle_movie_metadata(self.db, new_metadata, movie, external_ids).await?;

        Ok(())
    }

    /// TODO: this whole thing is wrong on so many levels
    pub async fn extract_subs(&self, video_id: i64) -> Result<(), AppError> {
        let source = self.get_source_by_id(video_id).await?;
        let mut jobs = Vec::new();
        let task_id = self
            .tasks
            .start_video_task(video_id, VideoTaskType::Subtitles, None)
            .unwrap();
        let subtitles_asset = SubtitlesDirAsset::new(video_id);
        let subtitles_dir = subtitles_asset.path();
        fs::create_dir_all(&subtitles_dir).await?;
        let video_metadata = source.video.metadata().await?;
        for stream in video_metadata.subtitle_streams() {
            if stream.codec().supports_text() {
                let job =
                    SubtitlesJob::from_source(&source.video, &subtitles_dir, stream.index).await?;
                let output_file = job.output_file_path.clone();
                let job = FFmpegRunningJob::spawn(job, video_metadata.duration())?;
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
        let metadata = video.metadata().await?;
        let track_number = {
            metadata
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

    pub async fn transcode_video(
        &self,
        video_id: i64,
        payload: TranscodePayload,
    ) -> Result<(), AppError> {
        let source = self.get_source_by_id(video_id).await?;
        let video_metadata = source.video.metadata().await?;
        let variants_dir = source.variants_dir();
        fs::create_dir_all(variants_dir.temp_path()).await?;
        let variant_id = uuid::Uuid::new_v4();
        let variant_asset = source.variant(variant_id.to_string());
        let temp_path = variant_asset.temp_path();
        let hw_accel_enabled: config::HwAccel = config::CONFIG.get_value();
        let job = TranscodeJob::from_source(&source, &temp_path, payload, hw_accel_enabled.0)?;
        let temp_path = job.output_path.clone();
        let job = FFmpegRunningJob::spawn(job, video_metadata.duration())?;
        let task_resource = self.tasks;
        let library = self.library;

        self.tasks.tracker.spawn(async move {
            let transcode_result = task_resource
                .observe_task(
                    job,
                    TaskKind::Video {
                        video_id,
                        task_type: VideoTaskType::Transcode,
                    },
                )
                .await;
            let resource_path = variant_asset.path();
            if let Err(err) = transcode_result {
                let _ = fs::remove_file(&temp_path).await;
                tracing::error!("Transcode task failed: {err}");
                return;
            } else {
                let _ = fs::create_dir_all(variants_dir.path()).await;
                fs::rename(&temp_path, &resource_path).await.unwrap();
            };

            let variant = match Video::from_path(resource_path).await {
                Ok(video) => video,
                Err(e) => {
                    tracing::error!("Failed to construct variant video: {e}");
                    return;
                }
            };

            if variant.metadata().await.is_err() {
                tracing::warn!("Removing broken transcoded variant");
                let _ = variant.delete().await;
            }

            if let Some(source) = library.lock().unwrap().get_source_mut(video_id) {
                source.variants.push(variant);
            };
        });
        Ok(())
    }

    pub async fn generate_previews(&self, video_id: i64) -> Result<(), AppError> {
        let source = self.get_source_by_id(video_id).await?;
        let video_metadata = source.video.metadata().await?;
        let previews_dir = source.previews_dir();
        let count = previews_dir.previews_count();
        if count > 0 {
            tracing::warn!("Rewriting existing previews")
        }
        let temp_dir = previews_dir.temp_path();
        fs::create_dir_all(&temp_dir).await?;
        let job = ffmpeg::PreviewsJob::new(source.video.path(), &temp_dir);
        let job = ffmpeg::FFmpegRunningJob::spawn(job, video_metadata.duration())?;

        let task_resource = self.tasks;
        self.tasks.tracker.spawn(async move {
            let job_result = task_resource
                .observe_task(
                    job,
                    TaskKind::Video {
                        video_id,
                        task_type: VideoTaskType::Previews,
                    },
                )
                .await;
            if job_result.is_ok() {
                let resources_dir = previews_dir.path();
                fs::create_dir_all(&resources_dir.parent().unwrap())
                    .await
                    .unwrap();
                let _ = fs::remove_dir_all(&resources_dir).await;
                fs::rename(temp_dir, resources_dir).await.unwrap();
            } else {
                let _ = fs::remove_dir(temp_dir).await;
            }
        });

        Ok(())
    }

    pub async fn reconciliate_library(&self) -> Result<(), AppError> {
        self.partial_refresh().await;
        let local_episodes: Vec<_> = {
            let library = self.library.lock().unwrap();
            library.episodes().collect()
        };

        let db_episodes_videos = sqlx::query!(
            r#"SELECT videos.id as "video_id", episodes.id as "episode_id" FROM videos
        JOIN episodes ON videos.id = episodes.video_id"#
        )
        .fetch_all(&self.db.pool)
        .await?;

        let missing_episodes: Vec<_> = db_episodes_videos
            .iter()
            .filter(|d| !local_episodes.iter().any(|x| x.source.id == d.video_id))
            .collect();

        for missing_episode in &missing_episodes {
            if let Err(e) = self.db.remove_video(missing_episode.video_id).await {
                tracing::error!("Failed to remove video: {e}");
            };
        }

        let mut new_episodes: Vec<_> = local_episodes
            .into_iter()
            .filter(|l| !db_episodes_videos.iter().any(|d| d.video_id == l.source.id))
            .collect();
        new_episodes.sort_unstable_by_key(|x| x.identifier.title.clone());

        let mut show_scan_handles: JoinSet<Result<(), AppError>> = JoinSet::new();

        for mut show_episodes in new_episodes
            .chunk_by(|a, b| a.identifier.title == b.identifier.title)
            .map(Vec::from)
        {
            show_episodes.sort_unstable_by_key(|x| x.identifier.season);
            let db = self.db.clone();
            let providers_stack = self.providers_stack;
            let discover_providers = providers_stack.discover_providers();
            let show_providers = providers_stack.show_providers();
            show_scan_handles.spawn(async move {
                let identifier = show_episodes.first().unwrap();
                let local_show_id = handle_series(identifier, &db, discover_providers).await?;
                handle_seasons_and_episodes(&db, local_show_id, show_episodes, &show_providers)
                    .await?;
                Ok(())
            });
        }

        while let Some(result) = show_scan_handles.join_next().await {
            match result {
                Ok(Err(e)) => tracing::error!("Show reconciliation task failed with err {}", e),
                Err(_) => tracing::error!("Show reconciliation task paniced"),
                Ok(Ok(_)) => tracing::trace!("Joined show reconciliation task"),
            }
        }

        let local_movies: Vec<_> = {
            let library = self.library.lock().unwrap();
            library.movies().collect()
        };

        let db_movies_videos = sqlx::query!(
            r#"SELECT videos.id as "video_id", movies.id as "episode_id" FROM videos
        JOIN movies ON videos.id = movies.video_id"#
        )
        .fetch_all(&self.db.pool)
        .await?;

        let missing_movies: Vec<_> = db_movies_videos
            .iter()
            .filter(|d| !local_movies.iter().any(|x| x.source.id == d.video_id))
            .collect();
        for missing_movie in &missing_movies {
            if let Err(e) = self.db.remove_video(missing_movie.video_id).await {
                tracing::error!("Failed to remove video: {e}");
            };
        }

        let new_movies: Vec<_> = local_movies
            .into_iter()
            .filter(|l| !db_movies_videos.iter().any(|d| d.video_id == l.source.id))
            .collect();

        let mut movie_scan_handles = JoinSet::new();
        let discover_providers = self.providers_stack.discover_providers();
        for movie in new_movies {
            let discover_providers = discover_providers.clone();
            let db = self.db;
            movie_scan_handles
                .spawn(async move { handle_movie(movie, db, discover_providers).await });
        }

        while let Some(result) = movie_scan_handles.join_next().await {
            match result {
                Ok(Err(e)) => tracing::error!("Movie reconciliation task failed with err {}", e),
                Err(_) => tracing::error!("Movie reconciliation task paniced"),
                Ok(Ok(_)) => tracing::trace!("Joined movie reconciliation task"),
            }
        }

        tracing::info!("Finished library reconciliation");
        Ok(())
    }

    pub async fn partial_refresh(&self) {
        tracing::info!("Partially refreshing library");
        let mut videos = HashMap::new();
        let mut to_remove = Vec::new();
        let show_folders: config::ShowFolders = config::CONFIG.get_value();
        let movie_folders: config::MovieFolders = config::CONFIG.get_value();
        let mut show_paths = Vec::new();
        let mut movie_paths = Vec::new();
        {
            let mut library = self.library.lock().unwrap();
            for (id, file) in &library.videos {
                let file_path = file.source.video.path();
                if !file_path.try_exists().unwrap_or(false) {
                    to_remove.push(*id);
                }
                match file.identifier {
                    ContentIdentifier::Show(_) => {
                        if !show_folders
                            .as_ref()
                            .iter()
                            .any(|p| file_path.starts_with(p))
                        {
                            to_remove.push(*id);
                        } else {
                            show_paths.push(file.source.video.path().to_owned());
                        }
                    }
                    ContentIdentifier::Movie(_) => {
                        if !movie_folders
                            .as_ref()
                            .iter()
                            .any(|p| file_path.starts_with(p))
                        {
                            to_remove.push(*id);
                        } else {
                            movie_paths.push(file.source.video.path().to_owned());
                        }
                    }
                }
            }

            for absent_id in &to_remove {
                library.remove_video(*absent_id)
            }
        }
        for absent_id in &to_remove {
            let _ = self.db.remove_video(*absent_id).await;
        }

        for folder in show_folders.as_ref() {
            if let Ok(items) =
                crate::library::explore_folder(folder, ContentType::Show, self.db, &show_paths)
                    .await
            {
                videos.extend(items);
            }
        }

        for folder in movie_folders.as_ref() {
            if let Ok(items) =
                crate::library::explore_folder(&folder, ContentType::Movie, self.db, &movie_paths)
                    .await
            {
                videos.extend(items);
            }
        }

        self.library.lock().unwrap().videos.extend(videos);
    }
}

async fn handle_series(
    item: &LibraryItem<ShowIdentifier>,
    db: &Db,
    providers: Vec<&(dyn DiscoverMetadataProvider + Send + Sync)>,
) -> Result<i64, AppError> {
    // BUG: this will perform search with full text search so for example if we search for Dexter it will
    // find Dexter: New Blood.
    let shows = db.search_show(&item.identifier.title).await.unwrap();

    // WARN: This temporary fix will only work if content does not have custom name
    if shows.is_empty()
        || shows.first().unwrap().title.split_whitespace().count()
            != item.identifier.title.split_whitespace().count()
    {
        for provider in providers {
            if provider.provider_identifier() == "local" {
                continue;
            }
            if let Ok(search_result) = provider.show_search(&item.identifier.title).await {
                let Some(first_result) = search_result.into_iter().next() else {
                    continue;
                };
                let local_id = handle_show_metadata(db, first_result, provider).await?;
                return Ok(local_id);
            }
        }
        // fallback
        tracing::warn!("Using show metadata fallback");
        let id = series_metadata_fallback(db, item).await?;
        return Ok(id);
    }
    let top_search = shows.into_iter().next().expect("shows not empty");
    Ok(top_search.metadata_id.parse().unwrap())
}

async fn handle_show_metadata(
    db: &Db,
    metadata: ShowMetadata,
    provider: &(dyn DiscoverMetadataProvider + Send + Sync),
) -> anyhow::Result<i64> {
    let external_ids = provider
        .external_ids(&metadata.metadata_id, ContentType::Show)
        .await?;
    let metadata_id = metadata.metadata_id.clone();
    let metadata_provider = metadata.metadata_provider;
    let poster_url = metadata.poster.clone();
    let backdrop_url = metadata.backdrop.clone();
    let local_id = db.insert_show(metadata.into_db_show()).await.unwrap();
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
        if let Err(e) = db.insert_external_id(db_external_id).await {
            tracing::error!("Failed to insert external id: {e}");
        };
    }
    Ok(local_id)
}

async fn handle_movie_metadata(
    db: &Db,
    metadata: MovieMetadata,
    movie: LibraryItem<MovieIdentifier>,
    external_ids: Vec<ExternalIdMetadata>,
) -> anyhow::Result<i64> {
    let metadata_id = metadata.metadata_id.clone();
    let metadata_provider = metadata.metadata_provider;
    let poster_url = metadata.poster.clone();
    let backdrop_url = metadata.backdrop.clone();
    let local_id = db
        .insert_movie(metadata.into_db_movie(movie.source.id).await)
        .await?;
    let poster_job = poster_url.map(|url| {
        let poster_asset = PosterAsset::new(local_id, PosterContentType::Movie);
        save_asset_from_url_with_frame_fallback(url.to_string(), poster_asset, &movie.source)
    });
    let backdrop_job = backdrop_url.map(|url| {
        let backdrop_asset = BackdropAsset::new(local_id, BackdropContentType::Movie);
        save_asset_from_url(url.to_string(), backdrop_asset)
    });
    let insert_job = db.insert_external_id(DbExternalId {
        metadata_provider: metadata_provider.to_string(),
        metadata_id: metadata_id.clone(),
        movie_id: Some(local_id),
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
            movie_id: Some(local_id),
            is_prime: false.into(),
            ..Default::default()
        };
        let _ = db.insert_external_id(db_external_id).await;
    }
    Ok(local_id)
}

async fn handle_seasons_and_episodes(
    db: &Db,
    local_show_id: i64,
    mut show_episodes: Vec<LibraryItem<ShowIdentifier>>,
    show_providers: &Vec<&'static (dyn ShowMetadataProvider + Send + Sync)>,
) -> anyhow::Result<()> {
    show_episodes.sort_unstable_by_key(|x| x.identifier.season);
    let external_ids = db
        .external_ids(&local_show_id.to_string(), ContentType::Show)
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
            let season = season_episodes.first().unwrap().clone();
            let local_season_id = handle_season(
                local_show_id,
                external_ids.clone(),
                season,
                &db,
                &show_providers,
            )
            .await?;
            let mut episodes_scan_handles: JoinSet<anyhow::Result<i64>> = JoinSet::new();
            tracing::debug!("Season's episodes count: {}", season_episodes.len());
            for episode in season_episodes {
                let db = db.clone();
                let show_providers = show_providers.clone();
                let external_ids = external_ids.clone();
                episodes_scan_handles.spawn(async move {
                    handle_episode(
                        local_show_id,
                        external_ids.clone(),
                        local_season_id,
                        episode,
                        &db,
                        &show_providers,
                    )
                    .await
                });
            }

            while let Some(result) = episodes_scan_handles.join_next().await {
                match result {
                    Ok(Err(e)) => {
                        tracing::error!("Episode reconciliation task failed with err {e}",)
                    }
                    Err(e) => tracing::error!("Episode reconciliation task paniced: {e}"),
                    Ok(Ok(_)) => tracing::trace!("Joined episode reconciliation task"),
                }
            }

            Ok(())
        });
    }

    while let Some(result) = seasons_scan_handles.join_next().await {
        match result {
            Ok(Err(e)) => {
                tracing::error!("Season Reconciliation task failed with err {}", e)
            }
            Err(_) => tracing::error!("Season reconciliation task paniced"),
            Ok(Ok(_)) => tracing::trace!("Joined season reconciliation task"),
        }
    }
    Ok(())
}

async fn handle_season(
    local_show_id: i64,
    external_shows_ids: Vec<ExternalIdMetadata>,
    item: LibraryItem<ShowIdentifier>,
    db: &Db,
    providers: &Vec<&(dyn ShowMetadataProvider + Send + Sync)>,
) -> anyhow::Result<i64> {
    let season = item.identifier.season as usize;
    let Ok(local_season) = db.season(&local_show_id.to_string(), season).await else {
        for provider in providers {
            let p = MetadataProvider::from_str(provider.provider_identifier())
                .expect("all providers are known");
            if let Some(id) = external_shows_ids.iter().find(|id| id.provider == p) {
                let Ok(season) = provider.season(&id.id, season).await else {
                    continue;
                };
                let id = db
                    .insert_season(season.into_db_season(local_show_id))
                    .await
                    .unwrap();
                return Ok(id);
            }
        }
        // fallback
        tracing::warn!("Using season metadata fallback");
        let id = season_metadata_fallback(db, &item, local_show_id).await?;
        return Ok(id);
    };
    Ok(local_season.metadata_id.parse().unwrap())
}

async fn handle_episode(
    local_show_id: i64,
    external_shows_ids: Vec<ExternalIdMetadata>,
    local_season_id: i64,
    item: LibraryItem<ShowIdentifier>,
    db: &Db,
    providers: &Vec<&(dyn ShowMetadataProvider + Send + Sync)>,
) -> anyhow::Result<i64> {
    let season = item.identifier.season as usize;
    let episode = item.identifier.episode as usize;
    let Ok(local_episode) = db
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
                let poster = episode.poster.clone();
                let local_id = db
                    .insert_episode(episode.into_db_episode(local_season_id, item.source.id))
                    .await?;
                if let Some(poster) = poster {
                    let poster_asset = PosterAsset::new(local_id, PosterContentType::Episode);
                    let _ = save_asset_from_url_with_frame_fallback(
                        poster.to_string(),
                        poster_asset,
                        &item.source,
                    )
                    .await;
                }
                return Ok(local_id);
            }
        }
        // fallback
        tracing::warn!("Using episode metadata fallback");
        let id = episode_metadata_fallback(db, &item, item.source.id, local_season_id).await?;
        return Ok(id);
    };
    Ok(local_episode.metadata_id.parse().unwrap())
}

async fn handle_movie(
    item: LibraryItem<MovieIdentifier>,
    db: &Db,
    providers: Vec<&(dyn DiscoverMetadataProvider + Send + Sync)>,
) -> Result<i64, AppError> {
    let db_movies = db.search_movie(&item.identifier.title).await?;
    if db_movies.is_empty()
        || db_movies.first().unwrap().title.split_whitespace().count()
            != item.identifier.title.split_whitespace().count()
    {
        for provider in providers {
            if let Ok(search_result) = provider.movie_search(&item.identifier.title).await {
                let Some(first_result) = search_result.into_iter().next() else {
                    continue;
                };
                // save poster, backdrop and mutate result's urls to local,
                // leave original url in if saving fails
                let external_ids = provider
                    .external_ids(&first_result.metadata_id, ContentType::Movie)
                    .await?;
                let local_id = handle_movie_metadata(db, first_result, item, external_ids).await?;
                return Ok(local_id);
            }
        }
        let id = movie_metadata_fallback(db, &item, item.source.id).await?;
        return Ok(id);
    };
    Ok(db_movies.first().unwrap().metadata_id.parse().unwrap())
}

pub async fn series_metadata_fallback(
    db: &Db,
    file: &LibraryItem<ShowIdentifier>,
) -> anyhow::Result<i64> {
    let show_fallback = DbShow {
        id: None,
        poster: None,
        backdrop: None,
        plot: None,
        release_date: None,
        title: file.identifier.title.to_string(),
    };
    let video_metadata = file.source.video.metadata().await?;
    let id = db.insert_show(show_fallback).await.unwrap();
    let _ = db
        .insert_external_id(DbExternalId {
            metadata_provider: MetadataProvider::Local.to_string(),
            metadata_id: id.to_string(),
            show_id: Some(id),
            is_prime: true.into(),
            ..Default::default()
        })
        .await;
    let poster_asset = PosterAsset::new(id, PosterContentType::Show);
    fs::create_dir_all(poster_asset.path().parent().unwrap())
        .await
        .unwrap();
    let _ = ffmpeg::pull_frame(
        file.source.video.path(),
        poster_asset.path(),
        video_metadata.duration() / 2,
    )
    .await;
    Ok(id)
}

pub async fn season_metadata_fallback(
    db: &Db,
    file: &LibraryItem<ShowIdentifier>,
    show_id: i64,
) -> anyhow::Result<i64> {
    let fallback_season = DbSeason {
        number: file.identifier.season.into(),
        show_id,
        id: None,
        release_date: None,
        plot: None,
        poster: None,
    };
    let id = db.insert_season(fallback_season).await?;
    Ok(id)
}

pub async fn episode_metadata_fallback(
    db: &Db,
    file: &LibraryItem<ShowIdentifier>,
    video_id: i64,
    season_id: i64,
) -> anyhow::Result<i64> {
    let fallback_episode = DbEpisode {
        release_date: None,
        plot: None,
        poster: None,
        number: file.identifier.episode.into(),
        title: format!("Episode {}", file.identifier.episode),
        id: None,
        video_id,
        season_id,
    };
    let video_metadata = file.source.video.metadata().await?;
    let id = db.insert_episode(fallback_episode).await?;
    let poster_asset = PosterAsset::new(id, PosterContentType::Episode);
    fs::create_dir_all(poster_asset.path().parent().unwrap())
        .await
        .unwrap();
    let _ = ffmpeg::pull_frame(
        file.source.video.path(),
        poster_asset.path(),
        video_metadata.duration() / 2,
    )
    .await;
    Ok(id)
}

pub async fn movie_metadata_fallback(
    db: &Db,
    file: &LibraryItem<MovieIdentifier>,
    video_id: i64,
) -> anyhow::Result<i64> {
    let title = file.identifier.title[..1].to_uppercase() + &file.identifier.title[1..];
    let video_duration = file.source.video.metadata().await?.duration();
    let fallback_movie = DbMovie {
        id: None,
        video_id,
        poster: None,
        backdrop: None,
        plot: None,
        release_date: None,
        title,
    };
    let id = db.insert_movie(fallback_movie).await?;
    let poster_asset = PosterAsset::new(id, PosterContentType::Movie);
    fs::create_dir_all(poster_asset.path().parent().unwrap())
        .await
        .unwrap();
    let _ = ffmpeg::pull_frame(
        file.source.video.path(),
        poster_asset.path(),
        video_duration / 2,
    )
    .await;
    Ok(id)
}

pub(crate) async fn save_asset_from_url(
    url: impl reqwest::IntoUrl,
    asset: impl FileAsset,
) -> anyhow::Result<()> {
    use std::io::{Error, ErrorKind};
    use tokio_stream::StreamExt;
    use tokio_util::io::StreamReader;

    let response = reqwest::get(url).await?;
    let stream = response
        .bytes_stream()
        .map(|data| data.map_err(|e| Error::new(ErrorKind::Other, e)));
    let mut stream_reader = StreamReader::new(stream);
    asset.save_from_reader(&mut stream_reader).await?;
    Ok(())
}

async fn save_asset_from_url_with_frame_fallback(
    url: impl reqwest::IntoUrl,
    asset: impl FileAsset,
    source: &Source,
) -> anyhow::Result<()> {
    let asset_path = asset.path();
    let video_duration = source.video.metadata().await?.duration();
    if let Err(e) = save_asset_from_url(url, asset).await {
        tracing::warn!("Failed to save image, pulling frame: {e}");
        fs::create_dir_all(asset_path.parent().unwrap()).await?;
        ffmpeg::pull_frame(source.video.path(), asset_path, video_duration / 2).await?;
    }
    Ok(())
}

impl FromRef<AppState> for &'static Mutex<Library> {
    fn from_ref(app_state: &AppState) -> &'static Mutex<Library> {
        app_state.library
    }
}

impl FromRef<AppState> for &'static TorrentClient {
    fn from_ref(app_state: &AppState) -> &'static TorrentClient {
        app_state.torrent_client
    }
}

impl FromRef<AppState> for Db {
    fn from_ref(app_state: &AppState) -> Db {
        app_state.db.clone()
    }
}

impl FromRef<AppState> for &'static TaskResource {
    fn from_ref(app_state: &AppState) -> &'static TaskResource {
        app_state.tasks
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
