use crate::{
    db::{DbActions, DbRole, DbTransaction},
    ffmpeg,
    library::{
        Source,
        assets::{BackdropAsset, FileAsset, PosterAsset, PosterContentType},
    },
    metadata::{ExternalIdMetadata, FetchParams, PersonMetadata},
};

pub mod episode;
pub mod fallback;
mod merge;
pub mod movie;
pub mod scan_progress;
pub mod show;

#[derive(Debug, Clone)]
enum MetadataLookup<T> {
    New { metadata: T },
    Local(i64),
}

#[derive(Debug, Clone)]
enum MetadataLookupWithIds<T> {
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
    pub max_show_concurrency: usize,
    pub max_movie_concurrency: usize,
    pub max_asset_concurrency: usize,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            fetch_params: FetchParams::default(),
            max_show_concurrency: 4,
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

async fn save_asset_from_frame(asset: impl FileAsset, source: &Source) -> anyhow::Result<()> {
    use tokio::fs;
    let asset_path = asset.path();
    let video_duration = source.video.metadata().await?.duration();
    fs::create_dir_all(asset_path.parent().unwrap()).await?;
    ffmpeg::pull_frame(source.video.path(), asset_path, video_duration / 2).await?;
    Ok(())
}

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
    content_id: i64,
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
            content_id,
            character: cast.role.as_ref().map(|r| r.character.clone()),
        })
        .await?;
    }
    Ok(())
}
