use crate::{
    db::{DbActions, DbRole, DbTransaction},
    ffmpeg,
    library::{
        LibraryItem, Media, Source,
        assets::{BackdropAsset, FileAsset, PosterAsset, PosterContentType},
    },
    metadata::{ExternalIdMetadata, FetchParams, PersonMetadata},
    scan::scan_progress::MetadataProgressEmitter,
};

pub mod episode;
pub mod fallback;
mod merge;
pub mod movie;
pub mod reconcile;
pub mod scan_progress;
pub mod show;

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
        asset_tasks: &mut Vec<AssetSaveTask>,
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
#[derive(Clone)]
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
            max_show_concurrency: 4,
            use_season_episodes: false,
            max_movie_concurrency: 8,
            max_asset_concurrency: 16,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AssetKind {
    Poster(PosterAsset),
    Backdrop(BackdropAsset),
}

pub enum AssetTaskSource {
    Url(String),
    VideoFrame(Source),
    UrlWithFrameFallback { url: String, source: Source },
}

pub struct AssetSaveTask {
    pub kind: AssetKind,
    pub source: AssetTaskSource,
}

impl AssetSaveTask {
    pub async fn execute(self) -> anyhow::Result<()> {
        match self.kind {
            AssetKind::Poster(asset) => self.source.execute_with(asset).await,
            AssetKind::Backdrop(asset) => self.source.execute_with(asset).await,
        }
    }
}

impl AssetTaskSource {
    async fn execute_with(self, asset: impl FileAsset) -> anyhow::Result<()> {
        match self {
            AssetTaskSource::Url(url) => save_asset_from_url(url.parse()?, asset).await,
            AssetTaskSource::VideoFrame(source) => save_asset_from_frame(asset, &source).await,
            AssetTaskSource::UrlWithFrameFallback { url, source } => {
                save_asset_from_url_with_frame_fallback(url.parse()?, asset, &source).await
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

#[tracing::instrument(level = "debug", skip(asset), fields(asset = %asset.path().display()))]
async fn save_asset_from_url(url: reqwest::Url, asset: impl FileAsset) -> anyhow::Result<()> {
    use std::io::{Error, ErrorKind};
    use tokio_stream::StreamExt;
    use tokio_util::io::StreamReader;

    let response = reqwest::get(url).await?;
    let stream = response
        .bytes_stream()
        .map(|data| data.map_err(|e| Error::new(ErrorKind::Other, e)));
    let mut stream_reader = StreamReader::new(stream);
    asset.save_from_reader(&mut stream_reader).await?;
    Ok(())
}

#[tracing::instrument(level = "debug", skip(asset, source), fields(asset = %asset.path().display()))]
async fn save_asset_from_url_with_frame_fallback(
    url: reqwest::Url,
    asset: impl FileAsset,
    source: &Source,
) -> anyhow::Result<()> {
    use tokio::fs;
    let asset_path = asset.path();
    if let Err(e) = save_asset_from_url(url, asset).await {
        let video_duration = source.video.metadata().await?.duration();
        tracing::warn!("Failed to save image, pulling frame: {e}");
        fs::create_dir_all(asset_path.parent().unwrap()).await?;
        ffmpeg::pull_frame(source.video.path(), asset_path, video_duration / 2).await?;
    }
    Ok(())
}

pub(super) async fn insert_roles(
    tx: &mut DbTransaction,
    metadata_id: i64,
    cast: Vec<PersonMetadata>,
    asset_tasks: &mut Vec<AssetSaveTask>,
) -> sqlx::Result<()> {
    for cast in cast {
        let actor_id = match tx
            .lookup_actor_id(
                cast.metadata_provider,
                &cast.metadata_id,
                cast.imdb_id.as_deref(),
            )
            .await?
        {
            Some(id) => id,
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
            character: cast.role.as_ref().map(|r| r.character.clone()),
        })
        .await?;
    }
    Ok(())
}
