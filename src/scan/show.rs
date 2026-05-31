//! Show scan pipeline
//!
//! Stages:
//! 1. `fetch_show_chunks` — parallel show metadata lookups
//! 2. `merge_show_chunks` — merge chunks that resolved to the same show
//! 3. `resolve_seasons_episodes` — parallel season/episode fetches (no DB writes)
//! 4. `flush_to_db` — sequential DB inserts, one transaction per show
//! 5. `save_assets` — parallel asset downloads/frame extracts

use std::sync::Arc;

use tokio::{sync::Semaphore, task::JoinSet};
use tracing::{Instrument, debug_span};

use crate::{
    db::{Db, DbActions, DbExternalId, DbTransaction},
    library::{
        LibraryItem,
        assets::{BackdropAsset, BackdropContentType, PosterAsset, PosterContentType},
        show::ShowIdentifier,
    },
    metadata::{
        ContentType, ExternalIdMetadata, MetadataProvider, ShowMetadata, ShowMetadataProvider,
        metadata_stack::MetadataProvidersStack,
    },
    scan::{ContentScanner, insert_roles, scan_progress::MetadataProgressEmitter},
};

use super::{
    AssetKind, AssetSaveTask, AssetTaskSource, MetadataLookup, MetadataLookupWithIds, ScanConfig,
    episode::{EpisodeScanner, ResolvedEpisode, ResolvedSeason, ResolvedShow, ShowProvider},
    fallback::show_fallback,
    merge::try_merge_chunks,
};

struct ShowChunk {
    lookup: MetadataLookupWithIds<ShowMetadata>,
    videos: Vec<LibraryItem<ShowIdentifier>>,
}

pub struct ShowScanner {
    db: Db,
    providers: &'static MetadataProvidersStack,
    config: ScanConfig,
}

impl ShowScanner {
    pub fn new(db: Db, providers: &'static MetadataProvidersStack, config: ScanConfig) -> Self {
        Self {
            db,
            providers,
            config,
        }
    }

    async fn fetch_show_chunks(
        &self,
        mut videos: Vec<LibraryItem<ShowIdentifier>>,
    ) -> Vec<ShowChunk> {
        videos.sort_unstable_by(|a, b| {
            a.identifier
                .title
                .to_lowercase()
                .cmp(&b.identifier.title.to_lowercase())
        });

        let mut show_providers = self.providers.show_providers();
        show_providers.retain(|p| p.provider_identifier() != MetadataProvider::Local);
        let show_providers: Arc<[&'static (dyn ShowMetadataProvider + Send + Sync)]> =
            Arc::from(show_providers.into_boxed_slice());

        let semaphore = Arc::new(Semaphore::new(self.config.max_show_concurrency));
        let mut handles: JoinSet<ShowChunk> = JoinSet::new();

        for title_videos in videos
            .chunk_by(|a, b| a.identifier.title.eq_ignore_ascii_case(&b.identifier.title))
            .map(Vec::from)
        {
            let title = title_videos.first().unwrap().identifier.title.clone();
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let db = self.db.clone();
            let config = self.config.clone();
            let show_providers = show_providers.clone();
            let span = debug_span!("scan_show", title = %title);
            handles.spawn(
                async move {
                    let _permit = permit;
                    let lookup = fetch_single_show_chunk(db, config, &title, &show_providers).await;
                    ShowChunk {
                        lookup,
                        videos: title_videos,
                    }
                }
                .instrument(span),
            );
        }

        handles.join_all().await
    }

    fn merge_show_chunks(&self, chunks: Vec<ShowChunk>) -> Vec<ShowChunk> {
        let (statuses, mut items): (Vec<_>, Vec<_>) =
            chunks.into_iter().map(|c| (c.lookup, c.videos)).unzip();

        try_merge_chunks(&statuses, &mut items);

        statuses
            .into_iter()
            .zip(items)
            .filter(|(_, videos)| !videos.is_empty())
            .map(|(lookup, videos)| ShowChunk { lookup, videos })
            .collect()
    }

    async fn resolve_seasons_episodes(
        &self,
        chunks: Vec<ShowChunk>,
        progress: MetadataProgressEmitter,
    ) -> Vec<ResolvedShow> {
        let semaphore = Arc::new(Semaphore::new(self.config.max_show_concurrency));
        let show_providers = self.providers.show_providers();
        let mut handles: JoinSet<ResolvedShow> = JoinSet::new();

        for chunk in chunks {
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            let db = self.db.clone();
            let config = self.config.clone();
            let progress = progress.clone();

            let external_ids: Vec<ExternalIdMetadata> = match &chunk.lookup {
                MetadataLookupWithIds::New { external_ids, .. } => external_ids.clone(),
                MetadataLookupWithIds::Local(show_id) => db
                    .get_external_ids(*show_id, ContentType::Show)
                    .await
                    .unwrap_or_default(),
            };

            let providers: Arc<[ShowProvider]> = show_providers
                .iter()
                .filter_map(|p| {
                    let identifier = p.provider_identifier();
                    if identifier == MetadataProvider::Local {
                        return None;
                    }
                    external_ids
                        .iter()
                        .find(|id| id.provider == identifier)
                        .map(|ext_id| ShowProvider {
                            provider: *p,
                            id: ext_id.id.clone(),
                        })
                })
                .collect();

            let ShowChunk {
                lookup: show_lookup,
                videos,
            } = chunk;
            let span = debug_span!("scan_show_details");
            handles.spawn(
                async move {
                    let _permit = permit;
                    EpisodeScanner::new(db, providers, config, progress)
                        .resolve_show(show_lookup, videos)
                        .await
                }
                .instrument(span),
            );
        }

        handles.join_all().await
    }
}

impl ContentScanner for ShowScanner {
    type Identifier = ShowIdentifier;
    type Resolved = ResolvedShow;

    async fn resolve(
        &self,
        videos: Vec<LibraryItem<ShowIdentifier>>,
        progress: MetadataProgressEmitter,
    ) -> Vec<ResolvedShow> {
        let chunks = self.fetch_show_chunks(videos).await;
        tracing::debug!(total_chunks = %chunks.len(), "Resolved show chunks");
        let merged = self.merge_show_chunks(chunks);
        tracing::debug!(total_chunks = %merged.len(), "Merged show chunks");
        self.resolve_seasons_episodes(merged, progress).await
    }

    async fn flush_to_db(
        &self,
        tx: &mut DbTransaction,
        asset_tasks: &mut Vec<AssetSaveTask>,
        resolved_shows: Vec<ResolvedShow>,
    ) -> sqlx::Result<()> {
        let span = debug_span!("flush_shows", count = resolved_shows.len());
        let _enter = span.enter();

        for resolved in resolved_shows {
            let show_id = match resolved.show_lookup {
                MetadataLookupWithIds::New {
                    metadata,
                    external_ids,
                } => {
                    let poster = metadata.poster.clone();
                    let backdrop = metadata.backdrop.clone();
                    let metadata_id = tx.insert_metadata(&metadata.into_db_metadata()).await?;
                    let show_id = tx.insert_show(&metadata.into_db_show(metadata_id)).await?;
                    if let Some(cast) = metadata.cast {
                        insert_roles(tx, metadata_id, cast, asset_tasks).await?;
                    }
                    for ext_id in &external_ids {
                        if let Err(e) = tx
                            .insert_external_id(DbExternalId {
                                external_provider: ext_id.provider,
                                external_id: ext_id.id.clone(),
                                metadata_id: Some(metadata_id),
                                ..Default::default()
                            })
                            .await
                        {
                            tracing::error!(
                                provider = %ext_id.provider,
                                "Failed to insert external id: {e}"
                            );
                        }
                    }
                    for genre in metadata.genres.into_iter().flatten() {
                        let _ = tx.insert_content_genre(metadata_id, genre.into()).await;
                    }
                    if let Some(url) = poster {
                        asset_tasks.push(AssetSaveTask {
                            kind: AssetKind::Poster(PosterAsset::new(
                                show_id,
                                PosterContentType::Show,
                            )),
                            source: AssetTaskSource::Url(url),
                        });
                    }
                    if let Some(url) = backdrop {
                        asset_tasks.push(AssetSaveTask {
                            kind: AssetKind::Backdrop(BackdropAsset::new(
                                show_id,
                                BackdropContentType::Show,
                            )),
                            source: AssetTaskSource::Url(url),
                        });
                    }
                    show_id
                }
                MetadataLookupWithIds::Local(show_id) => show_id,
            };

            for resolved_season in resolved.seasons {
                let ResolvedSeason { lookup, episodes } = resolved_season;

                let season_id = match lookup {
                    MetadataLookup::New { metadata } => {
                        let poster = metadata.poster.clone();
                        let metadata_id = tx.insert_metadata(&metadata.into_db_metadata()).await?;
                        let season_id = tx
                            .insert_season(metadata.into_db_season(metadata_id, show_id))
                            .await?;
                        if let Some(cast) = metadata.cast {
                            insert_roles(tx, metadata_id, cast, asset_tasks).await?;
                        }
                        if let Some(url) = poster {
                            asset_tasks.push(AssetSaveTask {
                                kind: AssetKind::Poster(PosterAsset::new(
                                    season_id,
                                    PosterContentType::Season,
                                )),
                                source: AssetTaskSource::Url(url.into()),
                            });
                        }
                        season_id
                    }
                    MetadataLookup::Local(season_id) => season_id,
                };

                for resolved_episode in episodes {
                    let ResolvedEpisode {
                        lookup,
                        duration,
                        videos,
                        ..
                    } = resolved_episode;

                    let metadata_id = match lookup {
                        MetadataLookup::New { metadata } => {
                            let poster = metadata.poster.clone();
                            let ext_provider = metadata.metadata_provider;
                            let ext_id = metadata.metadata_id.clone();
                            let metadata_id =
                                tx.insert_metadata(&metadata.into_db_metadata()).await?;
                            if metadata.metadata_provider != MetadataProvider::Local {
                                tx.insert_external_id(DbExternalId {
                                    external_provider: ext_provider,
                                    external_id: ext_id,
                                    metadata_id: Some(metadata_id),
                                    ..Default::default()
                                })
                                .await?;
                            }
                            let episode_id = tx
                                .insert_episode(&metadata.into_db_episode(
                                    metadata_id,
                                    season_id,
                                    duration,
                                ))
                                .await?;
                            if let Some(cast) = metadata.cast {
                                insert_roles(tx, metadata_id, cast, asset_tasks).await?;
                            }
                            let first_source = videos.first().map(|v| v.source.clone());
                            if let Some(url) = poster {
                                let task_source = match first_source {
                                    Some(source) => AssetTaskSource::UrlWithFrameFallback {
                                        url: url.into(),
                                        source,
                                    },
                                    None => AssetTaskSource::Url(url.into()),
                                };
                                asset_tasks.push(AssetSaveTask {
                                    kind: AssetKind::Poster(PosterAsset::new(
                                        episode_id,
                                        PosterContentType::Episode,
                                    )),
                                    source: task_source,
                                });
                            } else if let Some(source) = first_source {
                                asset_tasks.push(AssetSaveTask {
                                    kind: AssetKind::Poster(PosterAsset::new(
                                        episode_id,
                                        PosterContentType::Episode,
                                    )),
                                    source: AssetTaskSource::VideoFrame(source),
                                });
                            }
                            metadata_id
                        }
                        MetadataLookup::Local(episode_id) => {
                            sqlx::query!(
                                "SELECT metadata_id FROM episodes WHERE id = ?",
                                episode_id
                            )
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
            }
        }

        Ok(())
    }
}

async fn fetch_single_show_chunk(
    db: Db,
    config: ScanConfig,
    title: &str,
    show_providers: &[&'static (dyn ShowMetadataProvider + Send + Sync)],
) -> MetadataLookupWithIds<ShowMetadata> {
    let shows = db.search_show(title).await.unwrap_or_default();

    if shows.is_empty()
        || shows.first().unwrap().title.split_whitespace().count()
            != title.split_whitespace().count()
    {
        for provider in show_providers {
            let Ok(search_results) = provider.show_search(title, config.fetch_params).await else {
                continue;
            };
            let Some(first_result) = search_results.into_iter().next() else {
                continue;
            };
            match db
                .crossreference_show(first_result.metadata_provider, &first_result.metadata_id)
                .await
            {
                Ok(Some(local_id)) => return MetadataLookupWithIds::Local(local_id),
                Ok(None) | Err(_) => {
                    let Ok(mut show_metadata) = provider
                        .show(&first_result.metadata_id, config.fetch_params)
                        .await
                    else {
                        continue;
                    };
                    let mut external_ids = Vec::new();
                    show_metadata
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
                    return MetadataLookupWithIds::New {
                        external_ids,
                        metadata: show_metadata,
                    };
                }
            }
        }
        tracing::warn!("Using show metadata fallback for: {title}");
        show_fallback(title)
    } else {
        let local_id = shows
            .into_iter()
            .next()
            .unwrap()
            .metadata_id
            .parse()
            .unwrap();
        MetadataLookupWithIds::Local(local_id)
    }
}
