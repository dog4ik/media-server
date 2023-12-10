use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

use anyhow::anyhow;
use axum::{extract::FromRef, http::StatusCode, response::IntoResponse, Json};
use tokio::{fs, sync::Mutex};

use crate::{
    db::{Db, DbSubtitles},
    library::{movie::MovieFile, show::ShowFile, Library, TranscodePayload},
    metadata::{MovieMetadataProvider, ShowMetadataProvider},
    progress::{TaskError, TaskKind, TaskResource},
    utils,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub library: Arc<Mutex<Library>>,
    pub db: Db,
    pub tasks: TaskResource,
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
    pub async fn reconciliate_library(&self) -> Result<(), AppError> {
        use crate::metadata::tmdb_api::TmdbApi;
        let tmdb_api = TmdbApi::new(
            std::env::var("TMDB_TOKEN")
                .map_err(|_| AppError::internal_error("tmdb token not found"))?,
        );
        let mut library = self.library.lock().await;
        let str = String::from("thing");
        reconciliate_library(&mut library, &self.db, tmdb_api)
            .await
            .map_err(move |_| AppError::not_found(str))
    }

    pub async fn remove_video(&self, id: i64) -> Result<(), AppError> {
        let video_path = sqlx::query!("SELECT path FROM videos WHERE id = ?", id)
            .fetch_one(&self.db.pool)
            .await?
            .path;
        let mut library = self.library.lock().await;
        let file = library
            .find_source(&video_path)
            .ok_or(anyhow::anyhow!("path not found in the library"))?;
        file.delete()
            .map_err(|_| AppError::internal_error("Failed to remove video"))?;
        library.remove_file(video_path);
        self.db.remove_video(id).await?;
        Ok(())
    }

    pub async fn add_show(
        &self,
        video_path: PathBuf,
        metadata_provider: &impl ShowMetadataProvider,
    ) -> Result<(), AppError> {
        let mut library = self.library.lock().await;
        let show = library.add_show(video_path)?;
        drop(library);
        handle_show(show, self.db.clone(), metadata_provider).await?;
        Ok(())
    }

    pub async fn add_movie(
        &self,
        video_path: PathBuf,
        metadata_provider: &impl MovieMetadataProvider,
    ) -> Result<(), AppError> {
        let mut library = self.library.lock().await;
        let movie = library.add_movie(video_path)?;
        handle_movie(movie, self.db.clone(), metadata_provider).await?;
        Ok(())
    }

    pub async fn extract_subs(&self, video_id: i64) -> Result<(), AppError> {
        let path: PathBuf = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id)
            .fetch_one(&self.db.pool)
            .await?
            .path
            .into();
        let library = self.library.lock().await;
        let file = library
            .find_source(&path)
            .ok_or(AppError::not_found("path not found in library"))?;
        let mut jobs = Vec::new();
        let task_id = self
            .tasks
            .add_new_task(file.source_path().to_path_buf(), TaskKind::Subtitles, None)
            .await
            .unwrap();
        let subtitles_path = file.subtitles_path();
        for stream in file.origin.subtitle_streams() {
            if stream.codec().supports_text() {
                let job = file.generate_subtitles(stream.index, stream.language);
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
        self.tasks
            .remove_task(task_id)
            .await
            .expect("task to exist");
        Ok(())
    }

    pub async fn transcode_video(
        &self,
        video_id: i64,
        payload: TranscodePayload,
    ) -> Result<(), AppError> {
        let path: PathBuf = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id)
            .fetch_one(&self.db.pool)
            .await?
            .path
            .into();

        let library = self.library.lock().await;
        let source = library
            .find_source(&path)
            .ok_or(anyhow!("path not found in library"))?;

        let job = source.transcode_video(payload)?;

        let run_result = self.tasks.run_ffmpeg_task(job, TaskKind::Transcode).await;

        if let Err(err) = run_result {
            match err {
                TaskError::Canceled => todo!(),
                TaskError::NotFound => todo!(),
                _ => todo!(),
            }
            // cancel logic
        }
        Ok(())
    }

    pub async fn generate_previews(&self, video_id: i64) -> Result<(), TaskError> {
        let path: PathBuf = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id)
            .fetch_one(&self.db.pool)
            .await
            .map_err(|_| TaskError::NotFound)?
            .path
            .into();

        let library = self.library.lock().await;
        let file = library.find_source(&path).ok_or(TaskError::NotFound)?;
        let previews_path = file.previews_path();

        if (file.previews_count() as f64) < (file.duration().as_secs() as f64 / 10.0).round() {
            let job = file.generate_previews();

            let run_result = self.tasks.run_ffmpeg_task(job, TaskKind::Previews).await;

            if let Err(err) = run_result {
                if let TaskError::Canceled = err {
                    let _ = utils::clear_directory(previews_path).await;
                }
                return Err(err);
            }
        }
        Ok(())
    }
}

impl FromRef<AppState> for Arc<Mutex<Library>> {
    fn from_ref(app_state: &AppState) -> Arc<Mutex<Library>> {
        app_state.library.clone()
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
pub async fn reconciliate_library(
    library: &mut Library,
    db: &Db,
    metadata_provider: impl ShowMetadataProvider + Send + Sync + 'static,
) -> Result<(), sqlx::Error> {
    let metadata_provider = Arc::new(metadata_provider);
    let db_episodes_videos = sqlx::query!(
        r#"SELECT videos.*, episodes.id as "episode_id!" FROM videos
        JOIN episodes ON videos.id = episodes.video_id"#
    )
    .fetch_all(&db.pool)
    .await?;
    library.full_refresh().await;
    let local_episodes = &library.shows;
    let mut common_paths: HashSet<&str> = HashSet::new();

    let local_episodes: HashMap<String, &ShowFile> = local_episodes
        .iter()
        .map(|ep| (ep.source.source_path().to_str().unwrap().into(), ep))
        .collect();

    for db_episode_video in &db_episodes_videos {
        let exists_in_db;
        let size_match;
        if let Some(local_eqivalent) = local_episodes.get(&db_episode_video.path) {
            exists_in_db = true;
            size_match = local_eqivalent.source.origin.file_size() == db_episode_video.size as u64;
        } else {
            size_match = false;
            exists_in_db = false;
        }

        if exists_in_db && size_match {
            common_paths.insert(db_episode_video.path.as_str());
        };
    }

    // clean up variants

    // clean up db
    for db_episode_video in &db_episodes_videos {
        if !common_paths.contains(db_episode_video.path.as_str()) {
            db.remove_episode(db_episode_video.episode_id).await?;
        }
    }

    let mut handles = Vec::new();

    for (local_ep_path, local_ep) in local_episodes {
        // skip existing media
        if common_paths.contains(local_ep_path.as_str()) {
            continue;
        }

        let local_ep = local_ep.clone();
        let db = db.clone();
        let metadata_provider = metadata_provider.clone();
        let handle = tokio::spawn(async move {
            let _ = handle_show(local_ep, db, &*metadata_provider).await;
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
    Ok(())
}

pub async fn handle_show(
    show: ShowFile,
    db: Db,
    metadata_provider: &impl ShowMetadataProvider,
) -> Result<(), AppError> {
    // BUG: what happens if local title changes? Duplicate shows in db.
    // We'll be fine if we avoid dublicate and insert video with different title.
    // After failure it will lookup title in provider and match it again
    let show_query = sqlx::query!(
        r#"SELECT shows.id, shows.metadata_id, shows.metadata_provider FROM episodes 
                    JOIN videos ON videos.id = episodes.video_id
                    JOIN seasons ON seasons.id = episodes.season_id
                    JOIN shows ON shows.id = seasons.show_id
                    WHERE videos.local_title = ?;"#,
        show.local_title
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| (x.id, x.metadata_id.unwrap(), x.metadata_provider));

    let (show_id, show_metadata_id, _show_metadata_provider) = match show_query {
        Ok(data) => data,
        Err(e) => match e {
            sqlx::Error::RowNotFound => {
                let metadata = metadata_provider
                    .show(&show.local_title)
                    .await
                    .map_err(|err| {
                        tracing::error!(
                            "Metadata lookup failed for file with local title: {}",
                            show.local_title
                        );
                        err
                    })?;
                let provider = metadata.metadata_provider.to_string();
                let metadata_id = metadata.metadata_id.clone().unwrap();
                let db_show = metadata.into_db_show().await;
                (db.insert_show(db_show).await?, metadata_id, provider)
            }
            _ => {
                return Err(e)?;
            }
        },
    };

    let season_id = sqlx::query!(
        "SELECT id FROM seasons WHERE show_id = ? AND number = ?",
        show_id,
        show.season
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| x.id);

    let season_id = match season_id {
        Ok(season_id) => season_id,
        Err(e) => match e {
            sqlx::Error::RowNotFound => {
                let metadata = metadata_provider
                    .season(&show_metadata_id, show.season.into())
                    .await
                    .unwrap();
                let db_season = metadata.into_db_season(show_id).await;
                db.insert_season(db_season).await?
            }
            _ => {
                return Err(e)?;
            }
        },
    };

    let episode_id = sqlx::query!(
        "SELECT id FROM episodes WHERE season_id = ? AND number = ?;",
        season_id,
        show.episode
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| x.id);

    if let Err(e) = episode_id {
        if let sqlx::Error::RowNotFound = e {
            let metadata = metadata_provider
                .episode(
                    &show_metadata_id,
                    show.season as usize,
                    show.episode as usize,
                )
                .await
                .unwrap();
            let db_video = show.source.into_db_video(show.local_title.clone());
            let video_id = db.insert_video(db_video).await?;
            for variant in &show.source.variants {
                let db_variant = variant.into_db_variant(video_id);
                db.insert_variant(db_variant).await?;
            }
            let db_episode = metadata.into_db_episode(season_id, video_id).await;
            db.insert_episode(db_episode).await?;
        } else {
            tracing::error!("Unexpected error while fetching episode {}", e);
            return Err(e)?;
        }
    };
    Ok(())
}

pub async fn handle_movie(
    movie: MovieFile,
    db: Db,
    metadata_provider: &impl MovieMetadataProvider,
) -> Result<(), sqlx::Error> {
    let movie_query = sqlx::query!(
        r#"SELECT movies.id as "id!", movies.metadata_id, movies.metadata_provider FROM movies 
                    JOIN videos ON videos.id = movies.video_id
                    WHERE videos.local_title = ?;"#,
        movie.local_title
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| (x.id, x.metadata_id.unwrap(), x.metadata_provider));

    match movie_query {
        Err(e) => {
            if let sqlx::Error::RowNotFound = e {
                let metadata = metadata_provider.movie(&movie.local_title).await.unwrap();
                let db_video = movie.source.into_db_video(movie.local_title);
                let video_id = db.insert_video(db_video).await?;
                let provider = metadata.metadata_provider.to_string();
                let metadata_id = metadata.metadata_id.clone().unwrap();
                let db_movie = metadata.into_db_movie(video_id).await;
                (db.insert_movie(db_movie).await?, metadata_id, provider);
            } else {
                return Err(e);
            }
        }
        _ => (),
    };

    Ok(())
}
