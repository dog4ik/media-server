use std::{
    collections::HashMap, error::Error, fmt::Display, num::ParseIntError, sync::Mutex,
    time::Instant,
};

use anyhow::Context;
use axum::{Json, extract::FromRef, http::StatusCode, response::IntoResponse};
use tokio::{fs, task::JoinSet};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{self},
    db::{Db, DbActions, DbSubtitles},
    ffmpeg::{self, FFmpegRunningJob, SubtitlesJob, TranscodeJob},
    library::{
        ContentIdentifier, Library, Source, TranscodePayload, Video,
        assets::{AssetDir, FileAsset, SubtitlesDirAsset, VariantAsset},
        explore_movie_dirs, explore_show_dirs,
    },
    metadata::{FetchParams, metadata_stack::MetadataProvidersStack},
    progress::TaskResource,
    scan,
    torrent::TorrentClient,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub library: &'static Mutex<Library>,
    pub db: &'static Db,
    pub tasks: &'static TaskResource,
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
            AppErrorKind::Duplicate => write!(f, "Duplicate Error: {}", self.message),
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
    pub fn metadata_fetch_params(&self) -> FetchParams {
        let language: config::MetadataLanguage = config::CONFIG.get_value();
        FetchParams { lang: language.0 }
    }

    pub fn get_source_by_id(&self, id: i64) -> Result<Source, AppError> {
        let library = self.library.lock().unwrap();
        library
            .get_source(id)
            .ok_or(AppError::not_found("file with path from db is not found"))
            .cloned()
    }

    pub async fn remove_video(&self, id: i64) -> Result<(), AppError> {
        let source = self.get_source_by_id(id)?;
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

    pub async fn delete_movie(&self, id: i64) -> Result<(), AppError> {
        let mut tx = self.db.begin().await?;
        let ids = sqlx::query!(
            "SELECT videos.id FROM videos JOIN movies ON movies.content_id = videos.content_id WHERE movies.id = ?",
            id
        )
        .fetch_all(&mut *tx)
        .await?;
        for video in ids {
            tx.remove_video(video.id).await?;
            if let Some(video) = {
                let mut library = self.library.lock().unwrap();
                library.remove_video(video.id)
            } {
                video.source.video.delete().await?;
                let _ = video.source.delete_all_resources().await;
            };
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_season(&self, id: i64) -> Result<(), AppError> {
        let mut tx = self.db.begin().await?;
        let ids = sqlx::query!(
            "SELECT videos.id FROM videos JOIN episodes ON episodes.content_id = videos.content_id WHERE episodes.season_id = ?",
            id
        )
        .fetch_all(&mut *tx)
        .await?;
        for video in ids {
            tx.remove_video(video.id).await?;
            if let Some(video) = {
                let mut library = self.library.lock().unwrap();
                library.remove_video(video.id)
            } {
                video.source.video.delete().await?;
                let _ = video.source.delete_all_resources().await;
            };
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_show(&self, id: i64) -> Result<(), AppError> {
        let mut tx = self.db.begin().await?;
        let ids = sqlx::query!(
            "SELECT videos.id FROM videos
JOIN episodes ON episodes.content_id = videos.content_id
JOIN seasons ON seasons.id = episodes.season_id
WHERE seasons.show_id = ?",
            id
        )
        .fetch_all(&mut *tx)
        .await?;
        for video in ids {
            tx.remove_video(video.id).await?;
            if let Some(video) = {
                let mut library = self.library.lock().unwrap();
                library.remove_video(video.id)
            } {
                video.source.video.delete().await?;
                let _ = video.source.delete_all_resources().await;
            };
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn delete_episode(&self, id: i64) -> Result<(), AppError> {
        let mut tx = self.db.begin().await?;
        let ids = sqlx::query!(
            "SELECT videos.id FROM videos JOIN episodes ON episodes.content_id = videos.content_id WHERE episodes.id = ?",
            id
        )
        .fetch_all(&mut *tx)
        .await?;
        for video in ids {
            tx.remove_video(video.id).await?;
            if let Some(video) = {
                let mut library = self.library.lock().unwrap();
                library.remove_video(video.id)
            } {
                video.source.video.delete().await?;
                let _ = video.source.delete_all_resources().await;
            };
        }
        tx.commit().await?;
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

    /// TODO: this whole thing is wrong on so many levels
    pub async fn extract_subs(&self, video_id: i64) -> Result<(), AppError> {
        let source = self.get_source_by_id(video_id)?;
        let mut jobs = Vec::new();
        let subtitles_asset = SubtitlesDirAsset::new(video_id);
        let subtitles_dir = subtitles_asset.path();
        fs::create_dir_all(&subtitles_dir).await?;
        let video_metadata = source.video.metadata().await?;
        for track in video_metadata.subtitle_streams() {
            if track.stream.codec.supports_text() {
                let job =
                    SubtitlesJob::from_source(&source.video, &subtitles_dir, track.index).await?;
                let output_file = job.output_file_path.clone();
                let job =
                    FFmpegRunningJob::spawn(&job, video_metadata.duration(), output_file.clone())?;
                jobs.push((job, &track.stream));
            }
        }
        for (mut job, stream) in jobs {
            if let Ok(status) = job.wait().await {
                if status.success() {
                    let db_subtitles = DbSubtitles {
                        id: None,
                        language: stream.language.clone(),
                        external_path: None,
                        video_id,
                    };

                    self.db.insert_subtitles(&db_subtitles).await?;
                }
            }
        }
        Ok(())
    }

    /// Get subtitle track from video file without saving it. Takes some time to run ffmpeg
    pub async fn pull_subtitle_from_video(
        &self,
        video_id: i64,
        subs_track: usize,
    ) -> Result<String, AppError> {
        let video = self.get_source_by_id(video_id)?.video;
        let metadata = video.metadata().await?;
        let track_number = {
            metadata
                .subtitle_streams()
                .nth(subs_track)
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
        let source = self.get_source_by_id(video_id)?;
        let video_metadata = source.video.metadata().await?;
        let variants_dir = source.variants_dir();
        fs::create_dir_all(variants_dir.temp_path()).await?;
        let variant_id = uuid::Uuid::new_v4();
        let variant_asset = source.variant(variant_id.to_string());
        let temp_path = variant_asset.temp_path();
        let hw_accel_enabled: config::HwAccel = config::CONFIG.get_value();
        let transcode_job =
            TranscodeJob::from_source(&source, &temp_path, payload, hw_accel_enabled.0).await?;
        let temp_path = transcode_job.output_path.clone();
        let running_job =
            FFmpegRunningJob::spawn(&transcode_job, video_metadata.duration(), temp_path.clone())?;
        let task_resource = self.tasks;
        let library = self.library;

        self.tasks.tracker.spawn(async move {
            let transcode_result = task_resource
                .transcode_tasks
                .observe_task(transcode_job, running_job)
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
        let source = self.get_source_by_id(video_id)?;
        let video_metadata = source.video.metadata().await?;
        let previews_dir = source.previews_dir();
        let count = previews_dir.previews_count();
        if count > 0 {
            tracing::warn!("Rewriting existing previews")
        }
        let temp_dir = previews_dir.temp_path();
        fs::create_dir_all(&temp_dir).await?;
        let previews_job = ffmpeg::PreviewsJob::new(video_id, source.video.path(), &temp_dir);
        let running_job = ffmpeg::FFmpegRunningJob::spawn(
            &previews_job,
            video_metadata.duration(),
            temp_dir.clone(),
        )?;

        let task_resource = self.tasks;
        self.tasks.tracker.spawn(async move {
            let job_result = task_resource
                .previews_tasks
                .observe_task(previews_job, running_job)
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

    pub async fn detect_intros(&self, show_id: i64, season_number: i64) -> Result<(), AppError> {
        let AppState { db, library, .. } = self;
        let video_ids = sqlx::query!(
            r#"SELECT min(videos.id) as "video_id!", episodes.id as "episode_id!" FROM episodes
        JOIN seasons ON seasons.id = episodes.season_id
        JOIN videos ON videos.content_id = episodes.content_id
        WHERE seasons.show_id = ? AND seasons.number = ?
        GROUP BY episodes.id;"#,
            show_id,
            season_number,
        )
        .fetch_all(&db.pool)
        .await?;
        let paths: Vec<_> = {
            let library = library.lock().unwrap();
            let mut paths = Vec::with_capacity(video_ids.len());
            for row in &video_ids {
                paths.push(
                    library
                        .videos
                        .get(&row.video_id)
                        .map(|s| s.source.video.path().to_path_buf())
                        .ok_or(AppError::internal_error("One of the episodes is not found"))?,
                );
            }
            paths
        };
        let intros = crate::intro_detection::intro_detection(paths).await?;
        let mut tx = db.begin().await?;
        for (i, intro) in intros.into_iter().enumerate() {
            let episode_id = video_ids[i].episode_id;
            if let Some(intro) = intro {
                if let Err(e) = tx.insert_intro(intro.into_db_intro(episode_id)).await {
                    tracing::warn!("Failed to insert intro for episode id({episode_id}): {e}");
                };
            } else {
                tracing::warn!("Could not detect intro for episode with id {episode_id}");
            }
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn reconciliate_library(&self) -> Result<(), AppError> {
        self.partial_refresh().await;
        let start = Instant::now();
        let language: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: language.0 };

        let db_movies_videos = sqlx::query!(
            "SELECT videos.id FROM videos WHERE videos.content_id IN (SELECT movies.content_id FROM movies);"
        )
        .fetch_all(&self.db.pool)
        .await?;

        let new_movies = {
            let movies: Vec<_> = {
                let library = self.library.lock().unwrap();
                library.movies().collect()
            };

            let missing_movies = db_movies_videos
                .iter()
                .filter(|d| !movies.iter().any(|x| x.source.id == d.id));

            for missing_movie in missing_movies {
                let mut tx = self.db.begin().await.context("begin movies removal tx")?;
                match tx.remove_video(missing_movie.id).await {
                    Ok(_) => {
                        let _ = tx.commit().await;
                    }
                    Err(e) => {
                        tracing::error!("Failed to remove video: {e}");
                    }
                };
            }

            let mut new_movies = Vec::new();
            let mut set = JoinSet::new();
            for new_movie in movies
                .into_iter()
                .filter(|l| !db_movies_videos.iter().any(|d| d.id == l.source.id))
            {
                set.spawn(async move {
                    match new_movie.source.video.metadata().await {
                        Ok(_) => Ok(new_movie),
                        Err(e) => {
                            tracing::warn!(
                                path = ?new_movie.source.video.path().display(), "Skipping invalid video: {e}",
                            );
                            Err(e)
                        },
                    }
                });
            }

            while let Some(v) = set.join_next().await {
                match v {
                    Ok(Ok(movie)) => new_movies.push(movie),
                    Ok(Err(_)) => {}
                    Err(e) => panic!("metadata retrieve panicked: {}", e),
                }
            }

            new_movies
        };

        if let Err(e) = scan::movie::scan_movies(
            fetch_params,
            self.db.clone(),
            self.providers_stack,
            new_movies,
        )
        .await
        {
            tracing::error!("Movie scan failed: {e}");
        };

        let db_episodes_videos = sqlx::query!(
            "SELECT videos.id FROM videos WHERE videos.content_id IN (SELECT episodes.content_id FROM episodes);"
        )
        .fetch_all(&self.db.pool)
        .await?;

        let new_episodes = {
            let episodes: Vec<_> = {
                let library = self.library.lock().unwrap();
                library.episodes().collect()
            };

            let missing_episodes = db_episodes_videos
                .iter()
                .filter(|d| !episodes.iter().any(|x| x.source.id == d.id));

            for missing_episode in missing_episodes {
                let mut tx = self.db.begin().await.context("begin episodes removal tx")?;
                match tx.remove_video(missing_episode.id).await {
                    Ok(_) => {
                        let _ = tx.commit().await;
                    }
                    Err(e) => {
                        tracing::error!("Failed to remove video: {e}");
                    }
                };
            }

            let mut new_episodes = Vec::new();
            let mut set = JoinSet::new();
            for new_episode in episodes
                .into_iter()
                .filter(|l| !db_episodes_videos.iter().any(|d| d.id == l.source.id))
            {
                set.spawn(async move {
                    match new_episode.source.video.metadata().await {
                        Ok(_) => Ok(new_episode),
                        Err(e) => {
                            tracing::warn!(
                                path = ?new_episode.source.video.path().display(), "Skipping invalid video: {e}",
                            );
                            Err(e)
                        },
                    }
                });
            }

            while let Some(v) = set.join_next().await {
                match v {
                    Ok(Ok(episode)) => new_episodes.push(episode),
                    Ok(Err(_)) => {}
                    Err(e) => panic!("metadata retrieve panicked: {}", e),
                }
            }

            new_episodes
        };

        if let Err(e) = scan::show::scan_shows(
            fetch_params,
            self.db.clone(),
            self.providers_stack,
            new_episodes,
        )
        .await
        {
            tracing::error!("Failed to scan episodes: {e}");
        };

        tracing::info!(took = ?start.elapsed(), "Finished library reconciliation");
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
                    continue;
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
                library.remove_video(*absent_id);
            }
        }

        let mut tx = self.db.begin().await.unwrap();
        for absent_id in to_remove {
            if let Err(e) = tx.remove_video(absent_id).await {
                tracing::error!("Failed to remove video: {e}");
            };
        }
        tx.commit().await.unwrap();

        explore_show_dirs(show_folders.0, self.db, &mut videos, &show_paths).await;

        explore_movie_dirs(movie_folders.0, self.db, &mut videos, &movie_paths).await;

        self.library.lock().unwrap().videos.extend(videos);
    }
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

impl FromRef<AppState> for &'static MetadataProvidersStack {
    fn from_ref(app_state: &AppState) -> &'static MetadataProvidersStack {
        app_state.providers_stack
    }
}
