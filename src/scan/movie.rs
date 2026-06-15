//! Movie scan pipeline
//!
//! Stages:
//! 1. `fetch_movie_chunks` — parallel movie metadata lookups + duration fetch
//! 2. `merge_movie_chunks` — merge chunks that resolved to the same movie
//! 3. `flush_to_db` — one transaction for all movies
//! 4. `save_assets` — parallel asset downloads/frame extracts

use std::{sync::Arc, time::Duration};

use tokio::{sync::Semaphore, task::JoinSet};
use tracing::{Instrument, debug_span};

use crate::{
    db::{Db, DbActions, DbExternalId, DbTransaction},
    library::{
        LibraryItem,
        assets::{BackdropAsset, BackdropContentType, PosterAsset, PosterContentType},
        movie::MovieIdentifier,
    },
    metadata::{
        ContentType, ExternalIdMetadata, MovieMetadata, MovieMetadataProvider,
        metadata_stack::MetadataProvidersStack,
    },
    scan::{
        ContentScanner, insert_roles,
        scan_progress::{FailedContent, MetadataProgressEmitter},
    },
};

use super::{
    AssetKind, AssetSaveTask, AssetTaskSource, MetadataLookupWithIds, ScanConfig,
    fallback::movie_fallback, merge::try_merge_chunks,
};

pub struct ResolvedMovie {
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

    async fn fetch_movie_chunks(
        &self,
        mut videos: Vec<LibraryItem<MovieIdentifier>>,
        progress: MetadataProgressEmitter,
    ) -> Vec<ResolvedMovie> {
        videos.sort_unstable_by(|a, b| {
            a.identifier
                .title
                .to_lowercase()
                .cmp(&b.identifier.title.to_lowercase())
        });

        let movie_providers = self.providers.movie_providers();
        let movie_providers: Arc<[&'static (dyn MovieMetadataProvider + Send + Sync)]> =
            Arc::from(movie_providers.into_boxed_slice());

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
            let movie_providers = movie_providers.clone();
            let progress = progress.clone();
            let span = debug_span!("scan_movie", title = %title);
            handles.spawn(
                async move {
                    let _permit = permit;
                    fetch_single_movie_chunk(
                        db,
                        config,
                        title,
                        &title_videos,
                        &movie_providers,
                        &progress,
                    )
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
}

impl ContentScanner for MovieScanner {
    type Identifier = MovieIdentifier;
    type Resolved = ResolvedMovie;

    async fn resolve(
        &self,
        videos: Vec<LibraryItem<MovieIdentifier>>,
        progress: MetadataProgressEmitter,
    ) -> Vec<ResolvedMovie> {
        let resolved = self.fetch_movie_chunks(videos, progress).await;
        self.merge_movie_chunks(resolved)
    }

    async fn flush_to_db(
        &self,
        tx: &mut DbTransaction,
        asset_tasks: &mut Vec<AssetSaveTask>,
        resolved: Vec<ResolvedMovie>,
    ) -> sqlx::Result<()> {
        let span = debug_span!("flush_movies", count = resolved.len());
        let _enter = span.enter();

        for movie in resolved {
            let ResolvedMovie {
                lookup,
                duration,
                videos,
            } = movie;

            let metadata_id = match lookup {
                MetadataLookupWithIds::New {
                    metadata,
                    external_ids,
                } => {
                    let poster = metadata.poster.clone();
                    let backdrop = metadata.backdrop.clone();
                    let metadata_id = tx.insert_metadata(&metadata.into_db_metadata()).await?;
                    let movie_id = tx
                        .insert_movie(&metadata.into_db_movie(metadata_id, duration))
                        .await?;
                    if let Some(cast) = metadata.cast {
                        insert_roles(tx, metadata_id, cast, asset_tasks).await?;
                    }
                    for ext_id in &external_ids {
                        let _ = tx
                            .insert_external_id(DbExternalId {
                                external_provider: ext_id.provider,
                                external_id: ext_id.id.clone(),
                                metadata_id: Some(metadata_id),
                                is_prime: false.into(),
                                ..Default::default()
                            })
                            .await;
                    }
                    for genre in metadata.genres.into_iter().flatten() {
                        let _ = tx.insert_content_genre(metadata_id, genre.into()).await;
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
                    metadata_id
                }
                MetadataLookupWithIds::Local(movie_id) => {
                    sqlx::query!("SELECT metadata_id FROM movies WHERE id = ?", movie_id)
                        .fetch_one(&mut **tx)
                        .await?
                        .metadata_id
                }
            };

            for video in &videos {
                tx.update_video_metadata_id(video.source.id, metadata_id)
                    .await?;
            }
        }

        Ok(())
    }
}

async fn fetch_single_movie_chunk(
    db: Db,
    config: ScanConfig,
    title: String,
    videos: &[LibraryItem<MovieIdentifier>],
    movie_providers: &[&'static (dyn MovieMetadataProvider + Send + Sync)],
    progress: &MetadataProgressEmitter,
) -> ResolvedMovie {
    let first = videos.first().expect("movies are chunked");
    let db_movies = db.search_movie(&title).await.unwrap_or_default();

    if db_movies.is_empty()
        || db_movies.first().unwrap().title.split_whitespace().count()
            != title.split_whitespace().count()
    {
        for provider in movie_providers {
            let Ok(search_results) = provider.movie_search(&title, config.fetch_params).await
            else {
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
                    progress.dispatch_success(videos.len());
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
                    let Ok(mut movie_metadata) = provider
                        .movie(&first_result.metadata_id, config.fetch_params)
                        .await
                    else {
                        continue;
                    };
                    let mut external_ids = Vec::new();
                    movie_metadata
                        .external_ids
                        .as_mut()
                        .map(|v| external_ids.append(v));
                    external_ids.insert(
                        0,
                        ExternalIdMetadata {
                            provider: first_result.metadata_provider,
                            id: first_result.metadata_id.clone(),
                        },
                    );
                    progress.dispatch_success(videos.len());
                    return ResolvedMovie {
                        lookup: MetadataLookupWithIds::New {
                            metadata: movie_metadata,
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
        let fallback = movie_fallback(&title);
        progress.dispatch_fail(
            FailedContent {
                title: title,
                videos: videos
                    .iter()
                    .map(|v| v.source.video.path().to_path_buf())
                    .collect(),
                content_type: ContentType::Movie,
            },
            videos.len(),
        );
        ResolvedMovie {
            lookup: fallback,
            duration,
            videos: videos.to_vec(),
        }
    } else {
        let local_id = db_movies.first().unwrap().metadata_id.parse().unwrap();
        progress.dispatch_success(videos.len());
        ResolvedMovie {
            lookup: MetadataLookupWithIds::Local(local_id),
            duration: Duration::ZERO,
            videos: videos.to_vec(),
        }
    }
}
