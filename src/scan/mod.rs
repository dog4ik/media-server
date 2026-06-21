use std::collections::HashMap;

use crate::{
    config,
    db::{DbActions, DbQueryBuilder, DbRole, DbTransaction},
    ffmpeg,
    library::{
        LibraryItem, Media, Source,
        assets::{BackdropAsset, FileAsset, PosterAsset, PosterContentType},
    },
    metadata::{
        ExternalIdMetadata, FetchParams, MetadataProvider, PersonMetadata,
        metadata_api::asset_saver::AssetTasks,
    },
    progress::{ProgressStatus, TaskProgress, TaskTrait},
    scan::scan_progress::MetadataProgressEmitter,
};

pub mod episode;
pub mod fallback;
mod merge;
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

#[derive(Debug, Clone)]
pub enum AssetKind {
    Poster(PosterAsset),
    Backdrop(BackdropAsset),
}

#[derive(Debug)]
pub enum AssetTaskSource {
    Url(String),
    VideoFrame(Source),
    UrlWithFrameFallback { url: String, source: Source },
}

#[derive(Debug)]
pub struct AssetSaveTask {
    pub kind: AssetKind,
    pub source: AssetTaskSource,
}

impl AssetSaveTask {
    pub async fn execute(self, http_client: &reqwest::Client) -> anyhow::Result<()> {
        match self.kind {
            AssetKind::Poster(asset) => self.source.execute_with(http_client, asset).await,
            AssetKind::Backdrop(asset) => self.source.execute_with(http_client, asset).await,
        }
    }
}

impl AssetTaskSource {
    async fn execute_with(
        self,
        http_client: &reqwest::Client,
        asset: impl FileAsset,
    ) -> anyhow::Result<()> {
        match self {
            AssetTaskSource::Url(url) => {
                save_asset_from_url(http_client, url.parse()?, asset).await
            }
            AssetTaskSource::VideoFrame(source) => save_asset_from_frame(asset, &source).await,
            AssetTaskSource::UrlWithFrameFallback { url, source } => {
                save_asset_from_url_with_frame_fallback(http_client, url.parse()?, asset, &source)
                    .await
            }
        }
    }
}

#[tracing::instrument(level = "debug", skip_all, fields(asset = %asset.path().display()))]
async fn save_asset_from_frame(asset: impl FileAsset, source: &Source) -> anyhow::Result<()> {
    use tokio::fs;
    let asset_path = asset.path();
    let video_duration = source.video.metadata().await?.duration();
    fs::create_dir_all(asset_path.parent().unwrap()).await?;
    ffmpeg::pull_frame(source.video.path(), asset_path, video_duration / 2).await?;
    Ok(())
}

#[tracing::instrument(level = "debug", skip(http_client, asset), fields(asset = %asset.path().display()))]
async fn save_asset_from_url(
    http_client: &reqwest::Client,
    url: reqwest::Url,
    asset: impl FileAsset,
) -> anyhow::Result<()> {
    use std::io::{Error, ErrorKind};
    use tokio_stream::StreamExt;
    use tokio_util::io::StreamReader;

    let response = http_client.get(url).send().await?;
    let stream = response
        .bytes_stream()
        .map(|data| data.map_err(|e| Error::new(ErrorKind::Other, e)));
    let mut stream_reader = StreamReader::new(stream);
    asset.save_from_reader(&mut stream_reader).await?;
    Ok(())
}

#[tracing::instrument(level = "debug", skip(http_client, asset, source), fields(asset = %asset.path().display()))]
async fn save_asset_from_url_with_frame_fallback(
    http_client: &reqwest::Client,
    url: reqwest::Url,
    asset: impl FileAsset,
    source: &Source,
) -> anyhow::Result<()> {
    use tokio::fs;
    let asset_path = asset.path();
    if let Err(e) = save_asset_from_url(http_client, url, asset).await {
        let video_duration = source.video.metadata().await?.duration();
        tracing::warn!("Failed to save image, pulling frame: {e}");
        fs::create_dir_all(asset_path.parent().unwrap()).await?;
        ffmpeg::pull_frame(source.video.path(), asset_path, video_duration / 2).await?;
    }
    Ok(())
}

pub(crate) async fn insert_roles(
    tx: &mut DbTransaction,
    metadata_id: i64,
    cast: Vec<PersonMetadata>,
    asset_tasks: &mut AssetTasks,
) -> sqlx::Result<()> {
    if cast.is_empty() {
        return Ok(());
    }

    #[derive(sqlx::FromRow)]
    struct ActorQueryRow {
        id: i64,
        external_metadata_id: String,
        external_metadata_provider: MetadataProvider,
    }

    #[derive(Debug, Hash, Eq, PartialEq)]
    struct MapKey<'a> {
        provider: MetadataProvider,
        provider_id: &'a str,
    }

    let local_actors = DbQueryBuilder::new(
        "select id, external_metadata_id, external_metadata_provider from actors where (external_metadata_id, external_metadata_provider) in ",
    )
    .push_tuples(
        cast.iter(),
        |mut b,
         PersonMetadata {
             metadata_id,
             metadata_provider,
             ..
         }| {
            b.push_bind(metadata_id).push_bind(metadata_provider);
        },
    )
    .build_query_as::<ActorQueryRow>()
    .fetch_all(&mut **tx)
    .await?;

    let local_actors_map: HashMap<_, _> = local_actors
        .iter()
        .map(|v| {
            (
                MapKey {
                    provider: v.external_metadata_provider,
                    provider_id: &v.external_metadata_id,
                },
                v.id,
            )
        })
        .collect();

    for cast in cast {
        let actor_id = match local_actors_map.get(&MapKey {
            provider: cast.metadata_provider,
            provider_id: &cast.metadata_id,
        }) {
            Some(id) => *id,
            None => {
                let actor_id = tx.insert_actor(&cast.into_db_actor()).await?;
                if let Some(poster_url) = cast.person_poster {
                    asset_tasks.push(AssetSaveTask {
                        kind: AssetKind::Poster(PosterAsset::new(
                            actor_id,
                            PosterContentType::Actor,
                        )),
                        source: AssetTaskSource::Url(poster_url),
                    });
                }
                actor_id
            }
        };

        tx.insert_role(&DbRole {
            id: None,
            actor_id,
            metadata_id,
            character: cast.role.map(|r| r.character),
        })
        .await?;
    }
    Ok(())
}
