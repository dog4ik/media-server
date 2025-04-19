//! Show scan
//!
//! Algorithm should look like:
//! 1. Chunk new show episodes by their local name.
//! 2. Concurrently fetch show metadata and external ids from providers.
//! 3. Wait for all shows, merge elements resolved to the same content using external ids.
//! 4. Concurrently fetch and save the rest of seasons and episodes, their assets.

use std::{sync::Arc, time::Duration};

use tokio::task::JoinSet;
use tokio_util::task::TaskTracker;
use tracing::instrument;

use crate::{
    app_state::AppError,
    db::{Db, DbActions, DbEpisode, DbExternalId, DbSeason, DbShow, DbTransaction},
    ffmpeg,
    library::{
        LibraryItem,
        assets::{BackdropAsset, BackdropContentType, FileAsset, PosterAsset, PosterContentType},
        show::ShowIdentifier,
    },
    metadata::{
        ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, FetchParams,
        MetadataProvider, ShowMetadata, ShowMetadataProvider,
        metadata_stack::MetadataProvidersStack,
    },
    scan::{MetadataLookup, merge::try_merge_chunks},
};

use super::{MetadataLookupWithIds, save_asset_from_url, save_asset_from_url_with_frame_fallback};

struct IdWithProvider {
    provider: &'static (dyn ShowMetadataProvider + Send + Sync),
    id: String,
}

impl IdWithProvider {
    pub fn new(id: String, provider: &'static (dyn ShowMetadataProvider + Send + Sync)) -> Self {
        Self { id, provider }
    }
}

pub async fn scan_shows(
    fetch_params: FetchParams,
    db: Db,
    providers: &MetadataProvidersStack,
    mut new_files: Vec<LibraryItem<ShowIdentifier>>,
) -> anyhow::Result<()> {
    new_files.sort_unstable_by(|a, b| {
        a.identifier
            .title
            .to_lowercase()
            .cmp(&b.identifier.title.to_lowercase())
    });

    let mut show_scan_handles: JoinSet<anyhow::Result<ItemsChunk>> = JoinSet::new();

    let mut discover_providers = providers.discover_providers();
    discover_providers.retain(|p| p.provider_identifier() != MetadataProvider::Local);
    let discover_providers = discover_providers.into_boxed_slice();

    tracing::trace!("Started shows fetch");
    for show_episodes in new_files
        .chunk_by(|a, b| a.identifier.title.eq_ignore_ascii_case(&b.identifier.title))
        .map(Vec::from)
    {
        let db = db.clone();
        let discover_providers = discover_providers.clone();
        show_scan_handles.spawn(async move {
            let first_item = show_episodes.first().expect("chunked");
            let relation = fetch_show(db, first_item, &fetch_params, discover_providers).await?;
            Ok(ItemsChunk {
                status: relation,
                items: show_episodes,
            })
        });
    }

    let (mut statuses, mut items_chunks) = show_scan_handles
        .join_all()
        .await
        .into_iter()
        .inspect(|res| {
            if let Err(e) = res {
                tracing::error!("Show scan task failed: {e}");
            }
        })
        .filter_map(Result::ok)
        .fold((Vec::new(), Vec::new()), |mut acc, n| {
            acc.0.push(n.status);
            acc.1.push(n.items);
            acc
        });
    tracing::trace!("Finished shows fetch");

    debug_assert_eq!(statuses.len(), items_chunks.len());
    try_merge_chunks(&statuses, &mut items_chunks);
    debug_assert_eq!(statuses.len(), items_chunks.len());

    {
        let mut idx = 0;
        statuses.retain(|_| {
            let keep = !items_chunks[idx].is_empty();
            idx += 1;
            keep
        });
    }
    items_chunks.retain(|chunk| !chunk.is_empty());
    debug_assert_eq!(statuses.len(), items_chunks.len());

    tracing::debug!("Saving shows");
    let task_tracker = TaskTracker::new();
    let mut local_show_chunks = Vec::new();
    let mut tx = db.begin().await?;
    for (status, chunk) in statuses.into_iter().zip(items_chunks) {
        match status {
            MetadataLookupWithIds::New {
                metadata,
                external_ids,
            } => {
                let external_ids: Arc<[ExternalIdMetadata]> = Arc::from(external_ids);
                let local_id = handle_new_series(
                    metadata,
                    external_ids.clone(),
                    &mut tx,
                    task_tracker.clone(),
                )
                .await?;
                local_show_chunks.push((local_id, external_ids, chunk));
            }
            MetadataLookupWithIds::Local(local_id) => {
                let external_ids = db
                    .get_external_ids(local_id, ContentType::Show)
                    .await
                    .unwrap_or_default();
                let external_ids: Arc<[ExternalIdMetadata]> = Arc::from(external_ids);
                local_show_chunks.push((local_id, external_ids, chunk));
            }
        }
    }
    tx.commit().await?;
    tracing::debug!("Finished saving shows");

    let show_providers = providers.show_providers();

    tracing::debug!("Started handling seasons and episodes");
    for (local_id, external_ids, chunk) in local_show_chunks {
        let db = db.clone();
        let show_providers: Arc<[IdWithProvider]> = show_providers
            .iter()
            .filter_map(|p| {
                let identifier = p.provider_identifier();
                if identifier == MetadataProvider::Local {
                    return None;
                }
                external_ids
                    .iter()
                    .find(|id| id.provider == identifier)
                    .map(|external_id| IdWithProvider::new(external_id.id.clone(), *p))
            })
            .collect();
        handle_seasons_and_episodes(
            &db,
            local_id,
            chunk,
            fetch_params,
            task_tracker.clone(),
            show_providers.clone(),
        )
        .await?;
    }

    tracing::debug!("Finished handling seasons and episodes");

    task_tracker.close();
    tracing::debug!("Waiting for all show asset tasks to finish");
    task_tracker.wait().await;
    Ok(())
}

#[derive(Debug)]
struct ItemsChunk {
    status: MetadataLookupWithIds<ShowMetadata>,
    items: Vec<LibraryItem<ShowIdentifier>>,
}

#[instrument(skip_all)]
async fn fetch_show(
    db: Db,
    item: &LibraryItem<ShowIdentifier>,
    fetch_params: &FetchParams,
    discover_providers: Box<[&(dyn DiscoverMetadataProvider + Send + Sync)]>,
) -> anyhow::Result<MetadataLookupWithIds<ShowMetadata>> {
    // BUG: this will perform search with full text search so for example if we search for Dexter it will
    // find Dexter: New Blood.
    let shows = db.search_show(&item.identifier.title).await?;

    // WARN: This temporary fix will only work if content does not have custom name
    if shows.is_empty()
        || shows.first().unwrap().title.split_whitespace().count()
            != item.identifier.title.split_whitespace().count()
    {
        for provider in discover_providers {
            if let Ok(search_result) = provider
                .show_search(&item.identifier.title, *fetch_params)
                .await
            {
                let Some(first_result) = search_result.into_iter().next() else {
                    continue;
                };
                match crossreference_show(&db, &first_result)
                    .await
                    .inspect_err(|e| tracing::error!("failed to crossreference show: {e}"))
                {
                    Ok(Some(local_id)) => {
                        return Ok(MetadataLookupWithIds::Local(local_id));
                    }
                    Ok(None) | Err(_) => {
                        let Ok(mut external_ids) = provider
                            .external_ids(&first_result.metadata_id, ContentType::Show)
                            .await
                        else {
                            continue;
                        };
                        external_ids.insert(
                            0,
                            ExternalIdMetadata {
                                provider: first_result.metadata_provider,
                                id: first_result.metadata_id.clone(),
                            },
                        );

                        return Ok(MetadataLookupWithIds::New {
                            external_ids,
                            metadata: first_result,
                        });
                    }
                };
            }
        }
        // fallback
        tracing::warn!("Using show metadata fallback");
        let local_id = series_metadata_fallback(&db, item).await?;
        return Ok(MetadataLookupWithIds::Local(local_id));
    }
    let top_search = shows.into_iter().next().expect("shows not empty");
    let local_id = top_search
        .metadata_id
        .parse()
        .expect("local ids are integers");
    Ok(MetadataLookupWithIds::Local(local_id))
}

/// Save series metadata and spawn saving assets.
///
/// Returns local id of the show
///
/// This function will not block.
async fn handle_new_series(
    show: ShowMetadata,
    external_ids: Arc<[ExternalIdMetadata]>,
    tx: &mut DbTransaction,
    assets_save_tracker: TaskTracker,
) -> anyhow::Result<i64> {
    let poster = show.poster.clone();
    let backdrop = show.backdrop.clone();
    let db_show = DbShow::from(show);
    let local_id = tx.insert_show(&db_show).await?;

    for id in external_ids.iter() {
        if let Err(e) = tx
            .insert_external_id(DbExternalId {
                metadata_provider: id.provider.to_string(),
                metadata_id: id.id.clone(),
                show_id: Some(local_id),
                ..Default::default()
            })
            .await
        {
            tracing::error!(provider = %id.provider, "Failed to insert external id: {e}");
        }
    }

    assets_save_tracker.spawn(async move {
        if let Some(url) = poster {
            let poster_asset = PosterAsset::new(local_id, PosterContentType::Show);
            if let Err(e) = save_asset_from_url(url.into(), poster_asset).await {
                tracing::warn!(local_id, "Failed to save show poster: {e}")
            }
        }
        if let Some(url) = backdrop {
            let backdrop_asset = BackdropAsset::new(local_id, BackdropContentType::Show);
            if let Err(e) = save_asset_from_url(url.into(), backdrop_asset).await {
                tracing::warn!(local_id, "Failed to save show backdrop: {e}")
            }
        }
    });

    Ok(local_id)
}

async fn handle_seasons_and_episodes(
    db: &Db,
    local_show_id: i64,
    mut show_episodes: Vec<LibraryItem<ShowIdentifier>>,
    fetch_params: FetchParams,
    assets_save_tracker: TaskTracker,
    show_providers: Arc<[IdWithProvider]>,
) -> anyhow::Result<()> {
    show_episodes.sort_unstable_by_key(|x| x.identifier.season);
    let mut seasons_scan_handles: JoinSet<Result<(), AppError>> = JoinSet::new();
    for mut season_episodes in show_episodes
        .chunk_by(|a, b| a.identifier.season == b.identifier.season)
        .map(Vec::from)
    {
        let show_providers = show_providers.clone();
        let db = db.clone();
        let assets_save_tracker = assets_save_tracker.clone();
        seasons_scan_handles.spawn(async move {
            let season = season_episodes.first().unwrap().clone();
            let local_season_id = fetch_save_season(
                local_show_id,
                season,
                fetch_params,
                &db,
                assets_save_tracker.clone(),
                show_providers.clone(),
            )
            .await?;
            let mut episodes_scan_handles: JoinSet<(
                anyhow::Result<MetadataLookup<EpisodeMetadata>>,
                Vec<LibraryItem<ShowIdentifier>>,
            )> = JoinSet::new();
            tracing::debug!("Season's episodes count: {}", season_episodes.len());
            season_episodes.sort_unstable_by_key(|x| x.identifier.episode);
            for episodes in season_episodes
                .chunk_by(|a, b| a.identifier.episode == b.identifier.episode)
                .map(Vec::from)
            {
                let db = db.clone();
                let show_providers = show_providers.clone();
                episodes_scan_handles.spawn(async move {
                    (
                        fetch_episode(
                            local_show_id,
                            local_season_id,
                            &episodes,
                            &db,
                            fetch_params,
                            show_providers,
                        )
                        .await,
                        episodes,
                    )
                });
            }

            let results = episodes_scan_handles
                .join_all()
                .await
                .into_iter()
                .inspect(|res| {
                    if let Err(e) = &res.0 {
                        tracing::error!("Episode scan task failed: {e}");
                    }
                })
                .filter_map(|r| Some((r.0.ok()?, r.1)));

            let mut tx = db.begin().await?;
            for (result, episodes) in results {
                let id = match result {
                    MetadataLookup::New { metadata } => {
                        let poster = metadata.poster.clone();
                        let first = episodes[0].source.clone();
                        let runtime = metadata.runtime.clone();
                        let db_episode = metadata.into_db_episode(
                            local_season_id,
                            first
                                .video
                                .fetch_duration()
                                .await
                                .ok()
                                .or(runtime)
                                .unwrap_or_default(),
                        );
                        let id = tx.insert_episode(&db_episode).await?;
                        assets_save_tracker.spawn(async move {
                            if let Some(poster) = poster {
                                let asset = PosterAsset::new(id, PosterContentType::Episode);
                                if let Err(e) = save_asset_from_url_with_frame_fallback(
                                    poster.into(),
                                    asset,
                                    &first,
                                )
                                .await
                                {
                                    tracing::error!("Failed to save episode poster: {e}");
                                };
                            }
                        });
                        id
                    }
                    MetadataLookup::Local(id) => id,
                };
                // connect new videos to existing episode
                for video in episodes {
                    tx.update_video_episode_id(video.source.id, id).await?;
                }
            }
            tx.commit().await?;

            Ok(())
        });
    }

    while let Some(result) = seasons_scan_handles.join_next().await {
        match result {
            Ok(Err(e)) => {
                tracing::error!("Season Reconciliation task failed with err {}", e)
            }
            Err(e) => tracing::error!("Season reconciliation task panicked: {e}"),
            Ok(Ok(_)) => tracing::trace!("Joined season reconciliation task"),
        }
    }
    Ok(())
}

#[instrument(skip_all)]
async fn fetch_save_season(
    local_show_id: i64,
    item: LibraryItem<ShowIdentifier>,
    fetch_params: FetchParams,
    db: &Db,
    assets_save_tracker: TaskTracker,
    providers: Arc<[IdWithProvider]>,
) -> anyhow::Result<i64> {
    let season = item.identifier.season as usize;
    let Ok(local_id) = db.get_season_id(local_show_id, season).await else {
        for provider in providers.iter() {
            let Ok(season) = provider
                .provider
                .season(&provider.id, season, fetch_params)
                .await
            else {
                continue;
            };
            let poster = season.poster.clone();
            let id = db
                .insert_season(season.into_db_season(local_show_id))
                .await
                .unwrap();
            assets_save_tracker.spawn(async move {
                if let Some(poster) = poster {
                    let poster_asset = PosterAsset::new(id, PosterContentType::Season);
                    let _ = save_asset_from_url(poster.into(), poster_asset).await;
                }
            });
            return Ok(id);
        }
        // fallback
        tracing::warn!("Using season metadata fallback");
        let id = season_metadata_fallback(db, &item, local_show_id).await?;
        return Ok(id);
    };
    Ok(local_id)
}

#[instrument(skip_all)]
async fn fetch_episode(
    local_show_id: i64,
    local_season_id: i64,
    items: &[LibraryItem<ShowIdentifier>],
    db: &Db,
    fetch_params: FetchParams,
    providers: Arc<[IdWithProvider]>,
) -> anyhow::Result<MetadataLookup<EpisodeMetadata>> {
    let item = items.first().expect("episodes are chunked");
    let season = item.identifier.season as usize;
    let episode = item.identifier.episode as usize;
    if let Ok(local_id) = db.get_episode_id(local_show_id, season, episode).await {
        Ok(MetadataLookup::Local(local_id))
    } else {
        tracing::trace!(
            "Fetching duration for the episode: {}",
            item.source.video.path().display()
        );
        let duration = item.source.video.fetch_duration().await?;
        for provider in providers.iter() {
            let Ok(episode) = provider
                .provider
                .episode(&provider.id, season, episode, fetch_params)
                .await
            else {
                continue;
            };
            return Ok(MetadataLookup::New { metadata: episode });
        }
        // fallback
        tracing::warn!("Using episode metadata fallback");
        let id = episode_metadata_fallback(db, item, local_season_id, duration).await?;
        Ok(MetadataLookup::Local(id))
    }
}

#[instrument(skip_all)]
async fn series_metadata_fallback(
    db: &Db,
    file: &LibraryItem<ShowIdentifier>,
) -> anyhow::Result<i64> {
    let show_fallback = DbShow {
        id: None,
        poster: None,
        backdrop: None,
        plot: None,
        release_date: None,
        title: file.identifier.title.to_string(),
    };
    let video_metadata = file.source.video.metadata().await?;
    let id = db.insert_show(&show_fallback).await.unwrap();
    let poster_asset = PosterAsset::new(id, PosterContentType::Show);
    tokio::fs::create_dir_all(poster_asset.path().parent().unwrap())
        .await
        .unwrap();
    let _ = ffmpeg::pull_frame(
        file.source.video.path(),
        poster_asset.path(),
        video_metadata.duration() / 2,
    )
    .await;
    Ok(id)
}

async fn season_metadata_fallback(
    db: &Db,
    file: &LibraryItem<ShowIdentifier>,
    show_id: i64,
) -> anyhow::Result<i64> {
    let fallback_season = DbSeason {
        number: file.identifier.season.into(),
        show_id,
        id: None,
        release_date: None,
        plot: None,
        poster: None,
    };
    let id = db.insert_season(fallback_season).await?;
    Ok(id)
}

async fn episode_metadata_fallback(
    db: &Db,
    file: &LibraryItem<ShowIdentifier>,
    season_id: i64,
    duration: Duration,
) -> anyhow::Result<i64> {
    let mut tx = db.begin().await?;
    let fallback_episode = DbEpisode {
        release_date: None,
        plot: None,
        poster: None,
        number: file.identifier.episode.into(),
        title: format!("Episode {}", file.identifier.episode),
        id: None,
        duration: duration.as_secs() as i64,
        season_id,
    };
    let video_metadata = file.source.video.metadata().await?;
    let id = tx.insert_episode(&fallback_episode).await?;
    tx.update_video_episode_id(file.source.id, id).await?;
    tx.commit().await?;
    let poster_asset = PosterAsset::new(id, PosterContentType::Episode);
    tokio::fs::create_dir_all(poster_asset.path().parent().unwrap())
        .await
        .unwrap();
    let _ = ffmpeg::pull_frame(
        file.source.video.path(),
        poster_asset.path(),
        video_metadata.duration() / 2,
    )
    .await;
    Ok(id)
}

/// external to local show id
async fn crossreference_show(
    db: &Db,
    external_metadata: &ShowMetadata,
) -> anyhow::Result<Option<i64>> {
    let provider = external_metadata.metadata_provider.to_string();
    let show_id = sqlx::query!(
        r#"SELECT show_id as "show_id!" FROM external_ids WHERE show_id NOT NULL AND metadata_provider = ? AND metadata_id = ?"#,
        provider,
        external_metadata.metadata_id
    )
    .fetch_optional(&db.pool)
    .await?
    .map(|r| r.show_id);
    Ok(show_id)
}
