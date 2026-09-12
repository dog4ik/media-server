use std::{path::PathBuf, time::Duration};

use crate::db::{DbTransaction, LocalContentId};

use self::asset_saver::AssetTasks;

pub mod asset_saver;
pub mod batch;
pub mod bucket;
pub mod fallback;
pub mod fix;
pub mod merge;
pub mod movie;
#[allow(unused)]
pub mod reconcile;
pub mod reset;
pub mod roles;
pub mod show;

/// Local object in a content tree
pub mod local_scope;
#[cfg(test)]
pub mod tests;

#[derive(Debug, Clone)]
pub enum MetadataLookup<T, L = LocalContentId> {
    New {
        metadata: T,
    },
    Local(L),
    /// Provider returned no metadata
    Missing,
}

/// Show metadata lookup method
#[derive(Debug, Clone, Copy)]
pub enum ShowLookupMethod<'a> {
    /// Use id to find the show
    Id(&'a str),
    /// Use title to find the show
    Title { title: &'a str, year: Option<u16> },
}

#[derive(Debug, Clone)]
pub struct LocalVideo {
    pub path: PathBuf,
    pub duration: Duration,
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
