//! Movie scan
//!
//! Algorithm should look like:
//! 1. Chunk all movies by their local name.
//! 2. Fetch all movies and external ids from providers.
//! 3. Merge all movies that have same metadata ids.
//! 4. Save merged movies and their assets locally.

use std::time::Duration;

use tokio::task::JoinSet;

use crate::{
    app_state::AppError,
    db::{Db, DbActions, DbExternalId, DbMovie, DbTransaction},
    ffmpeg,
    library::{
        LibraryItem,
        assets::{BackdropAsset, BackdropContentType, FileAsset, PosterAsset, PosterContentType},
        movie::MovieIdentifier,
    },
    metadata::{
        ContentType, DiscoverMetadataProvider, ExternalIdMetadata, FetchParams, MovieMetadata,
        metadata_stack::MetadataProvidersStack,
    },
};

use super::{
    MetadataLookupWithIds, merge::try_merge_chunks, save_asset_from_url,
    save_asset_from_url_with_frame_fallback,
};

pub async fn scan_movies(
    fetch_params: FetchParams,
    db: Db,
    providers: &MetadataProvidersStack,
    mut new_files: Vec<LibraryItem<MovieIdentifier>>,
) -> anyhow::Result<()> {
    new_files.sort_unstable_by(|a, b| {
        a.identifier
            .title
            .to_lowercase()
            .cmp(&b.identifier.title.to_lowercase())
    });

    let discover_providers = providers.discover_providers();
    let mut movie_scan_handles = JoinSet::new();
    for movie_files in new_files
        .chunk_by(|a, b| a.identifier.title.eq_ignore_ascii_case(&b.identifier.title))
        .map(Vec::from)
    {
        let discover_providers = discover_providers.clone();
        let db = db.clone();
        movie_scan_handles.spawn(async move {
            (
                fetch_movie(&movie_files, &db, fetch_params, discover_providers).await,
                movie_files,
            )
        });
    }

    // bad
    let (mut statuses, mut items) = movie_scan_handles
        .join_all()
        .await
        .into_iter()
        .inspect(|res| {
            if let Err(e) = &res.0 {
                tracing::error!("Movie scan task failed: {e}");
            }
        })
        .filter_map(|v| Some((v.0.ok()?, v.1)))
        .fold((Vec::new(), Vec::new()), |mut acc, n| {
            acc.0.push(n.0);
            acc.1.push(n.1);
            acc
        });

    debug_assert_eq!(statuses.len(), items.len());
    try_merge_chunks(&statuses, &mut items);
    debug_assert_eq!(statuses.len(), items.len());

    {
        let mut idx = 0;
        statuses.retain(|_| {
            let keep = !items[idx].is_empty();
            idx += 1;
            keep
        });
    }
    items.retain(|chunk| !chunk.is_empty());
    debug_assert_eq!(statuses.len(), items.len());

    let mut assets_save_tracker = JoinSet::new();
    let mut tx = db.begin().await?;
    for (lookup, items) in statuses.into_iter().zip(items) {
        let local_id = match lookup {
            MetadataLookupWithIds::New {
                metadata,
                external_ids,
            } => {
                handle_movie_metadata(
                    &mut tx,
                    metadata,
                    &items,
                    external_ids,
                    &mut assets_save_tracker,
                )
                .await?
            }
            MetadataLookupWithIds::Local(local_id) => local_id,
        };
        for item in items {
            tx.update_video_movie_id(item.source.id, local_id).await?;
        }
    }
    tx.commit().await?;
    tracing::trace!("waiting for all assets to save");
    assets_save_tracker.join_all().await;
    Ok(())
}

async fn fetch_movie(
    items: &[LibraryItem<MovieIdentifier>],
    db: &Db,
    fetch_params: FetchParams,
    providers: Vec<&(dyn DiscoverMetadataProvider + Send + Sync)>,
) -> Result<MetadataLookupWithIds<MovieMetadata>, AppError> {
    let item = items.first().expect("movies are chunked");
    let db_movies = db.search_movie(&item.identifier.title).await?;
    let duration = item.source.video.fetch_duration().await?;
    if db_movies.is_empty()
        || db_movies.first().unwrap().title.split_whitespace().count()
            != item.identifier.title.split_whitespace().count()
    {
        for provider in providers {
            if let Ok(search_result) = provider
                .movie_search(&item.identifier.title, fetch_params)
                .await
            {
                let Some(first_result) = search_result.into_iter().next() else {
                    continue;
                };
                match crossreference_movie(db, &first_result).await {
                    Ok(Some(local_id)) => {
                        tracing::debug!(movie_title = first_result.title, "Using local movie ref");
                        return Ok(MetadataLookupWithIds::Local(local_id));
                    }
                    Ok(None) | Err(_) => {
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
                        return Ok(MetadataLookupWithIds::New {
                            metadata: first_result,
                            external_ids,
                        });
                    }
                };
            }
        }
        let id = movie_metadata_fallback(db, item, duration).await?;
        return Ok(MetadataLookupWithIds::Local(id));
    };
    Ok(MetadataLookupWithIds::Local(
        db_movies.first().unwrap().metadata_id.parse().unwrap(),
    ))
}

async fn handle_movie_metadata(
    tx: &mut DbTransaction,
    metadata: MovieMetadata,
    movie_files: &[LibraryItem<MovieIdentifier>],
    external_ids: Vec<ExternalIdMetadata>,
    task_tracker: &mut JoinSet<()>,
) -> anyhow::Result<i64> {
    let first_movie = movie_files[0].source.clone();
    let poster_url = metadata.poster.clone();
    let backdrop_url = metadata.backdrop.clone();
    let duration = first_movie
        .video
        .fetch_duration()
        .await
        .ok()
        .or(metadata.runtime)
        .unwrap_or_default();
    let db_movie = metadata.into_db_movie(duration);
    let local_id = tx.insert_movie(db_movie).await.unwrap();
    for external_id in external_ids {
        let db_external_id = DbExternalId {
            metadata_provider: external_id.provider.to_string(),
            metadata_id: external_id.id,
            movie_id: Some(local_id),
            is_prime: false.into(),
            ..Default::default()
        };
        let _ = tx.insert_external_id(db_external_id).await;
    }

    task_tracker.spawn(async move {
        let poster_job = poster_url.map(|url| {
            let poster_asset = PosterAsset::new(local_id, PosterContentType::Movie);
            save_asset_from_url_with_frame_fallback(url.into(), poster_asset, &first_movie)
        });
        let backdrop_job = backdrop_url.map(|url| {
            let backdrop_asset = BackdropAsset::new(local_id, BackdropContentType::Movie);
            save_asset_from_url(url.into(), backdrop_asset)
        });
        match (poster_job, backdrop_job) {
            (Some(poster_job), Some(backdrop_job)) => {
                let _ = tokio::join!(poster_job, backdrop_job);
            }
            (Some(poster_job), None) => {
                let _ = poster_job.await;
            }
            (None, Some(backdrop_job)) => {
                let _ = backdrop_job.await;
            }
            _ => {}
        }
    });

    Ok(local_id)
}

/// external to local movie id
async fn crossreference_movie(
    db: &Db,
    external_metadata: &MovieMetadata,
) -> anyhow::Result<Option<i64>> {
    let provider = external_metadata.metadata_provider.to_string();
    let movie_id = sqlx::query!(
        r#"SELECT movie_id as "movie_id!" FROM external_ids WHERE movie_id NOT NULL AND metadata_provider = ? AND metadata_id = ?"#,
        provider,
        external_metadata.metadata_id,
    )
    .fetch_optional(&db.pool)
    .await?
    .map(|r| r.movie_id);
    Ok(movie_id)
}

async fn movie_metadata_fallback(
    db: &Db,
    file: &LibraryItem<MovieIdentifier>,
    duration: Duration,
) -> anyhow::Result<i64> {
    let mut chars = file.identifier.title.chars();
    let first_letter = chars.next().and_then(|c| c.to_uppercase().next());
    let title = first_letter.into_iter().chain(chars).collect();
    let fallback_movie = DbMovie {
        id: None,
        poster: None,
        backdrop: None,
        plot: None,
        release_date: None,
        duration: duration.as_secs() as i64,
        title,
    };
    let mut tx = db.begin().await?;
    let id = tx.insert_movie(fallback_movie).await?;

    let poster_asset = PosterAsset::new(id, PosterContentType::Movie);
    tokio::fs::create_dir_all(poster_asset.path().parent().unwrap()).await?;
    if let Err(e) =
        ffmpeg::pull_frame(file.source.video.path(), poster_asset.path(), duration / 2).await
    {
        tracing::warn!("Failed to create video thumbnail: {e}");
    };
    Ok(id)
}
