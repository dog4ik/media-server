// Good idea to create 2 structs.
// TempAsset<T: Asset> -> ResourceAsset<T: Asset>
// Temp struct can only be written while Resource struct can be read
// How do i distinguish Dir asset from File asset?

use std::{
    fmt::Display,
    path::{Path, PathBuf},
    time::Duration,
};

use axum::{body::Body, response::IntoResponse};
use axum_extra::{TypedHeader, headers::ContentLength};
use reqwest::StatusCode;
use tokio::{fs, io::AsyncRead};
use tokio_util::io::ReaderStream;

use crate::config;

use super::Video;

pub(crate) trait FileAsset {
    fn relative_path(&self) -> &Path;

    fn temp_path(&self) -> PathBuf {
        use crate::config::APP_RESOURCES;
        let base_path = &APP_RESOURCES.temp_path;
        base_path.join(self.relative_path())
    }

    fn path(&self) -> PathBuf {
        use crate::config::APP_RESOURCES;
        let base_path = &APP_RESOURCES.resources_path;
        base_path.join(self.relative_path())
    }

    async fn save_from_reader(&self, reader: &mut (impl AsyncRead + Unpin)) -> anyhow::Result<()> {
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
        let start_time = &config::APP_RESOURCES.start_time;

        if let (Some(if_modified_since), Ok(metadata_modified)) =
            (if_modified_since, metadata.modified())
        {
            if !if_modified_since.is_modified(*start_time.max(&metadata_modified)) {
                return Ok(StatusCode::NOT_MODIFIED.into_response());
            }
        }

        let crated_header = metadata
            .created()
            .map(|c| TypedHeader(axum_extra::headers::Date::from(c)))
            .ok();
        let modified_header = metadata
            .modified()
            .map(|d| TypedHeader(axum_extra::headers::LastModified::from(d.max(*start_time))))
            .ok();

        let cache_control = TypedHeader(
            axum_extra::headers::CacheControl::new()
                .with_no_cache()
                .with_max_age(Duration::ZERO),
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
    fn relative_path(&self) -> &Path;

    fn temp_path(&self) -> PathBuf {
        use crate::config::APP_RESOURCES;
        let base_path = &APP_RESOURCES.temp_path;
        base_path.join(self.relative_path())
    }

    fn path(&self) -> PathBuf {
        use crate::config::APP_RESOURCES;
        let base_path = &APP_RESOURCES.resources_path;
        base_path.join(self.relative_path())
    }

    async fn delete_dir(&self) -> std::io::Result<()> {
        fs::remove_dir_all(self.path()).await
    }
}

#[derive(Debug, Clone)]
pub struct PosterAsset {
    path: PathBuf,
}
impl FileAsset for PosterAsset {
    fn relative_path(&self) -> &Path {
        &self.path
    }
}
pub enum PosterContentType {
    Show,
    Movie,
    Episode,
    Season,
}
impl From<PosterContentType> for AssetContentType {
    fn from(val: PosterContentType) -> Self {
        match val {
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
pub struct BackdropAsset(PathBuf);
impl FileAsset for BackdropAsset {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
pub enum BackdropContentType {
    Show,
    Movie,
}
impl From<BackdropContentType> for AssetContentType {
    fn from(val: BackdropContentType) -> Self {
        match val {
            BackdropContentType::Show => AssetContentType::Show,
            BackdropContentType::Movie => AssetContentType::Movie,
        }
    }
}
impl BackdropAsset {
    pub fn new(id: i64, content_type: BackdropContentType) -> Self {
        Self(sharded_path(id, content_type.into()).join("backdrop.jpg"))
    }
}

#[derive(Debug, Clone)]
pub struct SubtitleAsset(PathBuf);
impl FileAsset for SubtitleAsset {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl SubtitleAsset {
    pub fn new(video_id: i64, subtitle_id: String) -> Self {
        Self(
            video_sharded_path(video_id)
                .join("subs")
                .join(format!("{}.srt", subtitle_id)),
        )
    }
}

#[derive(Debug, Clone)]
pub struct VariantAsset(PathBuf);
impl FileAsset for VariantAsset {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl VariantAsset {
    pub fn new(video_id: i64, variant_id: String) -> Self {
        Self(
            video_sharded_path(video_id)
                .join("variants")
                .join(format!("{}.mkv", variant_id)),
        )
    }

    pub async fn video(&self) -> anyhow::Result<Video> {
        Video::from_path(self.path()).await
    }
}

#[derive(Debug, Clone)]
pub struct PreviewAsset(PathBuf);
impl FileAsset for PreviewAsset {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl PreviewAsset {
    pub fn new(video_id: i64, index: usize) -> Self {
        Self(
            video_sharded_path(video_id)
                .join("previews")
                .join(format!("{}.jpg", index)),
        )
    }
}

#[derive(Debug, Clone)]
pub struct ChapterThumbnailAsset(PathBuf);
impl FileAsset for ChapterThumbnailAsset {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl ChapterThumbnailAsset {
    pub fn new(video_id: i64, number: usize) -> Self {
        Self(
            video_sharded_path(video_id)
                .join("chapters")
                .join(format!("{}.jpg", number)),
        )
    }
}

// DIRECTORY ASSETS

/// Directory of all video assets
#[derive(Debug, Clone)]
pub struct VideoAssetsDir(PathBuf);
impl AssetDir for VideoAssetsDir {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl VideoAssetsDir {
    pub fn new(video_id: i64) -> Self {
        Self(video_sharded_path(video_id))
    }
}

/// Directory of all episode assets
#[derive(Debug, Clone)]
pub struct EpisodeAssetsDir(PathBuf);
impl AssetDir for EpisodeAssetsDir {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl EpisodeAssetsDir {
    pub fn new(episode_id: i64) -> Self {
        Self(sharded_path(episode_id, AssetContentType::Episode))
    }
}

/// Directory of all season assets
#[derive(Debug, Clone)]
pub struct SeasonAssetsDir(PathBuf);
impl AssetDir for SeasonAssetsDir {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl SeasonAssetsDir {
    pub fn new(season_id: i64) -> Self {
        Self(sharded_path(season_id, AssetContentType::Season))
    }
}

/// Directory of all show assets
#[derive(Debug, Clone)]
pub struct ShowAssetsDir(PathBuf);
impl AssetDir for ShowAssetsDir {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl ShowAssetsDir {
    pub fn new(show_id: i64) -> Self {
        Self(sharded_path(show_id, AssetContentType::Show))
    }
}

/// Directory of all movie assets
#[derive(Debug, Clone)]
pub struct MovieAssetsDir(PathBuf);
impl AssetDir for MovieAssetsDir {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl MovieAssetsDir {
    pub fn new(movie_id: i64) -> Self {
        Self(sharded_path(movie_id, AssetContentType::Movie))
    }
}

#[derive(Debug, Clone)]
pub struct VariantsDirAsset {
    path: PathBuf,
    id: i64,
}
impl AssetDir for VariantsDirAsset {
    fn relative_path(&self) -> &Path {
        &self.path
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
pub struct PreviewsDirAsset(PathBuf);
impl AssetDir for PreviewsDirAsset {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl PreviewsDirAsset {
    pub fn new(video_id: i64) -> Self {
        Self(video_sharded_path(video_id).join("previews"))
    }
    pub fn previews_count(&self) -> usize {
        std::fs::read_dir(self.path()).map_or(0, |d| d.count())
    }
}

#[derive(Debug, Clone)]
pub struct SubtitlesDirAsset(PathBuf);
impl AssetDir for SubtitlesDirAsset {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl SubtitlesDirAsset {
    pub fn new(video_id: i64) -> Self {
        Self(video_sharded_path(video_id).join("subtitles"))
    }
}

#[derive(Debug, Clone)]
pub struct ChapterThumbnailsDirAsset(PathBuf);
impl AssetDir for ChapterThumbnailsDirAsset {
    fn relative_path(&self) -> &Path {
        &self.0
    }
}
impl ChapterThumbnailsDirAsset {
    pub fn new(video_id: i64) -> Self {
        Self(video_sharded_path(video_id).join("chapters"))
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

impl From<ShardedPath> for PathBuf {
    fn from(val: ShardedPath) -> Self {
        val.0
    }
}

impl AsRef<Path> for ShardedPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

fn video_sharded_path(video_id: i64) -> PathBuf {
    sharded_path(video_id, AssetContentType::Video)
}

fn sharded_path(idx: i64, content_type: AssetContentType) -> PathBuf {
    let shard = format!("{:x}", idx % 16);
    let path = PathBuf::from(shard);
    let path = path.join(format!("{}.{:x}", content_type, idx));
    ShardedPath(path).into()
}
