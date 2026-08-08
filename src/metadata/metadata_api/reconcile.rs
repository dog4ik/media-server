//! Writes a freshly resolved show tree *over* an existing local one.
//!
//! Used by the metadata fix / reset / refresh flows. Unlike [`flush_show_tree`],
//! nodes that already exist locally are matched by season/episode **number** and
//! their metadata rows are updated *in place* — the content/metadata ids are
//! reused, so every foreign key pointing at them (history, intros, saved list,
//! watch progress) survives. External ids are recreated (old deleted, fresh
//! inserted). Local nodes that are absent from the fresh tree are left untouched.
//!
//! The caller supplies an already-resolved fresh tree (resolved against the
//! corrected provider id, so every node is [`MetadataLookup::New`]). This keeps
//! reconcile a pure database-write step.
//!
//! [`flush_show_tree`]: super::show::ShowMetadataApi::flush_show_tree

use anyhow::bail;

use crate::{
    db::{Db, DbActions, DbExternalId, DbTransaction},
    library::assets::{BackdropAsset, BackdropContentType, PosterAsset, PosterContentType},
    metadata::{ExternalIdMetadata, metadata_api::asset_saver::AssetTasks},
    scan::{AssetKind, AssetSaveTask, AssetTaskSource},
};

use super::{
    MetadataLookup,
    show::{
        HasSource, LocalTree, ResolvedEpisode, ResolvedSeason, ResolvedShow, WrittenEpisode,
        WrittenSeason, WrittenShow, queue_episode_poster,
    },
};

/// Reconciles `fresh` onto the existing local show `old_show_id`, returning the
/// written tree with the reused/new ids. Cast/roles are intentionally not
/// re-applied here to avoid duplicating role rows on every refresh.
pub async fn reconcile_show_tree<T>(
    db: &Db,
    tx: &mut DbTransaction,
    asset_tasks: &mut AssetTasks,
    old_show_id: i64,
    fresh: ResolvedShow<T>,
) -> anyhow::Result<WrittenShow<T>>
where
    T: HasSource,
{
    let old_tree = LocalTree::load(db, old_show_id).await?;
    let show_metadata_id =
        sqlx::query_scalar!("SELECT metadata_id FROM shows WHERE id = ?", old_show_id)
            .fetch_one(&mut **tx)
            .await?;

    let MetadataLookup::New {
        metadata: show_meta,
    } = fresh.show_lookup
    else {
        bail!("reconcile requires a freshly resolved show");
    };

    // Show: update the metadata row in place and recreate its external ids.
    let poster = show_meta.poster.clone();
    let backdrop = show_meta.backdrop.clone();
    tx.update_metadata(show_metadata_id, &show_meta.into_db_metadata())
        .await?;
    tx.update_show_backdrop(old_show_id, backdrop.clone())
        .await?;
    recreate_external_ids(tx, show_metadata_id, show_meta.external_ids).await;
    for genre in show_meta.genres.into_iter().flatten() {
        // INSERT OR IGNORE: safe to re-run on refresh.
        let _ = tx
            .insert_content_genre(show_metadata_id, genre.into())
            .await;
    }
    if let Some(url) = poster {
        asset_tasks.push(AssetSaveTask {
            kind: AssetKind::Poster(PosterAsset::new(old_show_id, PosterContentType::Show)),
            source: AssetTaskSource::Url(url),
        });
    }
    if let Some(url) = backdrop {
        asset_tasks.push(AssetSaveTask {
            kind: AssetKind::Backdrop(BackdropAsset::new(old_show_id, BackdropContentType::Show)),
            source: AssetTaskSource::Url(url),
        });
    }

    let mut written_seasons = Vec::new();
    for season in fresh.seasons {
        let ResolvedSeason {
            number: season_number,
            lookup,
            episodes,
        } = season;

        let MetadataLookup::New {
            metadata: season_meta,
        } = lookup
        else {
            tracing::warn!(
                season = season_number,
                "Skipping non-fresh season in reconcile"
            );
            continue;
        };

        let (season_id, season_metadata_id) = match old_tree.seasons.get(&season_number) {
            Some(local) => {
                let poster = season_meta.poster.clone();
                tx.update_metadata(local.metadata_id, &season_meta.into_db_metadata())
                    .await?;
                queue_simple_poster(asset_tasks, local.id, PosterContentType::Season, poster);
                (local.id, local.metadata_id)
            }
            None => {
                let poster = season_meta.poster.clone();
                let metadata_id = tx.insert_metadata(&season_meta.into_db_metadata()).await?;
                let season_id = tx
                    .insert_season(season_meta.into_db_season(metadata_id, old_show_id))
                    .await?;
                queue_simple_poster(asset_tasks, season_id, PosterContentType::Season, poster);
                (season_id, metadata_id)
            }
        };

        let mut written_episodes = Vec::new();
        for episode in episodes {
            let ResolvedEpisode {
                number: episode_number,
                lookup,
                duration,
                items,
            } = episode;

            let MetadataLookup::New { metadata: ep_meta } = lookup else {
                tracing::warn!(
                    season = season_number,
                    episode = episode_number,
                    "Skipping non-fresh episode in reconcile"
                );
                continue;
            };

            let poster = ep_meta.poster.clone();
            let source = items.first().and_then(|i| i.fallback_source());

            let (episode_id, episode_metadata_id) = match old_tree
                .episodes
                .get(&(season_number, episode_number))
            {
                Some(local) => {
                    tx.update_metadata(local.metadata_id, &ep_meta.into_db_metadata())
                        .await?;
                    tx.delete_external_ids(local.metadata_id).await?;
                    if !ep_meta.metadata_provider.is_local() {
                        tx.insert_external_id(DbExternalId {
                            external_provider: ep_meta.metadata_provider,
                            external_id: ep_meta.metadata_id.clone(),
                            metadata_id: Some(local.metadata_id),
                            ..Default::default()
                        })
                        .await?;
                    }
                    queue_episode_poster(asset_tasks, local.id, poster, source);
                    (local.id, local.metadata_id)
                }
                None => {
                    let ext_provider = ep_meta.metadata_provider;
                    let ext_id = ep_meta.metadata_id.clone();
                    let metadata_id = tx.insert_metadata(&ep_meta.into_db_metadata()).await?;
                    if !ext_provider.is_local() {
                        tx.insert_external_id(DbExternalId {
                            external_provider: ext_provider,
                            external_id: ext_id,
                            metadata_id: Some(metadata_id),
                            ..Default::default()
                        })
                        .await?;
                    }
                    let episode_id = tx
                        .insert_episode(&ep_meta.into_db_episode(metadata_id, season_id, duration))
                        .await?;
                    queue_episode_poster(asset_tasks, episode_id, poster, source);
                    (episode_id, metadata_id)
                }
            };

            written_episodes.push(WrittenEpisode {
                episode_id,
                metadata_id: episode_metadata_id,
                number: episode_number,
                items,
            });
        }

        written_seasons.push(WrittenSeason {
            season_id,
            metadata_id: season_metadata_id,
            number: season_number,
            episodes: written_episodes,
        });
    }

    Ok(WrittenShow {
        show_id: old_show_id,
        metadata_id: show_metadata_id,
        seasons: written_seasons,
    })
}

/// Deletes every external id of a metadata row, then inserts the fresh set.
async fn recreate_external_ids(
    tx: &mut DbTransaction,
    metadata_id: i64,
    external_ids: Option<Vec<ExternalIdMetadata>>,
) {
    if let Err(e) = tx.delete_external_ids(metadata_id).await {
        tracing::error!("Failed to delete external ids: {e}");
        return;
    }
    for ext in external_ids.into_iter().flatten() {
        if let Err(e) = tx
            .insert_external_id(DbExternalId {
                external_provider: ext.provider,
                external_id: ext.id,
                metadata_id: Some(metadata_id),
                ..Default::default()
            })
            .await
        {
            tracing::error!(provider = %ext.provider, "Failed to insert external id: {e}");
        }
    }
}

fn queue_simple_poster(
    asset_tasks: &mut AssetTasks,
    content_id: i64,
    content_type: PosterContentType,
    poster: Option<String>,
) {
    if let Some(url) = poster {
        asset_tasks.push(AssetSaveTask {
            kind: AssetKind::Poster(PosterAsset::new(content_id, content_type)),
            source: AssetTaskSource::Url(url),
        });
    }
}
