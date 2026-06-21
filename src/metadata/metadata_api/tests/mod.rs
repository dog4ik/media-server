//! Tests for the metadata_api resolve/write/reconcile pipeline.

use sqlx::SqlitePool;

use crate::{
    db::{Db, DbActions, DbHistory},
    library::Source,
    metadata::metadata_api::asset_saver::AssetTasks,
};

use super::{
    MetadataLookup,
    reconcile::reconcile_show_tree,
    show::{ShowItem, ShowMetadataApi},
};

use std::assert_matches;

pub mod provider_mock;

/// Minimal [`ShowItem`] standing in for a library video. `fallback_source` is
/// `None`, modelling an episode marked watched outside the library.
#[derive(Clone)]
pub struct TestItem {
    pub season: usize,
    pub episode: usize,
}

impl ShowItem for TestItem {
    fn season(&self) -> usize {
        self.season
    }
    fn episode(&self) -> usize {
        self.episode
    }
    fn fallback_source(&self) -> Option<Source> {
        None
    }
}

pub fn leak_db(pool: SqlitePool) -> &'static Db {
    Box::leak(Box::new(Db { pool }))
}

mod search_show {
    use super::provider_mock::{MockProvider, ShowTreeBuilder};
    use super::*;

    #[sqlx::test]
    async fn search_show_by_id_external(pool: SqlitePool) -> anyhow::Result<()> {
        let db = leak_db(pool);
        // The provider knows the show, but it is absent from the local db -> New.
        let show_tree = ShowTreeBuilder::new(1).season(1, 1..=1);
        let expected = show_tree.show_key().show_metadata();
        let api = ShowMetadataApi::new(MockProvider::new([show_tree], []), db);

        let show = api.search_show_by_id(&expected.metadata_id).await?;
        let MetadataLookup::New { metadata } = show else {
            panic!("metadata must be new");
        };
        assert_eq!(metadata.metadata_id, expected.metadata_id);
        assert_eq!(metadata.seasons, Some(vec![1]));
        assert_eq!(metadata.metadata_provider, expected.metadata_provider);
        Ok(())
    }

    #[sqlx::test]
    async fn search_show_by_id_internal(pool: SqlitePool) -> anyhow::Result<()> {
        let db = leak_db(pool);
        // Seed the show locally and let the provider serve the very same show.
        let builder = ShowTreeBuilder::new(1).season(1, 1..=1);
        let saved = builder.save(db).await?;
        let api = ShowMetadataApi::new(MockProvider::new([builder], []), db);

        let show = api
            .search_show_by_id(&saved.key.show_metadata().metadata_id)
            .await?;
        let MetadataLookup::Local(local_id) = show else {
            panic!("metadata must be local");
        };
        assert_eq!(local_id.metadata_id, saved.metadata_id);
        assert_eq!(local_id.id, saved.content_id);
        Ok(())
    }
}

#[sqlx::test]
async fn reconcile_updates_in_place(pool: SqlitePool) -> anyhow::Result<()> {
    use provider_mock::{MockProvider, ShowTreeBuilder};

    let db = leak_db(pool);

    // Seed the existing local show: one season with episodes s1e1 and s1e2.
    let saved = ShowTreeBuilder::new(1).season(1, 1..=2).save(db).await?;
    let s1e1 = *saved.episode(1, 1).expect("s1e1 was seeded");
    let s1e2 = *saved.episode(1, 2).expect("s1e2 was seeded");

    // Watch history on s1e1, to prove the FK survives the in-place metadata update.
    db.insert_history(DbHistory {
        id: None,
        time: 42,
        is_finished: true,
        update_time: Some(time::OffsetDateTime::now_utc().into()),
        metadata_id: s1e1.metadata_id,
    })
    .await?;

    // The corrected metadata is a *different* show served by the provider: it keeps
    // s1e1 (matches the existing tree), adds s1e3 and omits the existing s1e2.
    let corrected = ShowTreeBuilder::new(2).season(1, [1, 3]);
    let corrected_show = corrected.show_key();
    let corrected_meta = corrected_show.show_metadata();
    let corrected_s1e1 = corrected_show
        .season_key(1)
        .episode_key(1)
        .external_metadata();
    let api = ShowMetadataApi::new(MockProvider::new([corrected], []), db);

    // The corrected show is not in the local db -> all New nodes.
    let show = api.search_show_by_id(&corrected_meta.metadata_id).await?;
    assert_matches!(show, MetadataLookup::New { .. });
    let fresh = api
        .fetch_show_tree(
            show,
            vec![
                TestItem {
                    season: 1,
                    episode: 1,
                },
                TestItem {
                    season: 1,
                    episode: 3,
                },
            ],
        )
        .await?;

    let mut tx = db.begin().await?;
    let written =
        reconcile_show_tree(db, &mut tx, &mut AssetTasks::new(), saved.content_id, fresh).await?;
    tx.commit().await?;

    // The show metadata row is reused (ids unchanged) and updated in place.
    assert_eq!(written.show_id, saved.content_id);
    assert_eq!(written.metadata_id, saved.metadata_id);
    let show_title: String =
        sqlx::query_scalar!("SELECT title FROM metadata WHERE id = ?", saved.metadata_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(show_title, corrected_meta.title);
    let show_count: i64 = sqlx::query_scalar!("SELECT COUNT(*) FROM shows")
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(show_count, 1);

    // Show external ids recreated: the old show's id deleted, the corrected one inserted.
    let show_ext: Vec<String> = sqlx::query_scalar!(
        "SELECT external_id FROM external_ids WHERE metadata_id = ?",
        saved.metadata_id
    )
    .fetch_all(&db.pool)
    .await?;
    assert_eq!(show_ext, vec![corrected_meta.metadata_id.clone()]);

    // Episode 1 metadata reused, updated, and its external id recreated.
    let ep1_title: String =
        sqlx::query_scalar!("SELECT title FROM metadata WHERE id = ?", s1e1.metadata_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(ep1_title, corrected_s1e1.title);
    let ep1_ext: Vec<String> = sqlx::query_scalar!(
        "SELECT external_id FROM external_ids WHERE metadata_id = ?",
        s1e1.metadata_id
    )
    .fetch_all(&db.pool)
    .await?;
    assert_eq!(ep1_ext, vec![corrected_s1e1.metadata_id.clone()]);

    // The existing episode 2 (absent from the fresh tree) is kept untouched.
    let ep2_title: String =
        sqlx::query_scalar!("SELECT title FROM metadata WHERE id = ?", s1e2.metadata_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(ep2_title, s1e2.key.external_metadata().title);

    // The new s1e3 is inserted -> three episodes total.
    let episode_count: i64 = sqlx::query_scalar!("SELECT COUNT(*) FROM episodes")
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(episode_count, 3);
    let new_episode_count: i64 =
        sqlx::query_scalar!("SELECT COUNT(*) FROM episodes WHERE number = 3")
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(new_episode_count, 1);

    // The history row (a FK onto episode 1's metadata) survived the in-place update.
    let history_count: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM history WHERE metadata_id = ?",
        s1e1.metadata_id
    )
    .fetch_one(&db.pool)
    .await?;
    assert_eq!(history_count, 1);

    Ok(())
}
