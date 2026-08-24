pub mod asset_saver;
pub mod batch;
pub mod movie;
pub mod reconcile;
pub mod show;

#[cfg(test)]
pub mod tests;

use crate::{
    api::api_data::local_show::{Episode, Season, Show},
    db::{Db, DbActions, DbTransaction, LocalContentId},
    metadata::MetadataProvider,
};

use self::asset_saver::AssetTasks;

#[derive(Debug, Clone)]
pub enum MetadataLookup<T> {
    New {
        metadata: T,
    },
    Local(LocalContentId),
    /// Provider returned no metadata
    Missing,
}

trait LocalLookupScope {
    type Show: LocalMetadataIdentifier;
    type Season: LocalMetadataIdentifier;
    type Episode: LocalMetadataIdentifier;
    async fn show_lookup(
        db: &Db,
        provider: MetadataProvider,
        metadata_id: &str,
    ) -> sqlx::Result<Option<Self::Show>>;
    async fn seasons_lookup(
        db: &Db,
        show_id: i64,
        seasons_scope: Vec<usize>,
    ) -> sqlx::Result<Vec<(usize, Self::Season)>>;
    async fn episodes_lookup(
        db: &Db,
        show_id: i64,
        episodes_scope: Vec<(usize, usize)>,
    ) -> sqlx::Result<Vec<(usize, usize, Self::Episode)>>;
}

/// Trait that is implemented by all local objects connected to the metadata
/// e.g. show, season, episode, movie.
trait LocalMetadataIdentifier {
    fn local_id(&self) -> LocalContentId;
}

impl LocalMetadataIdentifier for LocalContentId {
    fn local_id(&self) -> LocalContentId {
        *self
    }
}

impl LocalMetadataIdentifier for Show {
    fn local_id(&self) -> LocalContentId {
        let local = self
            .local
            .as_ref()
            .expect("show object must be local for the valid usage of this trait");
        LocalContentId {
            id: local.id,
            metadata_id: local.metadata_id,
        }
    }
}

impl LocalMetadataIdentifier for Episode {
    fn local_id(&self) -> LocalContentId {
        let local = self
            .local
            .as_ref()
            .expect("episode object must be local for the valid usage of this trait");
        LocalContentId {
            id: local.id,
            metadata_id: local.metadata_id,
        }
    }
}

impl LocalMetadataIdentifier for Season {
    fn local_id(&self) -> LocalContentId {
        let local = self
            .local
            .as_ref()
            .expect("season object must be local for the valid usage of this trait");
        LocalContentId {
            id: local.id,
            metadata_id: local.metadata_id,
        }
    }
}

impl LocalLookupScope for LocalContentId {
    type Show = Self;
    type Season = Self;
    type Episode = Self;

    async fn show_lookup(
        db: &Db,
        provider: MetadataProvider,
        metadata_id: &str,
    ) -> sqlx::Result<Option<Self::Show>> {
        db.crossreference_show(provider, metadata_id).await
    }

    async fn seasons_lookup(
        db: &Db,
        show_id: i64,
        seasons_scope: Vec<usize>,
    ) -> sqlx::Result<Vec<(usize, Self::Season)>> {
        let seasons = db.get_show_season_nodes(show_id).await?;
        Ok(seasons
            .into_iter()
            .map(|s| {
                (
                    s.number as usize,
                    LocalContentId {
                        id: s.id,
                        metadata_id: s.metadata_id,
                    },
                )
            })
            .collect())
    }

    async fn episodes_lookup(
        db: &Db,
        show_id: i64,
        episodes_scope: Vec<(usize, usize)>,
    ) -> sqlx::Result<Vec<(usize, usize, Self::Episode)>> {
        let episodes = db.get_show_episode_nodes(show_id).await?;
        Ok(episodes
            .into_iter()
            .map(|s| {
                (
                    s.season_number as usize,
                    s.number as usize,
                    LocalContentId {
                        id: s.id,
                        metadata_id: s.metadata_id,
                    },
                )
            })
            .collect())
    }
}

pub struct PendingInsert<T> {
    pub content: T,
    pub tx: DbTransaction,
    pub assets: AssetTasks,
}

impl<T> PendingInsert<T> {
    /// Commit the transaction and ensure assets are saved.
    ///
    /// When the transaction fails to commit assets are not being saved
    pub async fn commit(self, max_concurrency: usize) -> sqlx::Result<()> {
        self.tx.commit().await?;
        self.assets.save(max_concurrency, ()).await;
        Ok(())
    }

    /// Map inner content to another type
    pub fn map<F, R>(self, map_fn: F) -> PendingInsert<R>
    where
        F: FnOnce(T) -> R,
    {
        let content = self.content;
        PendingInsert {
            content: map_fn(content),
            tx: self.tx,
            assets: self.assets,
        }
    }
}
