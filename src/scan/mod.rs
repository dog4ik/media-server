use crate::{
    config,
    db::DbTransaction,
    library::{LibraryItem, Media},
    metadata::{
        ExternalIdMetadata, FetchParams,
        metadata_api::{
            asset_saver::AssetTasks,
            merge::{MergeKey, Mergeable},
        },
    },
    progress::{ProgressStatus, TaskProgress, TaskTrait},
    scan::scan_progress::MetadataProgressEmitter,
};

pub mod episode;
pub mod fallback;
pub mod movie;
pub mod reconcile;
pub mod scan_progress;
pub mod show;

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct LibraryScanTask {
    scan_config: ScanConfig,
    /// Content without proper metadata
    failed_content: Vec<scan_progress::FailedContent>,
}

impl LibraryScanTask {
    pub fn new(scan_config: ScanConfig) -> Self {
        Self {
            scan_config,
            failed_content: Vec::new(),
        }
    }
}

impl PartialEq for LibraryScanTask {
    fn eq(&self, _other: &Self) -> bool {
        // All scan tasks are even (no duplicates are allowed)
        true
    }
}

impl Eq for LibraryScanTask {}

impl TaskTrait for LibraryScanTask {
    type Progress = scan_progress::ProgressChunk;

    fn into_progress(status: ProgressStatus<Self>) -> TaskProgress {
        TaskProgress::LibraryScan(status)
    }
}

/// Common interface for content scanners (shows, movies): fetch metadata for a batch of
/// library videos, then flush the resolved tree into the database.
// Used only with static dispatch within this crate; auto-trait bounds on the returned
// futures are inferred at the call sites, so the `async fn` desugaring is fine here.
#[allow(async_fn_in_trait)]
pub trait ContentScanner {
    type Identifier: Media;
    type Resolved;

    /// Resolve metadata for the given videos. Reports per-video progress through `progress`,
    /// counting fallbacks as failures.
    async fn resolve(
        &self,
        videos: Vec<LibraryItem<Self::Identifier>>,
        progress: MetadataProgressEmitter,
    ) -> Vec<Self::Resolved>;

    /// Flush resolved metadata to the database, queueing asset downloads into `asset_tasks`.
    async fn flush_to_db(
        &self,
        tx: &mut DbTransaction,
        asset_tasks: &mut AssetTasks,
        resolved: Vec<Self::Resolved>,
    ) -> sqlx::Result<()>;
}

#[derive(Debug, Clone)]
pub enum MetadataLookup<T> {
    New { metadata: T },
    Local(i64),
}

#[derive(Debug, Clone)]
pub enum MetadataLookupWithIds<T> {
    New {
        metadata: T,
        external_ids: Vec<ExternalIdMetadata>,
    },
    Local(i64),
}

/// Configuration for scan operations.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ScanConfig {
    pub fetch_params: FetchParams,
    /// Try to use season's episodes list to resolve episodes metadata
    /// It will speed up metadata fetch for newly added season, but episodes will end up with partially incomplete metadata
    pub use_season_episodes: bool,
    pub max_show_concurrency: usize,
    pub max_movie_concurrency: usize,
    pub max_asset_concurrency: usize,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            fetch_params: FetchParams::default(),
            max_show_concurrency: config::scan::MaxShowConcurrency::default().0,
            use_season_episodes: config::scan::UseSeasonEpisodes::default().0,
            max_movie_concurrency: config::scan::MaxMovieConcurrency::default().0,
            max_asset_concurrency: config::scan::MaxAssetConcurrency::default().0,
        }
    }
}

impl ScanConfig {
    pub fn new_from_server_configuration() -> Self {
        let (
            config::MetadataLanguage(lang),
            config::scan::MaxShowConcurrency(max_show_concurrency),
            config::scan::UseSeasonEpisodes(use_season_episodes),
            config::scan::MaxMovieConcurrency(max_movie_concurrency),
            config::scan::MaxAssetConcurrency(max_asset_concurrency),
        ) = config::CONFIG.get_values();
        Self {
            fetch_params: FetchParams { lang },
            max_show_concurrency,
            use_season_episodes,
            max_movie_concurrency,
            max_asset_concurrency,
        }
    }
}

impl<M> Mergeable for MetadataLookupWithIds<M> {
    fn merge_key(&self) -> MergeKey<'_> {
        match self {
            MetadataLookupWithIds::New { external_ids, .. } => MergeKey::External(external_ids),
            MetadataLookupWithIds::Local(local_id) => MergeKey::Local(*local_id),
        }
    }
}
