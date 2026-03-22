//! Movie scan pipeline
//!
//! Stages:
//! 1. `fetch_movie_chunks` — parallel movie metadata lookups + duration fetch
//! 2. `merge_movie_chunks` — merge chunks that resolved to the same movie
//! 3. `flush_to_db` — one transaction for all movies
//! 4. `save_assets` — parallel asset downloads/frame extracts

use std::{sync::Arc, time::Duration};

use tokio::{sync::Semaphore, task::JoinSet};
use tokio_util::task::TaskTracker;
use tracing::{Instrument, debug_span};

use crate::{
    db::{Db, DbActions, DbExternalId},
    library::{
        LibraryItem,
        assets::{BackdropAsset, BackdropContentType, PosterAsset, PosterContentType},
        movie::MovieIdentifier,
    },
    metadata::{
        ContentType, DiscoverMetadataProvider, ExternalIdMetadata, MovieMetadata,
        metadata_stack::MetadataProvidersStack,
    },
    scan::insert_roles,
};

use super::{
    AssetKind, AssetSaveTask, AssetTaskSource, MetadataLookupWithIds, ScanConfig,
    fallback::movie_fallback, merge::try_merge_chunks,
};

struct ResolvedMovie {
    lookup: MetadataLookupWithIds<MovieMetadata>,
    duration: Duration,
    videos: Vec<LibraryItem<MovieIdentifier>>,
}

pub struct MovieScanner {
    db: Db,
    providers: &'static MetadataProvidersStack,
    config: ScanConfig,
}

impl MovieScanner {
    pub fn new(db: Db, providers: &'static MetadataProvidersStack, config: ScanConfig) -> Self {
        Self {
            db,
            providers,
            config,
        }
    }

    pub async fn scan(&self, videos: Vec<LibraryItem<MovieIdentifier>>) -> anyhow::Result<()> {
        let resolved = self.fetch_movie_chunks(videos).await;
        let merged = self.merge_movie_chunks(resolved);
        let tasks = self.flush_to_db(merged).await?;
        self.save_assets(tasks).await;
        Ok(())
    }

    async fn fetch_movie_chunks(
        &self,
        mut videos: Vec<LibraryItem<MovieIdentifier>>,
    ) -> Vec<ResolvedMovie> {
        videos.sort_unstable_by(|a, b| {
            a.identifier
                .title
                .to_lowercase()
                .cmp(&b.identifier.title.to_lowercase())
        });

        let discover_providers = self.providers.discover_providers();
        let discover_providers: Arc<[&'static (dyn DiscoverMetadataProvider + Send + Sync)]> =
            Arc::from(discover_providers.into_boxed_slice());

        let semaphore = Arc::new(Semaphore::new(self.config.max_movie_concurrency));
        let mut handles: JoinSet<ResolvedMovie> = JoinSet::new();

        for title_videos in videos
            .chunk_by(|a, b| a.identifier.title.eq_ignore_ascii_case(&b.identifier.title))
            .map(Vec::from)
        {
            let title = title_videos.first().unwrap().identifier.title.clone();
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let db = self.db.clone();
            let config = self.config.clone();
            let discover_providers = discover_providers.clone();
            let span = debug_span!("scan_movie", title = %title);
            handles.spawn(
                async move {
                    let _permit = permit;
                    fetch_single_movie_chunk(db, config, &title, &title_videos, &discover_providers)
                        .await
                }
                .instrument(span),
            );
        }

        handles.join_all().await
    }

    fn merge_movie_chunks(&self, chunks: Vec<ResolvedMovie>) -> Vec<ResolvedMovie> {
        let mut lookups_durations: Vec<(MetadataLookupWithIds<MovieMetadata>, Duration)> =
            Vec::new();
        let mut items: Vec<Vec<LibraryItem<MovieIdentifier>>> = Vec::new();

        for ResolvedMovie {
            lookup,
            duration,
            videos,
        } in chunks
        {
            lookups_durations.push((lookup, duration));
            items.push(videos);
        }

        let statuses: Vec<_> = lookups_durations.iter().map(|(l, _)| l.clone()).collect();
        try_merge_chunks(&statuses, &mut items);

        lookups_durations
            .into_iter()
            .zip(items)
            .filter(|(_, videos)| !videos.is_empty())
            .map(|((lookup, duration), videos)| ResolvedMovie {
                lookup,
                duration,
                videos,
            })
            .collect()
    }

    async fn flush_to_db(
        &self,
        resolved: Vec<ResolvedMovie>,
    ) -> anyhow::Result<Vec<AssetSaveTask>> {
        let span = debug_span!("flush_movies", count = resolved.len());
        let _enter = span.enter();

        let mut asset_tasks = Vec::new();
        let mut tx = self.db.begin().await?;

        for movie in resolved {
            let ResolvedMovie {
                lookup,
                duration,
                videos,
            } = movie;

            let content_id = match lookup {
                MetadataLookupWithIds::New {
                    metadata,
                    external_ids,
                } => {
                    let poster = metadata.poster.clone();
                    let backdrop = metadata.backdrop.clone();
                    let content_id = tx.insert_content(&metadata.into_db_content()).await?;
                    let movie_id = tx
                        .insert_movie(&metadata.into_db_movie(content_id, duration))
                        .await?;
                    if let Some(cast) = metadata.cast {
                        insert_roles(&mut tx, content_id, cast, &mut asset_tasks).await?;
                    }
                    for ext_id in &external_ids {
                        let _ = tx
                            .insert_external_id(DbExternalId {
                                metadata_provider: ext_id.provider,
                                metadata_id: ext_id.id.clone(),
                                content_id: Some(content_id),
                                is_prime: false.into(),
                                ..Default::default()
                            })
                            .await;
                    }
                    let first_source = videos.first().map(|v| v.source.clone());
                    if let Some(url) = poster {
                        let task_source = match first_source.clone() {
                            Some(source) => AssetTaskSource::UrlWithFrameFallback { url, source },
                            None => AssetTaskSource::Url(url),
                        };
                        asset_tasks.push(AssetSaveTask {
                            kind: AssetKind::Poster(PosterAsset::new(
                                movie_id,
                                PosterContentType::Movie,
                            )),
                            source: task_source,
                        });
                    } else if let Some(source) = first_source.clone() {
                        asset_tasks.push(AssetSaveTask {
                            kind: AssetKind::Poster(PosterAsset::new(
                                movie_id,
                                PosterContentType::Movie,
                            )),
                            source: AssetTaskSource::VideoFrame(source),
                        });
                    }
                    if let Some(url) = backdrop {
                        asset_tasks.push(AssetSaveTask {
                            kind: AssetKind::Backdrop(BackdropAsset::new(
                                movie_id,
                                BackdropContentType::Movie,
                            )),
                            source: AssetTaskSource::Url(url),
                        });
                    }
                    content_id
                }
                MetadataLookupWithIds::Local(movie_id) => {
                    sqlx::query!("SELECT content_id FROM movies WHERE id = ?", movie_id)
                        .fetch_one(&mut *tx)
                        .await?
                        .content_id
                }
            };

            for video in &videos {
                tx.update_video_content_id(video.source.id, content_id)
                    .await?;
            }
        }

        tx.commit().await?;
        Ok(asset_tasks)
    }

    async fn save_assets(&self, tasks: Vec<AssetSaveTask>) {
        let span = debug_span!("save_assets", count = tasks.len());
        let _enter = span.enter();
        let semaphore = Arc::new(Semaphore::new(self.config.max_asset_concurrency));
        let tracker = TaskTracker::new();
        for task in tasks {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            tracker.spawn(async move {
                let _permit = permit;
                if let Err(e) = task.execute().await {
                    tracing::warn!("Asset save task failed: {e}");
                }
            });
        }
        tracker.close();
        tracker.wait().await;
    }
}

async fn fetch_single_movie_chunk(
    db: Db,
    config: ScanConfig,
    title: &str,
    videos: &[LibraryItem<MovieIdentifier>],
    discover_providers: &[&'static (dyn DiscoverMetadataProvider + Send + Sync)],
) -> ResolvedMovie {
    let first = videos.first().expect("movies are chunked");
    let db_movies = db.search_movie(title).await.unwrap_or_default();

    if db_movies.is_empty()
        || db_movies.first().unwrap().title.split_whitespace().count()
            != title.split_whitespace().count()
    {
        for provider in discover_providers {
            let Ok(search_results) = provider.movie_search(title, config.fetch_params).await else {
                continue;
            };
            let Some(first_result) = search_results.into_iter().next() else {
                continue;
            };
            match db
                .crossreference_movie(first_result.metadata_provider, &first_result.metadata_id)
                .await
            {
                Ok(Some(local_id)) => {
                    tracing::debug!(movie_title = first_result.title, "Using local movie ref");
                    return ResolvedMovie {
                        lookup: MetadataLookupWithIds::Local(local_id),
                        duration: Duration::ZERO,
                        videos: videos.to_vec(),
                    };
                }
                Ok(None) | Err(_) => {
                    let duration = first
                        .source
                        .video
                        .fetch_duration()
                        .await
                        .unwrap_or_default();
                    let mut external_ids = provider
                        .external_ids(&first_result.metadata_id, ContentType::Movie)
                        .await
                        .inspect_err(|e| tracing::error!("Failed to fetch external ids: {e}"))
                        .unwrap_or_default();
                    external_ids.insert(
                        0,
                        ExternalIdMetadata {
                            provider: first_result.metadata_provider,
                            id: first_result.metadata_id.clone(),
                        },
                    );
                    return ResolvedMovie {
                        lookup: MetadataLookupWithIds::New {
                            metadata: first_result,
                            external_ids,
                        },
                        duration,
                        videos: videos.to_vec(),
                    };
                }
            }
        }

        tracing::warn!("Using movie metadata fallback for: {title}");
        let duration = first
            .source
            .video
            .fetch_duration()
            .await
            .unwrap_or_default();
        ResolvedMovie {
            lookup: movie_fallback(title),
            duration,
            videos: videos.to_vec(),
        }
    } else {
        let local_id = db_movies.first().unwrap().metadata_id.parse().unwrap();
        ResolvedMovie {
            lookup: MetadataLookupWithIds::Local(local_id),
            duration: Duration::ZERO,
            videos: videos.to_vec(),
        }
    }
}
