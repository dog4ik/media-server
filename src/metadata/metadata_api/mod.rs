pub mod asset_saver;
pub mod batch;
pub mod movie;
#[allow(unused)]
pub mod reconcile;
pub mod show;

/// Local object in a content tree
pub mod local_scope;
#[cfg(test)]
pub mod tests;

use crate::db::{DbTransaction, LocalContentId};

use self::asset_saver::AssetTasks;

#[derive(Debug, Clone)]
pub enum MetadataLookup<T, L = LocalContentId> {
    New {
        metadata: T,
    },
    Local(L),
    /// Provider returned no metadata
    Missing,
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
    #[allow(unused)]
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
