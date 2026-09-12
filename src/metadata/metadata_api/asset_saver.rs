use std::sync::Arc;

use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    ffmpeg,
    library::assets::{BackdropAsset, FileAsset, PosterAsset},
    metadata::metadata_api::LocalVideo,
};

#[derive(Debug, Clone)]
pub enum AssetKind {
    Poster(PosterAsset),
    Backdrop(BackdropAsset),
}

#[derive(Debug)]
pub enum AssetTaskSource {
    Url(String),
    VideoFrame(LocalVideo),
    UrlWithFrameFallback { url: String, video: LocalVideo },
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
            AssetTaskSource::VideoFrame(video) => save_asset_from_frame(asset, &video).await,
            AssetTaskSource::UrlWithFrameFallback { url, video } => {
                save_asset_from_url_with_frame_fallback(http_client, url.parse()?, asset, &video)
                    .await
            }
        }
    }
}

#[tracing::instrument(level = "debug", skip_all, fields(asset = %asset.path().display()))]
async fn save_asset_from_frame(asset: impl FileAsset, video: &LocalVideo) -> anyhow::Result<()> {
    use tokio::fs;
    let asset_path = asset.path();
    fs::create_dir_all(asset_path.parent().unwrap()).await?;
    ffmpeg::pull_frame(&video.path, asset_path, video.duration / 2).await?;
    Ok(())
}

#[tracing::instrument(level = "debug", skip(http_client, asset), fields(asset = %asset.path().display()))]
async fn save_asset_from_url(
    http_client: &reqwest::Client,
    url: reqwest::Url,
    asset: impl FileAsset,
) -> anyhow::Result<()> {
    use std::io::Error;
    use tokio_stream::StreamExt;
    use tokio_util::io::StreamReader;

    let response = http_client.get(url).send().await?;
    let stream = response
        .bytes_stream()
        .map(|data| data.map_err(Error::other));
    let mut stream_reader = StreamReader::new(stream);
    asset.save_from_reader(&mut stream_reader).await?;
    Ok(())
}

#[tracing::instrument(level = "debug", skip(http_client, asset), fields(asset = %asset.path().display()))]
async fn save_asset_from_url_with_frame_fallback(
    http_client: &reqwest::Client,
    url: reqwest::Url,
    asset: impl FileAsset,
    video: &LocalVideo,
) -> anyhow::Result<()> {
    use tokio::fs;
    let asset_path = asset.path();
    if let Err(e) = save_asset_from_url(http_client, url, asset).await {
        tracing::warn!("Failed to save image, pulling frame: {e}");
        if let Some(parent) = video.path.parent() {
            fs::create_dir_all(parent).await?;
        }
        ffmpeg::pull_frame(&video.path, asset_path, video.duration / 2).await?;
    }
    Ok(())
}

#[derive(Debug)]
pub struct AssetTasks {
    tasks: Vec<AssetSaveTask>,
    http_client: reqwest::Client,
}

pub trait AssetsProgressSink {
    fn dispatch_success(&self);
    fn dispatch_fail(&self);
}

impl AssetsProgressSink for () {
    fn dispatch_success(&self) {}

    fn dispatch_fail(&self) {}
}

impl AssetTasks {
    pub fn new(http_client: reqwest::Client) -> Self {
        Self {
            tasks: Vec::new(),
            http_client,
        }
    }

    pub fn push(&mut self, task: AssetSaveTask) {
        self.tasks.push(task);
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    #[tracing::instrument(level = "debug", skip_all, fields(max_concurrency))]
    pub async fn save<T>(self, max_concurrency: usize, progress_handler: T)
    where
        T: AssetsProgressSink,
    {
        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        let mut join_set = JoinSet::new();
        for task in self.tasks {
            let semaphore = semaphore.clone();
            let http_client = self.http_client.clone();
            join_set.spawn(async move {
                let _permit = semaphore.acquire_owned().await.unwrap();
                task.execute(&http_client).await
            });
        }
        while let Some(Ok(val)) = join_set.join_next().await {
            match val {
                Ok(_) => {
                    progress_handler.dispatch_success();
                }
                Err(e) => {
                    progress_handler.dispatch_fail();
                    tracing::warn!("Asset save task failed: {e}");
                }
            }
        }
    }
}
