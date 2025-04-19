use crate::{
    ffmpeg,
    library::{Source, assets::FileAsset},
    metadata::ExternalIdMetadata,
};

mod merge;
pub mod movie;
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
