use std::{collections::HashMap, sync::Mutex};

use axum::extract::FromRef;
use tokio::fs;
use tokio_util::sync::CancellationToken;

use crate::{
    AppError,
    config::{self},
    db::{Db, DbActions},
    ffmpeg::{self, FFmpegRunningJob, TranscodeJob},
    library::{
        ContentIdentifier, Library, Source, TranscodePayload,
        assets::{AssetDir, FileAsset, VariantAsset},
        explore_movie_dirs, explore_show_dirs,
        media::Video,
    },
    metadata::{FetchParams, metadata_stack::MetadataProvidersStack},
    progress::{ProgressDispatcher, TaskResource},
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
    pub http_client: reqwest::Client,
    pub cancelation_token: CancellationToken,
}

impl AppState {
    pub fn metadata_fetch_params(&self) -> FetchParams {
        let language: config::MetadataLanguage = config::CONFIG.get_value();
        FetchParams { lang: language.0 }
    }

    pub fn get_source_by_id(&self, id: i64) -> crate::Result<Source> {
        let library = self.library.lock().unwrap();
        library
            .get_source(id)
            .ok_or(AppError::not_found("file with path from db is not found"))
            .cloned()
    }

    #[tracing::instrument(skip(self, id), fields(video_id = id))]
    pub async fn remove_video(&self, id: i64) -> crate::Result<()> {
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

    #[tracing::instrument(skip(self, id), fields(movie_id = id))]
    pub async fn delete_movie(&self, id: i64) -> crate::Result<()> {
        let mut tx = self.db.begin().await?;
        let ids = sqlx::query!(
            "SELECT videos.id FROM videos JOIN movies ON movies.metadata_id = videos.metadata_id WHERE movies.id = ?",
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

    #[tracing::instrument(skip(self, id), fields(season_id = id))]
    pub async fn delete_season(&self, id: i64) -> crate::Result<()> {
        let mut tx = self.db.begin().await?;
        let ids = sqlx::query!(
            "SELECT videos.id FROM videos JOIN episodes ON episodes.metadata_id = videos.metadata_id WHERE episodes.season_id = ?",
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

    #[tracing::instrument(skip(self, id), fields(show_id = id))]
    pub async fn delete_show(&self, id: i64) -> crate::Result<()> {
        let mut tx = self.db.begin().await?;
        let ids = sqlx::query!(
            "SELECT videos.id FROM videos
JOIN episodes ON episodes.metadata_id = videos.metadata_id
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

    #[tracing::instrument(skip(self, id), fields(episode_id = id))]
    pub async fn delete_episode(&self, id: i64) -> crate::Result<()> {
        let mut tx = self.db.begin().await?;
        let ids = sqlx::query!(
            "SELECT videos.id FROM videos JOIN episodes ON episodes.metadata_id = videos.metadata_id WHERE episodes.id = ?",
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

    #[tracing::instrument(skip(self))]
    pub async fn remove_variant(&self, video_id: i64, variant_id: &str) -> crate::Result<()> {
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

    /// Get subtitle track from video file without saving it. Takes some time to run ffmpeg
    #[tracing::instrument(skip(self))]
    pub async fn pull_subtitle_from_video(
        &self,
        video_id: i64,
        subs_track: usize,
    ) -> crate::Result<String> {
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

    #[tracing::instrument(skip(self, payload))]
    pub async fn transcode_video(
        &self,
        video_id: i64,
        payload: TranscodePayload,
    ) -> crate::Result<()> {
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

    #[tracing::instrument(skip(self))]
    pub async fn generate_previews(&self, video_id: i64) -> crate::Result<()> {
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

    #[tracing::instrument(skip(self))]
    pub async fn detect_intros(&self, show_id: i64, season_number: i64) -> crate::Result<()> {
        let AppState { db, library, .. } = self;
        let video_ids = sqlx::query!(
            r#"SELECT min(videos.id) as "video_id!: i64", episodes.id as "episode_id!" FROM episodes
        JOIN seasons ON seasons.id = episodes.season_id
        JOIN videos ON videos.metadata_id = episodes.metadata_id
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

    #[tracing::instrument(skip(self), fields(%task_id))]
    pub async fn reconciliate_library(
        &self,
        task_id: uuid::Uuid,
        config: scan::ScanConfig,
    ) -> crate::Result<()> {
        self.partial_refresh().await;
        let progress = scan::scan_progress::ScanProgressEmitter::new(ProgressDispatcher::new(
            &self.tasks.library_scan_tasks,
            task_id,
        ));
        scan::reconcile::LibraryReconciler::new(
            self.library,
            self.db,
            self.providers_stack,
            progress,
            self.http_client.clone(),
        )
        .reconciliate(config)
        .await
    }

    #[tracing::instrument(skip(self))]
    pub async fn partial_refresh(&self) {
        tracing::info!("Partially refreshing library");
        let mut videos = HashMap::new();
        let mut to_remove = Vec::new();
        let (config::ShowFolders(show_folders), config::MovieFolders(movie_folders)) =
            config::CONFIG.get_values();
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
                        if !show_folders.iter().any(|p| file_path.starts_with(p)) {
                            to_remove.push(*id);
                        } else {
                            show_paths.push(file.source.video.path().to_owned());
                        }
                    }
                    ContentIdentifier::Movie(_) => {
                        if !movie_folders.iter().any(|p| file_path.starts_with(p)) {
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

        explore_show_dirs(show_folders, self.db, &mut videos, &show_paths).await;

        explore_movie_dirs(movie_folders, self.db, &mut videos, &movie_paths).await;

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

impl FromRef<AppState> for &'static Db {
    fn from_ref(app_state: &AppState) -> &'static Db {
        app_state.db
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
