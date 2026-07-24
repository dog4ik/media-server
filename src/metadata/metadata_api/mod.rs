pub mod asset_saver;
pub mod movie;
pub mod reconcile;
pub mod show;

#[cfg(test)]
pub mod tests;

use crate::db::{DbTransaction, LocalContentId};

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

pub struct PendingInsert<T> {
    pub content: T,
    pub tx: DbTransaction,
    pub assets: AssetTasks,
}

impl<T> PendingInsert<T> {
    /// Commit the transaction and ensure assets are saved.
    ///
    /// When the transaction fails to commit assets are not being saved
    pub async fn commit(
        self,
        max_concurrency: usize,
        http_client: reqwest::Client,
    ) -> sqlx::Result<()> {
        self.tx.commit().await?;
        self.assets.save(max_concurrency, http_client, ()).await;
        Ok(())
    }
}
