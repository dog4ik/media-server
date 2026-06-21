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
