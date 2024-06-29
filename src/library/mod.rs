use std::{
    collections::HashMap,
    convert::Infallible,
    fmt::Display,
    io::SeekFrom,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
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
    sync::Semaphore,
};
use tokio_util::codec::{BytesCodec, FramedRead};

use crate::{
    db::DbVideo,
    ffmpeg::{
        get_metadata, FFprobeAudioStream, FFprobeOutput, FFprobeSubtitleStream, FFprobeVideoStream,
    },
    metadata::{EpisodeMetadata, MovieMetadata},
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

#[derive(Debug)]
pub struct Library {
    pub shows: HashMap<i64, LibraryFile<ShowIdentifier>>,
    pub movies: HashMap<i64, LibraryFile<MovieIdentifier>>,
}

pub fn is_format_supported(path: &impl AsRef<Path>) -> bool {
    let path = path.as_ref().to_path_buf();
    let is_extra = path
        .components()
        .into_iter()
        .any(|c| EXTRAS_FOLDERS.contains(&c.as_os_str().to_string_lossy().to_lowercase().as_ref()));
    let supports_extension = path
        .extension()
        .map_or(false, |ex| SUPPORTED_FILES.contains(&ex.to_str().unwrap()));
    !is_extra && supports_extension
}

pub async fn explore_folder<T: Media + Send + 'static>(
    folder: &PathBuf,
    db: &crate::db::Db,
    exclude: &Vec<PathBuf>,
) -> Result<HashMap<i64, LibraryFile<T>>, anyhow::Error> {
    let paths = utils::walk_recursive(folder, Some(is_format_supported))?;
    let mut handles = Vec::with_capacity(paths.len());
    let semaphore = Arc::new(Semaphore::new(100));
    for path in paths {
        if exclude.contains(&path) {
            continue;
        }
        let semaphore = semaphore.clone();
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire().await;
            LibraryFile::from_path(path, &db).await
        }));
    }

    let mut result = HashMap::with_capacity(handles.len());

    for handle in handles {
        match handle.await {
            Ok(Ok(item)) => {
                result.insert(item.source.id, item);
            }
            Ok(Err(e)) => tracing::warn!("One of the metadata collectors errored: {}", e),
            Err(e) => tracing::error!("One of the metadata collectors paniced: {}", e),
        }
    }

    return Ok(result);
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContentIdentifier {
    Show(ShowIdentifier),
    Movie(MovieIdentifier),
}

#[derive(Debug, Clone)]
pub struct LibraryFile<T: Media> {
    pub identifier: T,
    pub source: Source,
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

impl<T: Media> LibraryFile<T> {
    pub async fn from_path(path: PathBuf, db: &crate::db::Db) -> Result<Self, anyhow::Error> {
        let video = Video::from_path(&path).await?;
        let metadata_title = video.metadata.format.tags.title.clone();
        let source = Source::from_video(video, db).await?;
        let file_name = path
            .file_name()
            .ok_or(anyhow::anyhow!("failed to get filename"))?
            .to_string_lossy()
            .to_string();
        let path_tokens = utils::tokenize_filename(file_name);
        let identifier = T::identify(&path_tokens)
            .or_else(|| {
                metadata_title.and_then(|video_metadata_title| {
                    T::identify(
                        &video_metadata_title
                            .split_whitespace()
                            .map(|x| x.to_string())
                            .collect(),
                    )
                })
            })
            .ok_or(anyhow::anyhow!(
                "Could not identify file: {}",
                path.file_name().unwrap_or_default().display()
            ))?;
        Ok(Self { identifier, source })
    }
}

impl LibraryFile<ShowIdentifier> {
    pub fn into_episode_metadata(&self) -> EpisodeMetadata {
        EpisodeMetadata {
            metadata_id: uuid::Uuid::new_v4().to_string(),
            metadata_provider: crate::metadata::MetadataProvider::Local,
            release_date: None,
            number: self.identifier.episode.into(),
            title: self.identifier.title.to_string(),
            plot: None,
            season_number: self.identifier.season.into(),
            runtime: Some(self.source.video.duration()),
            poster: None,
        }
    }
}

impl LibraryFile<MovieIdentifier> {
    pub fn into_movie_metadata(&self) -> MovieMetadata {
        MovieMetadata {
            metadata_id: uuid::Uuid::new_v4().to_string(),
            metadata_provider: crate::metadata::MetadataProvider::Local,
            poster: None,
            backdrop: None,
            plot: None,
            release_date: None,
            title: self.identifier.title.clone(),
        }
    }
}

impl Library {
    pub fn new(
        shows: HashMap<i64, LibraryFile<ShowIdentifier>>,
        movies: HashMap<i64, LibraryFile<MovieIdentifier>>,
    ) -> Self {
        Self {
            shows,
            movies,
        }
    }

    pub fn add_show(&mut self, id: i64, show: LibraryFile<ShowIdentifier>) {
        self.shows.insert(id, show);
    }

    pub fn add_movie(&mut self, id: i64, movie: LibraryFile<MovieIdentifier>) {
        self.movies.insert(id, movie);
    }

    pub fn remove_show_by_path(&mut self, path: impl AsRef<Path>) {
        if let Some(id) = self
            .shows
            .iter()
            .find(|(_, f)| f.source.video.path() == path.as_ref())
            .map(|(i, _f)| *i)
        {
            self.shows.remove(&id);
        };
    }

    pub fn remove_movie_by_path(&mut self, path: impl AsRef<Path>) {
        if let Some(id) = self
            .movies
            .iter()
            .find(|(_, f)| f.source.video.path() == path.as_ref())
            .map(|(i, _f)| *i)
        {
            self.movies.remove(&id);
        };
    }

    pub fn remove_show(&mut self, id: i64) {
        self.shows.remove(&id);
    }

    pub fn remove_movie(&mut self, id: i64) {
        self.movies.remove(&id);
    }

    pub fn remove_file_by_path(&mut self, path: impl AsRef<Path>) {
        self.remove_show_by_path(&path);
        self.remove_movie_by_path(path);
    }

    /// Remove video from the library
    /// NOTE: it does not physically delete video
    pub fn remove_file(&mut self, id: i64) {
        self.remove_show(id);
        self.remove_movie(id);
    }

    pub fn find_video_by_path(&self, path: impl AsRef<Path>) -> Option<&Video> {
        let show = self
            .shows
            .values()
            .find(|f| f.source.video.path() == path.as_ref())
            .map(|x| &x.source.video);
        if show.is_none() {
            return self
                .movies
                .values()
                .find(|f| f.source.video.path() == path.as_ref())
                .map(|x| &x.source.video);
        }
        show
    }

    pub fn get_source(&self, id: i64) -> Option<&Source> {
        let show = self.shows.get(&id).map(|f| &f.source);
        if show.is_none() {
            return self.movies.get(&id).map(|f| &f.source);
        }
        show
    }

    pub fn get_source_mut(&mut self, id: i64) -> Option<&mut Source> {
        let show = self.shows.get_mut(&id).map(|f| &mut f.source);
        if show.is_none() {
            return self.movies.get_mut(&id).map(|f| &mut f.source);
        }
        show
    }

    pub fn find_video_mut(&mut self, path: impl AsRef<Path>) -> Option<&mut Video> {
        let show = self
            .shows
            .values_mut()
            .find(|f| f.source.video.path() == path.as_ref())
            .map(|x| &mut x.source.video);
        if show.is_none() {
            return self
                .movies
                .values_mut()
                .find(|f| f.source.video.path() == path.as_ref())
                .map(|x| &mut x.source.video);
        }
        show
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Chapter {
    pub title: String,
    pub start_time: String,
}

pub trait Media {
    fn identify(tokens: &Vec<String>) -> Option<Self>
    where
        Self: Sized;
    fn title(&self) -> &str;
}

#[derive(Debug, Clone, Serialize)]
pub struct Video {
    path: PathBuf,
    metadata: FFprobeOutput,
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
    pub fn into_db_video(&self) -> DbVideo {
        let now = time::OffsetDateTime::now_utc();
        let duration = self.duration().as_secs() as i64;

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
            Ok(r) => Ok(r.id.unwrap()),
            Err(sqlx::Error::RowNotFound) => {
                let db_video = self.into_db_video();
                let id = db.insert_video(db_video).await?;
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
        get_metadata(&path).await.map(|metadata| Self {
            path: path.as_ref().to_path_buf(),
            metadata,
        })
    }

    /// Calculate hash for the video
    pub fn calculate_video_hash(&self) -> Result<u32, std::io::Error> {
        tracing::trace!("Calculating hash for file: {}", self.path.display());
        let path = &self.path;
        let mut file = std::fs::File::open(path)?;
        let hash = utils::file_hash(&mut file)?;
        Ok(hash)
    }

    /// Chapters
    pub fn chapters(&self) -> Vec<Chapter> {
        self.metadata
            .chapters
            .iter()
            .map(|ffprobe_chapter| Chapter {
                title: ffprobe_chapter.tags.title.clone(),
                start_time: ffprobe_chapter.start_time.clone(),
            })
            .collect()
    }

    /// Get file size in bytes
    pub fn file_size(&self) -> u64 {
        std::fs::metadata(&self.path).expect("to have access").len()
    }

    /// Get video duration
    pub fn duration(&self) -> Duration {
        self.metadata.duration()
    }

    /// Delete self
    pub async fn delete(&self) -> Result<(), std::io::Error> {
        tokio::fs::remove_file(&self.path).await
    }

    pub fn video_streams(&self) -> Vec<FFprobeVideoStream> {
        self.metadata.video_streams()
    }

    pub fn audio_streams(&self) -> Vec<FFprobeAudioStream> {
        self.metadata.audio_streams()
    }

    pub fn subtitle_streams(&self) -> Vec<FFprobeSubtitleStream> {
        self.metadata.subtitle_streams()
    }

    /// Default audio stream
    pub fn default_audio(&self) -> Option<FFprobeAudioStream> {
        self.metadata.default_audio()
    }

    /// Default video stream
    pub fn default_video(&self) -> Option<FFprobeVideoStream> {
        self.metadata.default_video()
    }

    /// Default subtitles stream
    pub fn default_subtitles(&self) -> Option<FFprobeSubtitleStream> {
        self.metadata.default_subtitles()
    }

    /// Video resoultion
    pub fn resolution(&self) -> Option<Resolution> {
        self.metadata.resolution()
    }

    /// Video bitrate
    pub fn bitrate(&self) -> usize {
        self.metadata.bitrate()
    }

    /// Title included in file metadata
    pub fn metadata_title(&self) -> Option<String> {
        self.metadata.format.tags.title.clone()
    }

    /// Video mime type
    pub fn guess_mime_type(&self) -> &'static str {
        self.metadata.guess_mime()
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
        let mut file = tokio::fs::File::open(&self.path)
            .await
            .expect("file to be open");

        let chunk_size = end - start + 1;
        file.seek(SeekFrom::Start(start)).await.unwrap();
        let stream_of_bytes = FramedRead::new(file.take(chunk_size), BytesCodec::new());
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from(end - start),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static(self.guess_mime_type()),
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

        return (
            StatusCode::PARTIAL_CONTENT,
            headers,
            Body::from_stream(stream_of_bytes),
        );
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

impl Into<(usize, usize)> for Resolution {
    fn into(self) -> (usize, usize) {
        self.0
    }
}

impl FromStr for Resolution {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (x, y) = s
            .split_once('x')
            .ok_or(anyhow::anyhow!("str must be seperated with 'x'"))?;
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
