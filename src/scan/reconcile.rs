//! Library reconciliation service.
//!
//! Diffs the in-memory library against the database, fetches metadata for newly added
//! videos through the show/movie scanners, flushes the resolved trees to the database, and
//! saves the associated assets to disk.

use std::{collections::HashSet, sync::Arc, sync::Mutex};

use anyhow::Context;
use tokio::{sync::Semaphore, task::JoinSet};
use tokio_util::task::TaskTracker;

use crate::{
    app_state::AppError,
    config,
    db::{Db, DbActions},
    library::{Library, LibraryItem, Media},
    metadata::{FetchParams, metadata_stack::MetadataProvidersStack},
};

use super::{
    AssetSaveTask, ContentScanner, ScanConfig,
    movie::MovieScanner,
    scan_progress::{AssetProgressEmitter, ScanProgressEmitter},
    show::ShowScanner,
};

/// Reconciles the library with the database, fetching metadata and saving assets.
pub struct LibraryReconciler {
    library: &'static Mutex<Library>,
    db: &'static Db,
    providers: &'static MetadataProvidersStack,
    progress: ScanProgressEmitter,
}

impl LibraryReconciler {
    pub fn new(
        library: &'static Mutex<Library>,
        db: &'static Db,
        providers: &'static MetadataProvidersStack,
        progress: ScanProgressEmitter,
    ) -> Self {
        Self {
            library,
            db,
            providers,
            progress,
        }
    }

    #[tracing::instrument(name = "reconcile", skip_all)]
    pub async fn reconciliate(self) -> Result<(), AppError> {
        let language: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: language.0 };

        let db_movies_videos = sqlx::query!(
            "SELECT videos.id FROM videos WHERE videos.metadata_id IN (SELECT movies.metadata_id FROM movies);"
        )
        .fetch_all(&self.db.pool)
        .await?;

        let movies: Vec<_> = {
            let library = self.library.lock().unwrap();
            library.movies().collect()
        };
        let new_movies = self
            .collect_new_videos(
                movies,
                db_movies_videos.iter().map(|d| d.id).collect(),
                "begin movies removal tx",
            )
            .await?;

        let config = ScanConfig {
            fetch_params,
            ..ScanConfig::default()
        };
        let max_asset_concurrency = config.max_asset_concurrency;

        let movie_scanner = MovieScanner::new(self.db.clone(), self.providers, config.clone());

        let db_episodes_videos = sqlx::query!(
            "SELECT videos.id FROM videos WHERE videos.metadata_id IN (SELECT episodes.metadata_id FROM episodes);"
        )
        .fetch_all(&self.db.pool)
        .await?;

        let episodes: Vec<_> = {
            let library = self.library.lock().unwrap();
            library.episodes().collect()
        };
        let new_episodes = self
            .collect_new_videos(
                episodes,
                db_episodes_videos.iter().map(|d| d.id).collect(),
                "begin episodes removal tx",
            )
            .await?;

        let show_scanner = ShowScanner::new(self.db.clone(), self.providers, config);
        let metadata_progress = self
            .progress
            .metadata_progress_emitter(new_episodes.len() + new_movies.len());

        let (resolved_shows, resolved_movies) = tokio::join!(
            show_scanner.resolve(new_episodes, metadata_progress.clone()),
            movie_scanner.resolve(new_movies, metadata_progress),
        );

        let mut tasks = Vec::new();
        let mut tx = self.db.begin().await?;
        show_scanner
            .flush_to_db(&mut tx, &mut tasks, resolved_shows)
            .await?;
        movie_scanner
            .flush_to_db(&mut tx, &mut tasks, resolved_movies)
            .await?;
        tx.commit().await?;
        let assets_progress = self.progress.assets_progress_emitter(tasks.len());
        self.save_assets(max_asset_concurrency, assets_progress, tasks)
            .await;
        self.progress.finish_scan();
        tracing::info!("Finished library reconciliation");
        Ok(())
    }

    /// Diffs the in-memory `items` against the videos already known to the database,
    /// removing DB entries for videos that no longer exist on disk and returning the freshly
    /// added videos whose container metadata could be read (invalid ones are skipped).
    async fn collect_new_videos<I>(
        &self,
        items: Vec<LibraryItem<I>>,
        db_video_ids: Vec<i64>,
        removal_context: &'static str,
    ) -> Result<Vec<LibraryItem<I>>, AppError>
    where
        I: Media + Send + 'static,
    {
        let library_ids: HashSet<i64> = items.iter().map(|i| i.source.id).collect();
        let db_ids: HashSet<i64> = db_video_ids.iter().copied().collect();

        let mut tx = self.db.begin().await.context(removal_context)?;
        for missing_id in db_video_ids.iter().filter(|id| !library_ids.contains(id)) {
            if let Err(e) = tx.remove_video(*missing_id).await {
                tracing::error!("Failed to remove video: {e}");
            };
        }
        tx.commit().await?;

        let mut new_items = Vec::new();
        let mut set = JoinSet::new();
        for item in items.into_iter().filter(|l| !db_ids.contains(&l.source.id)) {
            set.spawn(async move {
                match item.source.video.metadata().await {
                    Ok(_) => Some(item),
                    Err(e) => {
                        tracing::warn!(
                            path = ?item.source.video.path().display(), "Skipping invalid video: {e}",
                        );
                        None
                    }
                }
            });
        }

        while let Some(v) = set.join_next().await {
            match v {
                Ok(Some(item)) => new_items.push(item),
                Ok(None) => {}
                Err(e) => panic!("metadata retrieve panicked: {}", e),
            }
        }

        Ok(new_items)
    }

    /// Executes queued asset save tasks concurrently, reporting per-task progress.
    async fn save_assets(
        &self,
        max_concurrency: usize,
        emitter: AssetProgressEmitter,
        tasks: Vec<AssetSaveTask>,
    ) {
        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        let tracker = TaskTracker::new();
        for task in tasks {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let emitter = emitter.clone();
            tracker.spawn(async move {
                let _permit = permit;
                if let Err(e) = task.execute().await {
                    emitter.dispatch_fail();
                    tracing::warn!("Asset save task failed: {e}");
                } else {
                    emitter.dispatch_success();
                }
            });
        }
        tracker.close();
        tracker.wait().await;
    }
}
