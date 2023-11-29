use std::{path::PathBuf, sync::Arc};

use anyhow::anyhow;
use axum::extract::FromRef;
use tokio::{fs, sync::Mutex};

use crate::{
    db::{Db, DbSubtitles},
    metadata_provider::{MovieMetadataProvider, ShowMetadataProvider},
    process_file::{AudioCodec, VideoCodec},
    progress::{TaskError, TaskKind, TaskResource},
    scan::{handle_movie, handle_show, Library},
    utils,
};

#[derive(Debug, Clone)]
pub struct AppState {
    pub library: Arc<Mutex<Library>>,
    pub db: Db,
    pub tasks: TaskResource,
}

impl AppState {
    pub async fn reconciliate_library(&self) -> Result<(), sqlx::Error> {
        use crate::tmdb_api::TmdbApi;
        let tmdb_api = TmdbApi::new(std::env::var("TMDB_TOKEN").expect("tmdb token to be in env"));
        let mut library = self.library.lock().await;
        library.reconciliate_library(&self.db, tmdb_api).await
    }

    pub async fn remove_video(&self, id: i64) -> Result<(), anyhow::Error> {
        let video_path = sqlx::query!("SELECT path FROM videos WHERE id = ?", id)
            .fetch_one(&self.db.pool)
            .await?
            .path;
        let library = self.library.lock().await;
        let file = library
            .find(video_path)
            .ok_or(anyhow::anyhow!("path not found in the library"))?;

        file.delete()?;
        self.db.remove_video(id).await?;
        Ok(())
    }

    pub async fn add_show(
        &self,
        video_path: PathBuf,
        metadata_provider: &impl ShowMetadataProvider,
    ) -> Result<(), anyhow::Error> {
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
    ) -> Result<(), anyhow::Error> {
        let mut library = self.library.lock().await;
        let movie = library.add_movie(video_path)?;
        handle_movie(movie, self.db.clone(), metadata_provider).await?;
        Ok(())
    }

    pub async fn extract_subs(&self, video_id: i64) -> Result<(), anyhow::Error> {
        let path: PathBuf = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id)
            .fetch_one(&self.db.pool)
            .await?
            .path
            .into();
        let library = self.library.lock().await;
        let file = library
            .find(&path)
            .ok_or(anyhow!("path not found in library"))?;
        let metadata = file.metadata();
        let mut jobs = Vec::new();
        let task_id = self
            .tasks
            .add_new_task(file.source_path().to_path_buf(), TaskKind::Subtitles, None)
            .await
            .unwrap();
        let subtitles_path = file.subtitles_path();
        for stream in metadata.subtitle_streams() {
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
        video_codec: Option<VideoCodec>,
        audio_codec: Option<AudioCodec>,
    ) -> Result<(), anyhow::Error> {
        let path: PathBuf = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id)
            .fetch_one(&self.db.pool)
            .await?
            .path
            .into();

        let library = self.library.lock().await;
        let file = library
            .find(&path)
            .ok_or(anyhow!("path not found in library"))?;
        let metadata = file.metadata();

        let audio_stream = metadata
            .default_audio()
            .ok_or(anyhow!("video does not contain audio stream"))?;

        let job = file.transcode_video(
            video_codec,
            audio_codec.map(|c| (audio_stream.index as usize, c)),
        )?;

        let run_result = self.tasks.run_ffmpeg_task(job, TaskKind::Transcode).await;

        match run_result {
            Ok(_) => {}
            Err(err) => {
                // cancel logic
            }
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
        let file = library.find(&path).ok_or(TaskError::NotFound)?;
        let previews_path = file.previews_path();

        if (file.previews_count() as f64) < (file.get_duration().as_secs() as f64 / 10.0).round() {
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
