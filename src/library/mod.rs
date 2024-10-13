use std::{
    collections::HashMap,
    convert::Infallible,
    fmt::Display,
    io::SeekFrom,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
};

use anyhow::Context;
use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use axum_extra::{headers::Range, TypedHeader};
use serde::{de::Visitor, ser::SerializeStruct, Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    sync::{OnceCell, Semaphore},
};
use tokio_util::codec::{BytesCodec, FramedRead};

use crate::{
    app_state::AppError,
    db::{Db, DbActions, DbVideo},
    ffmpeg::{get_metadata, FFprobeOutput},
    metadata::ContentType,
    utils,
};

use self::{
    assets::{
        AssetDir, PreviewAsset, PreviewsDirAsset, SubtitleAsset, SubtitlesDirAsset, VideoAssetsDir,
    },
    movie::MovieIdentifier,
};
use self::{
    assets::{VariantAsset, VariantsDirAsset},
    show::ShowIdentifier,
};

pub mod assets;
pub mod movie;
pub mod show;

const SUPPORTED_FILES: [&str; 3] = ["mkv", "webm", "mp4"];

const EXTRAS_FOLDERS: [&str; 11] = [
    "behind the scenes",
    "deleted scenes",
    "interviews",
    "scenes",
    "samples",
    "shorts",
    "featurettes",
    "clips",
    "other",
    "extras",
    "trailers",
];

#[derive(Debug, Clone)]
pub struct Library {
    pub videos: HashMap<i64, LibraryFile>,
}

pub fn is_format_supported(path: &impl AsRef<Path>) -> bool {
    let path = path.as_ref().to_path_buf();
    let is_extra = path
        .components()
        .any(|c| EXTRAS_FOLDERS.contains(&c.as_os_str().to_string_lossy().to_lowercase().as_ref()));
    let supports_extension = path
        .extension()
        .map_or(false, |ex| SUPPORTED_FILES.contains(&ex.to_str().unwrap()));
    !is_extra && supports_extension
}

pub async fn explore_folder(
    folder: impl AsRef<Path>,
    folder_type: ContentType,
    db: &crate::db::Db,
    exclude: &[PathBuf],
) -> Result<HashMap<i64, LibraryFile>, anyhow::Error> {
    let paths = utils::walk_recursive(folder, Some(is_format_supported))?;
    let mut handles = Vec::with_capacity(paths.len());
    let semaphore = Arc::new(Semaphore::new(200));
    for path in paths {
        if exclude.contains(&path) {
            continue;
        }
        let semaphore = semaphore.clone();
        let db = db.clone();
        handles.push((
            path.clone(),
            tokio::spawn(async move {
                let _permit = semaphore.acquire().await;
                let file: Result<LibraryFile, anyhow::Error> = match folder_type {
                    ContentType::Movie => LibraryItem::<MovieIdentifier>::from_path(path, &db)
                        .await
                        .map(Into::into),
                    ContentType::Show => LibraryItem::<ShowIdentifier>::from_path(path, &db)
                        .await
                        .map(Into::into),
                };
                file
            }),
        ));
    }

    let mut result = HashMap::with_capacity(handles.len());

    for (path, handle) in handles {
        let path = format!("{}", path.display());
        match handle.await {
            Ok(Ok(item)) => {
                result.insert(item.source.id, item);
            }
            Ok(Err(e)) => tracing::warn!(path, "One of the metadata collectors errored: {}", e),
            Err(e) => tracing::error!(path, "One of the metadata collectors panicked: {}", e),
        }
    }

    Ok(result)
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContentIdentifier {
    Show(ShowIdentifier),
    Movie(MovieIdentifier),
}

#[derive(Debug, Clone)]
pub struct LibraryFile {
    pub identifier: ContentIdentifier,
    pub source: Source,
}

#[derive(Debug, Clone)]
pub struct LibraryItem<T: Media> {
    pub identifier: T,
    pub source: Source,
}

impl From<LibraryItem<ShowIdentifier>> for LibraryFile {
    fn from(value: LibraryItem<ShowIdentifier>) -> Self {
        Self {
            identifier: ContentIdentifier::Show(value.identifier),
            source: value.source,
        }
    }
}

impl From<LibraryItem<MovieIdentifier>> for LibraryFile {
    fn from(value: LibraryItem<MovieIdentifier>) -> Self {
        Self {
            identifier: ContentIdentifier::Movie(value.identifier),
            source: value.source,
        }
    }
}

impl ContentIdentifier {
    pub fn identify(content_type: ContentType, tokens: &[String]) -> Option<Self> {
        match content_type {
            ContentType::Movie => MovieIdentifier::identify(tokens).map(Into::into),
            ContentType::Show => ShowIdentifier::identify(tokens).map(Into::into),
        }
    }
    pub fn title(&self) -> &str {
        match self {
            ContentIdentifier::Show(i) => &i.title,
            ContentIdentifier::Movie(i) => &i.title,
        }
    }

    pub fn show_identifier(&self) -> Option<ShowIdentifier> {
        match self {
            ContentIdentifier::Show(s) => Some(s.clone()),
            ContentIdentifier::Movie(_) => None,
        }
    }

    pub fn movie_identifier(&self) -> Option<MovieIdentifier> {
        match self {
            ContentIdentifier::Show(_) => None,
            ContentIdentifier::Movie(m) => Some(m.clone()),
        }
    }
}

impl LibraryFile {
    pub fn into_movie(self) -> Option<LibraryItem<MovieIdentifier>> {
        match self.identifier {
            ContentIdentifier::Movie(m) => Some(LibraryItem {
                identifier: m,
                source: self.source,
            }),
            _ => None,
        }
    }

    pub fn into_show(self) -> Option<LibraryItem<ShowIdentifier>> {
        match self.identifier {
            ContentIdentifier::Show(m) => Some(LibraryItem {
                identifier: m,
                source: self.source,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Source {
    pub id: i64,
    pub video: Video,
    pub variants: Vec<Video>,
}

impl Source {
    pub async fn from_video(video: Video, db: &crate::db::Db) -> anyhow::Result<Self> {
        let id = video.get_or_insert_id(db).await?;
        let variants_dir = VariantsDirAsset::new(id);
        let variants = variants_dir.variants().await.unwrap_or_default();
        let mut variants_videos = Vec::with_capacity(variants.len());
        for variant in variants {
            if let Ok(video) = variant.video().await {
                variants_videos.push(video);
            }
        }
        Ok(Self {
            video,
            id,
            variants: variants_videos,
        })
    }
    pub async fn from_path(path: impl AsRef<Path>, db: &crate::db::Db) -> anyhow::Result<Self> {
        let video = Video::from_path(path).await?;
        let id = video.get_or_insert_id(db).await?;
        let variants = VariantsDirAsset::new(id).variants().await?;
        let mut variants_videos = Vec::with_capacity(variants.len());
        for variant in variants {
            if let Ok(video) = variant.video().await {
                variants_videos.push(video);
            }
        }
        Ok(Self {
            video,
            id,
            variants: variants_videos,
        })
    }

    pub async fn delete_all_resources(&self) -> std::io::Result<()> {
        VideoAssetsDir::new(self.id).delete_dir().await
    }

    pub fn previews_dir(&self) -> PreviewsDirAsset {
        PreviewsDirAsset::new(self.id)
    }

    pub fn preview(&self, index: usize) -> PreviewAsset {
        PreviewAsset::new(self.id, index)
    }

    pub fn variants_dir(&self) -> VariantsDirAsset {
        VariantsDirAsset::new(self.id)
    }

    pub fn variant(&self, id: String) -> VariantAsset {
        VariantAsset::new(self.id, id)
    }

    pub fn subtitles_dir(&self) -> SubtitlesDirAsset {
        SubtitlesDirAsset::new(self.id)
    }

    pub fn subtitle(&self, id: String) -> SubtitleAsset {
        SubtitleAsset::new(self.id, id)
    }
}

impl<T: Media> LibraryItem<T> {
    pub async fn from_path(path: PathBuf, db: &crate::db::Db) -> Result<Self, anyhow::Error> {
        let video = Video::from_path(&path).await?;
        let file_name = path.file_name().context("get filename")?.to_string_lossy();
        let path_tokens = utils::tokenize_filename(&file_name);
        let identifier = match T::identify(&path_tokens) {
            Some(val) => val,
            None => {
                let metadata = video.metadata().await?;
                metadata
                    .format
                    .tags
                    .title
                    .as_ref()
                    .and_then(|metadata_title| {
                        let tokens: Vec<_> = metadata_title
                            .split_whitespace()
                            .map(|t| t.to_owned())
                            .collect();
                        T::identify(&tokens)
                    })
                    .context("Try to identify content from container metadata")?
            }
        };
        let source = Source::from_video(video, db).await?;
        Ok(Self { identifier, source })
    }
}

impl Library {
    pub fn new(videos: HashMap<i64, LibraryFile>) -> Self {
        Self { videos }
    }

    pub async fn init_from_folders(
        show_dirs: &Vec<PathBuf>,
        movie_dirs: &Vec<PathBuf>,
        db: &Db,
    ) -> Self {
        let mut videos = HashMap::new();
        for dir in show_dirs {
            videos.extend(
                explore_folder(dir, ContentType::Show, db, &Vec::new())
                    .await
                    .unwrap(),
            );
        }

        for dir in movie_dirs {
            videos.extend(
                explore_folder(dir, ContentType::Movie, db, &Vec::new())
                    .await
                    .unwrap(),
            );
        }
        Self { videos }
    }

    pub fn add_video(&mut self, id: i64, video: LibraryFile) {
        self.videos.insert(id, video);
    }

    pub fn remove_video_by_path(&mut self, path: impl AsRef<Path>) {
        if let Some(id) = self
            .videos
            .iter()
            .find(|(_, f)| f.source.video.path() == path.as_ref())
            .map(|(i, _f)| *i)
        {
            self.videos.remove(&id);
        };
    }

    pub fn remove_video(&mut self, id: i64) {
        self.videos.remove(&id);
    }

    pub fn find_video_by_path(&self, path: impl AsRef<Path>) -> Option<&Video> {
        self.videos
            .values()
            .find(|f| f.source.video.path() == path.as_ref())
            .map(|x| &x.source.video)
    }

    pub fn get_source(&self, id: i64) -> Option<&Source> {
        self.videos.get(&id).map(|f| &f.source)
    }

    pub fn get_source_mut(&mut self, id: i64) -> Option<&mut Source> {
        self.videos.get_mut(&id).map(|f| &mut f.source)
    }

    pub fn find_video_mut(&mut self, path: impl AsRef<Path>) -> Option<&mut Video> {
        self.videos
            .values_mut()
            .find(|f| f.source.video.path() == path.as_ref())
            .map(|x| &mut x.source.video)
    }

    pub fn episodes(&self) -> impl Iterator<Item = LibraryItem<ShowIdentifier>> + '_ {
        self.videos.values().filter_map(|v| match &v.identifier {
            ContentIdentifier::Show(i) => Some(LibraryItem {
                identifier: i.clone(),
                source: v.source.clone(),
            }),
            _ => None,
        })
    }

    pub fn movies(&self) -> impl Iterator<Item = LibraryItem<MovieIdentifier>> + '_ {
        self.videos.values().filter_map(|v| match &v.identifier {
            ContentIdentifier::Movie(i) => Some(LibraryItem {
                identifier: i.clone(),
                source: v.source.clone(),
            }),
            _ => None,
        })
    }

    pub fn get_movie(&self, video_id: i64) -> Option<LibraryItem<MovieIdentifier>> {
        self.videos
            .get(&video_id)
            .and_then(|v| v.clone().into_movie())
    }

    pub fn get_show(&self, video_id: i64) -> Option<LibraryItem<ShowIdentifier>> {
        self.videos
            .get(&video_id)
            .and_then(|v| v.clone().into_show())
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Chapter {
    pub title: String,
    pub start_time: String,
}

pub trait Media {
    fn identify(tokens: &[String]) -> Option<Self>
    where
        Self: Sized;
    fn title(&self) -> &str;
}

#[derive(Debug, Clone)]
pub struct Video {
    path: PathBuf,
    metadata: LazyFFprobeOutput,
}

/// Lazily evaluated ffprobe metadata
#[derive(Debug, Clone)]
struct LazyFFprobeOutput {
    metadata: Arc<OnceCell<FFprobeOutput>>,
}

impl LazyFFprobeOutput {
    fn new() -> Self {
        Self {
            metadata: Arc::new(OnceCell::new()),
        }
    }

    async fn get_or_init(&self, path: impl AsRef<Path>) -> anyhow::Result<&FFprobeOutput> {
        self.metadata
            .get_or_try_init(|| async { get_metadata(path).await })
            .await
    }

    fn get(&self) -> Option<&FFprobeOutput> {
        self.metadata.get()
    }
}

/// Ignores failed Video::new results
async fn get_variants(folder: impl AsRef<Path>) -> anyhow::Result<Vec<Video>> {
    let dir = std::fs::read_dir(&folder).context("failed to read dir")?;
    let mut out = Vec::new();
    for entry in dir {
        let Ok(file) = entry else {
            tracing::error!("Could not read file in dir {}", folder.as_ref().display());
            continue;
        };
        let Ok(file_type) = file.file_type() else {
            tracing::error!("Could not get filetype for file {}", file.path().display());
            continue;
        };
        // expect all files to be videos
        if file_type.is_file() {
            if let Ok(video) = Video::from_path(file.path()).await {
                out.push(video);
            }
        }
    }
    Ok(out)
}

impl Video {
    /// Returns struct compatible with database Video table
    pub async fn into_db_video(&self) -> DbVideo {
        let now = time::OffsetDateTime::now_utc();
        let duration = self
            .metadata()
            .await
            .map(FFprobeOutput::duration)
            .unwrap_or_default()
            .as_secs() as i64;

        DbVideo {
            id: None,
            path: self.path.to_string_lossy().to_string(),
            size: self.file_size() as i64,
            duration,
            scan_date: now.to_string(),
        }
    }

    pub async fn get_or_insert_id(&self, db: &crate::db::Db) -> anyhow::Result<i64> {
        let path = self.path().to_string_lossy().to_string();
        let res = sqlx::query!("SELECT id FROM videos WHERE path = ?", path)
            .fetch_one(&db.pool)
            .await;
        let video_id: Result<i64, anyhow::Error> = match res {
            Ok(r) => Ok(r.id),
            Err(sqlx::Error::RowNotFound) => {
                let db_video = self.into_db_video().await;
                let id = db.pool.insert_video(db_video).await?;
                Ok(id)
            }
            Err(e) => Err(e.into()),
        };
        video_id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Create self from path
    pub async fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        if tokio::fs::try_exists(&path).await? {
            Ok(Self {
                path: path.as_ref().to_path_buf(),
                metadata: LazyFFprobeOutput::new(),
            })
        } else {
            Err(anyhow::anyhow!(
                "Video {} does not exist",
                path.as_ref().display()
            ))
        }
    }

    pub async fn metadata(&self) -> anyhow::Result<&FFprobeOutput> {
        self.metadata.get_or_init(self.path()).await
    }

    /// Calculate hash for the video
    pub fn calculate_video_hash(&self) -> Result<u32, std::io::Error> {
        tracing::trace!("Calculating hash for file: {}", self.path.display());
        let path = &self.path;
        let mut file = std::fs::File::open(path)?;
        let hash = utils::file_hash(&mut file)?;
        Ok(hash)
    }

    /// Get file size in bytes
    pub fn file_size(&self) -> u64 {
        std::fs::metadata(&self.path).expect("to have access").len()
    }

    /// Delete self
    pub async fn delete(&self) -> Result<(), std::io::Error> {
        tokio::fs::remove_file(&self.path).await
    }

    pub async fn serve(&self, range: Option<TypedHeader<Range>>) -> impl IntoResponse {
        let file_size = self.file_size();
        let range = range.map(|h| h.0).unwrap_or(Range::bytes(0..).unwrap());
        let (start, end) = range
            .satisfiable_ranges(file_size)
            .next()
            .expect("at least one tuple");
        let start = match start {
            std::ops::Bound::Included(val) => val,
            std::ops::Bound::Excluded(val) => val,
            std::ops::Bound::Unbounded => 0,
        };

        let end = match end {
            std::ops::Bound::Included(val) => val,
            std::ops::Bound::Excluded(val) => val,
            std::ops::Bound::Unbounded => file_size,
        };

        let Ok(metadata) = self.metadata().await else {
            return AppError::internal_error("Failed to get file metadata").into_response();
        };
        let Ok(mut file) = tokio::fs::File::open(&self.path).await else {
            return AppError::internal_error("Failed to open file").into_response();
        };
        if file.seek(SeekFrom::Start(start)).await.is_err() {
            return AppError::bad_request("Failed to seek file to requested range").into_response();
        };

        let chunk_size = end - start + 1;
        let stream_of_bytes = FramedRead::new(file.take(chunk_size), BytesCodec::new());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from(end - start),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(metadata.guess_mime()),
        );
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=0"),
        );
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end - 1, file_size)).unwrap(),
        );

        (
            StatusCode::PARTIAL_CONTENT,
            headers,
            Body::from_stream(stream_of_bytes),
        )
            .into_response()
    }
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct TranscodePayload {
    pub audio_codec: Option<AudioCodec>,
    pub audio_track: Option<usize>,
    pub video_codec: Option<VideoCodec>,
    pub resolution: Option<Resolution>,
}

impl TranscodePayload {
    pub fn builder() -> TranscodePayloadBuilder {
        TranscodePayloadBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct TranscodePayloadBuilder {
    audio_codec: Option<AudioCodec>,
    audio_track: Option<usize>,
    video_codec: Option<VideoCodec>,
    resolution: Option<Resolution>,
}

impl TranscodePayloadBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn build(self) -> TranscodePayload {
        TranscodePayload {
            audio_codec: self.audio_codec,
            audio_track: self.audio_track,
            video_codec: self.video_codec,
            resolution: self.resolution,
        }
    }

    pub fn video_codec(mut self, codec: VideoCodec) -> Self {
        self.video_codec = Some(codec);
        self
    }

    pub fn audio_codec(mut self, codec: AudioCodec) -> Self {
        self.audio_codec = Some(codec);
        self
    }

    pub fn audio_track(mut self, track: usize) -> Self {
        self.audio_track = Some(track);
        self
    }

    pub fn resolution(mut self, resolution: Resolution) -> Self {
        self.resolution = Some(resolution);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AudioCodec {
    AAC,
    AC3,
    Other(String),
}

impl Display for AudioCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AAC => write!(f, "aac"),
            Self::AC3 => write!(f, "ac3"),
            Self::Other(codec) => write!(f, "{codec}"),
        }
    }
}

impl FromStr for AudioCodec {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed = match s {
            "aac" => AudioCodec::AAC,
            "ac3" => AudioCodec::AC3,
            rest => AudioCodec::Other(rest.to_string()),
        };
        Ok(parsed)
    }
}

impl<'de> Deserialize<'de> for AudioCodec {
    fn deserialize<D>(deserializer: D) -> Result<AudioCodec, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct AudioCodecVisitor;

        impl<'de> serde::de::Visitor<'de> for AudioCodecVisitor {
            type Value = AudioCodec;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an audio codec string")
            }

            fn visit_str<E>(self, value: &str) -> Result<AudioCodec, E>
            where
                E: serde::de::Error,
            {
                Ok(AudioCodec::from_str(value).expect("any str to be valid"))
            }
        }

        deserializer.deserialize_str(AudioCodecVisitor)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    Hevc,
    H264,
    Other(String),
}

impl Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hevc => write!(f, "hevc"),
            Self::H264 => write!(f, "h264"),
            Self::Other(codec) => write!(f, "{codec}"),
        }
    }
}

impl FromStr for VideoCodec {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed = match s {
            "hevc" => VideoCodec::Hevc,
            "h264" => VideoCodec::H264,
            rest => VideoCodec::Other(rest.to_string()),
        };
        Ok(parsed)
    }
}

impl<'de> Deserialize<'de> for VideoCodec {
    fn deserialize<D>(deserializer: D) -> Result<VideoCodec, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct VideoCodecVisitor;

        impl<'de> serde::de::Visitor<'de> for VideoCodecVisitor {
            type Value = VideoCodec;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an video codec string")
            }

            fn visit_str<E>(self, value: &str) -> Result<VideoCodec, E>
            where
                E: serde::de::Error,
            {
                Ok(VideoCodec::from_str(value).expect("any str to be valid"))
            }
        }

        deserializer.deserialize_str(VideoCodecVisitor)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resolution(pub (usize, usize));

impl<'__s> utoipa::ToSchema<'__s> for Resolution {
    fn schema() -> (
        &'__s str,
        utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>,
    ) {
        (
            "Resolution",
            utoipa::openapi::ObjectBuilder::new()
                .property(
                    "width",
                    utoipa::openapi::ObjectBuilder::new()
                        .schema_type(utoipa::openapi::SchemaType::Integer),
                )
                .required("width")
                .property(
                    "height",
                    utoipa::openapi::ObjectBuilder::new()
                        .schema_type(utoipa::openapi::SchemaType::Integer),
                )
                .required("height")
                .into(),
        )
    }
}

impl Resolution {
    pub fn new(width: usize, height: usize) -> Self {
        Self((width, height))
    }
}

impl Serialize for Resolution {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (x, y) = self.0;
        let mut resolution = serializer.serialize_struct("Resolution", 2)?;
        resolution.serialize_field("width", &x)?;
        resolution.serialize_field("height", &y)?;
        resolution.end()
    }
}

impl<'de> Deserialize<'de> for Resolution {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ResolutionVisitor;

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Height,
            Width,
        }

        impl<'de> Visitor<'de> for ResolutionVisitor {
            type Value = Resolution;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "String like 1920x1080 or tuple of integers or { height, width } object",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(Resolution::from_str(v).expect("any str to be valid"))
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::SeqAccess<'de>,
            {
                let width = seq
                    .next_element()?
                    .ok_or(serde::de::Error::missing_field("width"))?;
                let height = seq
                    .next_element()?
                    .ok_or(serde::de::Error::missing_field("height"))?;
                Ok(Resolution::from((width, height)))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut width = None;
                let mut height = None;
                while let Some(key) = map.next_key()? {
                    match key {
                        Field::Width => {
                            if width.is_some() {
                                return Err(serde::de::Error::duplicate_field("width"));
                            }
                            width = Some(map.next_value()?);
                        }
                        Field::Height => {
                            if height.is_some() {
                                return Err(serde::de::Error::duplicate_field("height"));
                            }
                            height = Some(map.next_value()?);
                        }
                    }
                }
                let width = width.ok_or_else(|| serde::de::Error::missing_field("width"))?;
                let height = height.ok_or_else(|| serde::de::Error::missing_field("height"))?;
                Ok(Resolution::new(width, height))
            }
        }
        deserializer.deserialize_any(ResolutionVisitor)
    }
}

impl Display for Resolution {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (x, y) = self.0;
        write!(f, "{}x{}", x, y)
    }
}

impl From<(usize, usize)> for Resolution {
    fn from(value: (usize, usize)) -> Self {
        Self((value.0, value.1))
    }
}

impl From<Resolution> for (usize, usize) {
    fn from(val: Resolution) -> Self {
        val.0
    }
}

impl FromStr for Resolution {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (x, y) = s
            .split_once('x')
            .ok_or(anyhow::anyhow!("str must be separated with 'x'"))?;
        let x = x.parse()?;
        let y = y.parse()?;
        Ok((x, y).into())
    }
}

#[derive(Debug, Deserialize, Clone, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", untagged)]
pub enum SubtitlesCodec {
    SubRip,
    WebVTT,
    DvdSubtitle,
    Other(String),
}

impl Serialize for SubtitlesCodec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl Display for SubtitlesCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SubRip => write!(f, "subrip"),
            Self::WebVTT => write!(f, "webvtt"),
            Self::DvdSubtitle => write!(f, "dvd_subtitle"),
            Self::Other(codec) => write!(f, "{codec}"),
        }
    }
}

impl FromStr for SubtitlesCodec {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parsed = match s {
            "subrip" => SubtitlesCodec::SubRip,
            "webvtt" => SubtitlesCodec::WebVTT,
            "dvd_subtitle" => SubtitlesCodec::DvdSubtitle,
            rest => SubtitlesCodec::Other(rest.to_string()),
        };
        Ok(parsed)
    }
}

impl SubtitlesCodec {
    pub fn supports_text(&self) -> bool {
        match self {
            SubtitlesCodec::SubRip => true,
            SubtitlesCodec::WebVTT => true,
            SubtitlesCodec::DvdSubtitle => false,
            SubtitlesCodec::Other(_) => false,
        }
    }
}
