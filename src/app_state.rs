use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use axum::{extract::FromRef, http::StatusCode, response::IntoResponse, Json};
use tokio::{fs, sync::Semaphore};

use crate::{
    config::ServerConfiguration,
    db::{Db, DbSubtitles},
    library::{
        movie::MovieFile, show::ShowFile, Library, LibraryItem, Source, TranscodePayload, Video,
    },
    metadata::{MovieMetadataProvider, ShowMetadataProvider},
    progress::{TaskError, TaskKind, TaskResource},
    utils,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub library: &'static Mutex<Library>,
    pub db: Db,
    pub tasks: TaskResource,
    pub configuration: &'static Mutex<ServerConfiguration>,
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
    Dubplicate,
    BadRequest,
}

impl Into<StatusCode> for AppErrorKind {
    fn into(self) -> StatusCode {
        match self {
            AppErrorKind::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            AppErrorKind::NotFound => StatusCode::NOT_FOUND,
            AppErrorKind::Dubplicate => StatusCode::BAD_REQUEST,
            AppErrorKind::BadRequest => StatusCode::BAD_REQUEST,
        }
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self {
            message: err.into().to_string(),
            kind: AppErrorKind::InternalError,
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

    pub async fn add_show(
        &self,
        video_path: PathBuf,
        metadata_provider: &impl ShowMetadataProvider,
    ) -> Result<(), AppError> {
        let show = {
            let mut library = self.library.lock().unwrap();
            library.add_show(video_path)?
        };
        self.handle_show(show, metadata_provider).await
    }

    pub async fn add_movie(
        &self,
        video_path: PathBuf,
        metadata_provider: &impl MovieMetadataProvider,
    ) -> Result<(), AppError> {
        let movie = {
            let mut library = self.library.lock().unwrap();
            library.add_movie(video_path)?
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
                source.source_path().to_path_buf(),
                TaskKind::Subtitles,
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
            .observe_ffmpeg_task(job, TaskKind::Transcode)
            .await?;
        let variant_video = Video::from_path(output_path).expect("file to be done transcoding");

        let mut library = self.library.lock().unwrap();
        library.add_variant(source.source_path(), variant_video);

        Ok(())
    }

    #[tracing::instrument]
    pub async fn generate_previews(&self, video_id: i64) -> Result<(), TaskError> {
        let file = self
            .get_file_by_id(video_id)
            .await
            .map_err(|_| TaskError::NotFound)?;

        if (file.previews_count() as f64) < (file.duration().as_secs() as f64 / 10.0).round() {
            tracing::warn!("Rewriting existing previews")
        }

        let job = file.generate_previews();

        let run_result = self
            .tasks
            .observe_ffmpeg_task(job, TaskKind::Previews)
            .await;

        if let Err(err) = run_result {
            if let TaskError::Canceled = err {
                let _ = utils::clear_directory(file.previews_path()).await;
            }
            return Err(err);
        }
        Ok(())
    }

    pub async fn reconciliate_library(&self) -> Result<(), AppError> {
        // TODO: Custom metadata provider
        use crate::metadata::tmdb_api::TmdbApi;
        let tmdb_api = TmdbApi::new(
            std::env::var("TMDB_TOKEN")
                .map_err(|_| AppError::internal_error("tmdb token not found"))?,
        );

        let local_episodes = {
            let library = self.library.lock().unwrap();
            library.shows.clone()
        };

        let metadata_provider = Arc::new(tmdb_api);

        let db_episodes_videos = sqlx::query!(
            r#"SELECT videos.*, episodes.id as "episode_id!" FROM videos
        JOIN episodes ON videos.id = episodes.video_id"#
        )
        .fetch_all(&self.db.pool)
        .await?;
        let mut common_paths: HashSet<&str> = HashSet::new();

        let local_episodes: HashMap<String, &ShowFile> = local_episodes
            .iter()
            .map(|ep| (ep.source.source_path().to_string_lossy().to_string(), ep))
            .collect();

        //TODO: add hashsum check
        for db_episode_video in &db_episodes_videos {
            if let Some(local_eqivalent) = local_episodes.get(&db_episode_video.path) {
                if local_eqivalent.source.origin.file_size() == db_episode_video.size as u64 {
                    common_paths.insert(db_episode_video.path.as_str());
                }
            }
        }

        // clean up variants and resources

        // clean up db
        for db_episode_video in &db_episodes_videos {
            if !common_paths.contains(db_episode_video.path.as_str()) {
                tracing::info!("Removing not existing episode: {}", db_episode_video.path);
                self.db.remove_episode(db_episode_video.episode_id).await?;
            }
        }

        let mut handles = Vec::new();
        let show_semaphore = Arc::new(Semaphore::new(10));

        for (local_ep_path, local_ep) in local_episodes {
            // skip existing media
            if common_paths.contains(local_ep_path.as_str()) {
                continue;
            }

            let local_ep = local_ep.clone();
            let metadata_provider = metadata_provider.clone();
            let app_state = self.clone();
            let semaphore = show_semaphore.clone();
            let handle = tokio::spawn(async move {
                let permit = semaphore.acquire().await.unwrap();
                let task_id = app_state.tasks.start_task(
                    local_ep.source_path().clone(),
                    TaskKind::Scan,
                    None,
                );
                if let Ok(task_id) = task_id {
                    let scan_result = app_state.handle_show(local_ep, &*metadata_provider).await;
                    match scan_result {
                        Err(_err) => {
                            app_state.tasks.error_task(task_id);
                        }
                        Ok(_) => {
                            app_state.tasks.finish_task(task_id);
                        }
                    };
                }
                drop(permit);
            });
            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        tracing::info!("Finished library reconciliation");
        Ok(())
    }

    pub async fn handle_show(
        &self,
        show: ShowFile,
        metadata_provider: &impl ShowMetadataProvider,
    ) -> Result<(), AppError> {
        // BUG: what happens if local title changes? Duplicate shows in db.
        // We'll be fine if we avoid dublicate and insert video with different title.
        // After failure it will lookup title in provider and match it again
        let show_query = sqlx::query!(
            r#"SELECT shows.id, shows.metadata_id as "metadata_id!", shows.metadata_provider FROM episodes 
                    JOIN videos ON videos.id = episodes.video_id
                    JOIN seasons ON seasons.id = episodes.season_id
                    JOIN shows ON shows.id = seasons.show_id
                    WHERE videos.local_title = ?;"#,
            show.local_title
        )
        .fetch_one(&self.db.pool)
        .await
        .map(|x| (x.id, x.metadata_id, x.metadata_provider));

        let (show_id, show_metadata_id, _show_metadata_provider) = match show_query {
            Ok(data) => data,
            Err(e) => match e {
                sqlx::Error::RowNotFound => {
                    tracing::debug!(
                        "Show {} is not found in local DB, fetching metadata from {}",
                        show.local_title,
                        metadata_provider.provider_identifier()
                    );
                    let metadata = metadata_provider.show(&show).await.map_err(|err| {
                        tracing::error!(
                            "Metadata lookup failed for file with local title: {}",
                            show.local_title
                        );
                        err
                    })?;
                    let provider = metadata.metadata_provider.to_string();
                    let metadata_id = metadata.metadata_id.clone().unwrap();
                    let db_show = metadata.into_db_show().await;
                    (self.db.insert_show(db_show).await?, metadata_id, provider)
                }
                _ => {
                    tracing::error!("Unexpected database error when fetching show: {}", e);
                    return Err(e)?;
                }
            },
        };

        let season_id = sqlx::query!(
            "SELECT id FROM seasons WHERE show_id = ? AND number = ?",
            show_id,
            show.season
        )
        .fetch_one(&self.db.pool)
        .await
        .map(|x| x.id);

        let season_id = match season_id {
            Ok(season_id) => season_id,
            Err(e) => match e {
                sqlx::Error::RowNotFound => {
                    tracing::debug!(
                        "Season {} of show {} is not found in local DB, fetching metadata from {}",
                        show.season,
                        show.local_title,
                        metadata_provider.provider_identifier()
                    );
                    let metadata = metadata_provider
                        .season(&show_metadata_id, show.season.into())
                        .await?;
                    let db_season = metadata.into_db_season(show_id).await;
                    self.db.insert_season(db_season).await?
                }
                _ => {
                    tracing::error!("Unexpected database error when fetching season: {}", e);
                    return Err(e)?;
                }
            },
        };

        let episode_id = sqlx::query!(
            "SELECT id FROM episodes WHERE season_id = ? AND number = ?;",
            season_id,
            show.episode
        )
        .fetch_one(&self.db.pool)
        .await
        .map(|x| x.id);

        if let Err(e) = episode_id {
            if let sqlx::Error::RowNotFound = e {
                tracing::debug!(
                    "Episode {} of show {}(season {}) is not found in local DB, fetching metadata from {}",
                    show.episode,
                    show.local_title,
                    show.season,
                    metadata_provider.provider_identifier()
                );
                let metadata = metadata_provider
                    .episode(
                        &show_metadata_id,
                        show.season as usize,
                        show.episode as usize,
                    )
                    .await?;
                let db_video = show.source.into_db_video(show.local_title.clone());
                let video_id = self.db.insert_video(db_video).await?;
                let db_episode = metadata.into_db_episode(season_id, video_id).await;
                self.db.insert_episode(db_episode).await?;
            } else {
                tracing::error!("Unexpected database error when fetching episode: {}", e);
                return Err(e)?;
            }
        };
        Ok(())
    }

    pub async fn handle_movie(
        &self,
        movie: MovieFile,
        metadata_provider: &impl MovieMetadataProvider,
    ) -> Result<(), AppError> {
        let movie_query = sqlx::query!(
            r#"SELECT movies.id as "id!", movies.metadata_id, movies.metadata_provider FROM movies 
                    JOIN videos ON videos.id = movies.video_id
                    WHERE videos.local_title = ?;"#,
            movie.local_title
        )
        .fetch_one(&self.db.pool)
        .await
        .map(|x| (x.id, x.metadata_id.unwrap(), x.metadata_provider));

        match movie_query {
            Err(e) => {
                if let sqlx::Error::RowNotFound = e {
                    tracing::debug!(
                        "Movie {} is not found in local DB, fetching metadata from {}",
                        movie.local_title,
                        metadata_provider.provider_identifier()
                    );
                    let metadata = metadata_provider.movie(&movie).await.unwrap();
                    let db_video = movie.source.into_db_video(movie.local_title);
                    let video_id = self.db.insert_video(db_video).await?;
                    let provider = metadata.metadata_provider.to_string();
                    let metadata_id = metadata.metadata_id.clone().unwrap();
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

impl FromRef<AppState> for &'static Mutex<Library> {
    fn from_ref(app_state: &AppState) -> &'static Mutex<Library> {
        app_state.library
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
