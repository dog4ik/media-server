use std::{path::PathBuf, sync::Arc};

use anyhow::anyhow;
use axum::extract::FromRef;
use tokio::sync::Mutex;

use crate::{
    db::Db,
    metadata_provider::{MovieMetadataProvider, ShowMetadataProvider},
    process_file::{AudioCodec, VideoCodec},
    progress::{TaskKind, TaskResource},
    scan::{handle_movie, handle_show, Library},
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

    pub async fn remove_video(&self, video_path: &PathBuf) -> Result<(), anyhow::Error> {
        let library = self.library.lock().await;
        let file = library
            .find(video_path)
            .ok_or(anyhow::anyhow!("path not found in the library"))?;

        file.delete_resources()?;
        drop(library);

        self.db.remove_video_by_path(video_path).await?;
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
