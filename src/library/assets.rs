use std::{
    fmt::Display,
    path::{Path, PathBuf},
    time::Duration,
};

use axum::{body::Body, response::IntoResponse};
use axum_extra::{headers::ContentLength, TypedHeader};
use reqwest::StatusCode;
use tokio::{
    fs,
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
};
use tokio_util::io::ReaderStream;

use crate::app_state::AppError;

use super::Video;

pub(crate) trait FileAsset {
    fn path(&self) -> PathBuf;
    async fn save_from_reader(
        &self,
        reader: &mut (impl AsyncRead + std::marker::Unpin),
    ) -> anyhow::Result<()> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(&parent).await?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .await?;
        tokio::io::copy(reader, &mut file).await?;
        Ok(())
    }

    async fn save_bytes(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(&parent).await?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .await?;
        file.write_all(&bytes).await?;
        Ok(())
    }

    async fn read_to_bytes(&self) -> Result<bytes::Bytes, AppError> {
        let path = self.path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(&parent).await?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .open(&path)
            .await?;
        let length = file.metadata().await?.len();
        let mut bytes = Vec::with_capacity(length as usize);
        file.read_to_end(&mut bytes).await?;
        Ok(bytes.into())
    }

    async fn read_to_writer(
        &self,
        writer: &mut (impl AsyncWrite + std::marker::Unpin),
    ) -> anyhow::Result<()> {
        let path = self.path();
        let mut file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .open(&path)
            .await?;
        tokio::io::copy(&mut file, writer).await?;
        Ok(())
    }

    async fn into_response(
        self,
        content_type: axum_extra::headers::ContentType,
        if_modified_since: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
    ) -> Result<impl IntoResponse, std::io::Error>
    where
        Self: Sized,
    {
        let file = self.open().await?;
        let metadata = file.metadata().await?;
        let length_header = TypedHeader(ContentLength(metadata.len()));

        if let (Some(if_modified_since), Ok(metadata_modified)) =
            (if_modified_since, metadata.modified())
        {
            if !if_modified_since.is_modified(metadata_modified) {
                return Ok(StatusCode::NOT_MODIFIED.into_response());
            }
        }

        let crated_header = metadata
            .created()
            .map(|c| TypedHeader(axum_extra::headers::Date::from(c)))
            .ok();
        let modified_header = metadata
            .modified()
            .map(|d| TypedHeader(axum_extra::headers::LastModified::from(d)))
            .ok();

        let cache_control = TypedHeader(
            axum_extra::headers::CacheControl::new()
                .with_public()
                .with_max_age(Duration::from_secs(60 * 60 * 12)),
        );

        let stream = ReaderStream::new(file);
        let body = Body::from_stream(stream);

        Ok((
            TypedHeader(content_type),
            length_header,
            crated_header,
            cache_control,
            modified_header,
            body,
        )
            .into_response())
    }

    async fn open(&self) -> Result<fs::File, std::io::Error> {
        let path = self.path();
        let file = fs::File::open(path).await?;
        Ok(file)
    }

    async fn delete_file(&self) -> std::io::Result<()> {
        fs::remove_file(&self.path()).await
    }
}

pub(crate) trait AssetDir {
    fn path(&self) -> PathBuf;
    async fn delete_dir(&self) -> std::io::Result<()> {
        fs::remove_dir_all(self.path()).await
    }

    /// Ensure that directory is created
    /// Useful when starting ffmpeg job inside asset directory
    async fn prepare_path(&self) -> anyhow::Result<PathBuf> {
        fs::create_dir_all(self.path()).await?;
        Ok(self.path())
    }
}

#[derive(Debug, Clone)]
pub struct PosterAsset {
    path: PathBuf,
}
impl FileAsset for PosterAsset {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
pub enum PosterContentType {
    Show,
    Movie,
    Episode,
    Season,
}
impl Into<AssetContentType> for PosterContentType {
    fn into(self) -> AssetContentType {
        match self {
            PosterContentType::Show => AssetContentType::Show,
            PosterContentType::Movie => AssetContentType::Movie,
            PosterContentType::Episode => AssetContentType::Episode,
            PosterContentType::Season => AssetContentType::Season,
        }
    }
}
impl PosterAsset {
    pub fn new(id: i64, content_type: PosterContentType) -> Self {
        let sharded_path = sharded_path(id, content_type.into());
        PosterAsset {
            path: sharded_path.join("poster.jpg"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackdropAsset {
    path: PathBuf,
}
impl FileAsset for BackdropAsset {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
pub enum BackdropContentType {
    Show,
    Movie,
}
impl Into<AssetContentType> for BackdropContentType {
    fn into(self) -> AssetContentType {
        match self {
            BackdropContentType::Show => AssetContentType::Show,
            BackdropContentType::Movie => AssetContentType::Movie,
        }
    }
}
impl BackdropAsset {
    pub fn new(id: i64, content_type: BackdropContentType) -> Self {
        let sharded_path = sharded_path(id, content_type.into());
        BackdropAsset {
            path: sharded_path.join("backdrop.jpg"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SubtitleAsset {
    id: String,
    path: PathBuf,
}
impl FileAsset for SubtitleAsset {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
impl SubtitleAsset {
    pub fn new(video_id: i64, subtitle_id: String) -> Self {
        let path = video_sharded_path(video_id)
            .join("subs")
            .join(format!("{}.srt", subtitle_id));
        Self {
            path,
            id: subtitle_id,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VariantAsset {
    id: String,
    path: PathBuf,
}
impl FileAsset for VariantAsset {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
impl VariantAsset {
    pub fn new(video_id: i64, variant_id: String) -> Self {
        let path = video_sharded_path(video_id)
            .join("variants")
            .join(format!("{}.mkv", variant_id));
        Self {
            path,
            id: variant_id,
        }
    }

    pub async fn video(&self) -> anyhow::Result<Video> {
        Video::from_path(&self.path).await
    }
}

#[derive(Debug, Clone)]
pub struct PreviewAsset {
    path: PathBuf,
    index: usize,
}
impl FileAsset for PreviewAsset {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
impl PreviewAsset {
    pub fn new(video_id: i64, index: usize) -> Self {
        let path = video_sharded_path(video_id)
            .join("previews")
            .join(format!("{}.jpg", index));
        Self { path, index }
    }
}

// DIRECTORY ASSETS

#[derive(Debug, Clone)]
/// Directory of all video assets
pub struct VideoAssetsDir {
    path: PathBuf,
    id: i64,
}
impl AssetDir for VideoAssetsDir {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
impl VideoAssetsDir {
    pub fn new(video_id: i64) -> Self {
        let path = video_sharded_path(video_id);
        Self { path, id: video_id }
    }
}

#[derive(Debug, Clone)]
pub struct VariantsDirAsset {
    path: PathBuf,
    id: i64,
}
impl AssetDir for VariantsDirAsset {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
impl VariantsDirAsset {
    pub fn new(video_id: i64) -> Self {
        let path = video_sharded_path(video_id).join("variants");
        Self { path, id: video_id }
    }
    pub async fn variants(&self) -> anyhow::Result<Vec<VariantAsset>> {
        let mut dir = fs::read_dir(self.path()).await?;
        let mut output = Vec::new();
        while let Ok(Some(entry)) = dir.next_entry().await {
            let path = entry.path();
            let Some(name) = path.file_stem() else {
                continue;
            };
            output.push(VariantAsset::new(
                self.id,
                name.to_string_lossy().to_string(),
            ))
        }
        Ok(output)
    }
}

#[derive(Debug, Clone)]
pub struct PreviewsDirAsset {
    path: PathBuf,
}
impl AssetDir for PreviewsDirAsset {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
impl PreviewsDirAsset {
    pub fn new(video_id: i64) -> Self {
        let path = video_sharded_path(video_id).join("previews");
        Self { path }
    }
    pub fn previews_count(&self) -> usize {
        std::fs::read_dir(self.path()).map_or(0, |d| d.count())
    }
}

#[derive(Debug, Clone)]
pub struct SubtitlesDirAsset {
    path: PathBuf,
}
impl AssetDir for SubtitlesDirAsset {
    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
impl SubtitlesDirAsset {
    pub fn new(video_id: i64) -> Self {
        let path = video_sharded_path(video_id).join("subtitles");
        Self { path }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum AssetContentType {
    Movie,
    Show,
    Season,
    Episode,
    Video,
}

impl Display for AssetContentType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssetContentType::Movie => write!(f, "movie"),
            AssetContentType::Show => write!(f, "show"),
            AssetContentType::Season => write!(f, "season"),
            AssetContentType::Episode => write!(f, "episode"),
            AssetContentType::Video => write!(f, "video"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ShardedPath(PathBuf);

impl ShardedPath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl Into<PathBuf> for ShardedPath {
    fn into(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for ShardedPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

pub fn video_sharded_path(video_id: i64) -> PathBuf {
    sharded_path(video_id, AssetContentType::Video).into()
}

fn sharded_path(idx: i64, content_type: AssetContentType) -> PathBuf {
    use crate::config::APP_RESOURCES;
    let base_path = &APP_RESOURCES.get().unwrap().resources_path;
    let shard = format!("{:x}", idx % 16);
    let path = PathBuf::from(shard);
    let path = path.join(format!("{}.{:x}", content_type, idx));
    ShardedPath(base_path.join(path)).into()
}
