use std::{
    convert::Infallible,
    fmt::Display,
    io::SeekFrom,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime},
};

use anyhow::Context;
use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::IntoResponse,
};
use axum_extra::{headers::Range, TypedHeader};
use serde::{de::Visitor, ser::SerializeStruct, Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::codec::{BytesCodec, FramedRead};

use crate::{
    db::DbVideo,
    ffmpeg::{get_metadata, FFprobeAudioStream, FFprobeSubtitleStream, FFprobeVideoStream},
    ffmpeg::{FFmpegRunningJob, FFprobeOutput, PreviewsJob, SubtitlesJob, TranscodeJob},
    utils,
};

use self::movie::MovieIdentifier;
use self::show::ShowIdentifier;

pub mod movie;
pub mod show;

const SUPPORTED_FILES: &[&str] = &["mkv", "webm", "mp4"];

#[derive(Debug, Serialize, Clone)]
pub struct Summary {
    pub subs: Vec<String>,
    pub previews: usize,
    pub duration: Duration,
    pub title: String,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug)]
pub struct Library {
    pub shows: Vec<LibraryFile<ShowIdentifier>>,
    pub movies: Vec<LibraryFile<MovieIdentifier>>,
    pub media_folders: MediaFolders,
    summary: Vec<Summary>,
}

fn extract_summary(file: &LibraryFile<impl Media>) -> Summary {
    let source = &file.source;
    return Summary {
        previews: source.previews_count(),
        subs: source.get_subs(),
        duration: source.origin.duration(),
        title: source.metadata_title(),
        chapters: source.origin.chapters(),
    };
}

pub fn is_format_supported(path: &impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .map_or(false, |ex| SUPPORTED_FILES.contains(&ex.to_str().unwrap()))
}

#[tracing::instrument(level = "trace", name = "explore library folder")]
pub async fn explore_folder<T: Media + Send + 'static>(
    folder: &PathBuf,
) -> Result<Vec<LibraryFile<T>>, anyhow::Error> {
    let paths = utils::walk_recursive(folder, Some(is_format_supported))?;
    let mut handles = Vec::new();

    for path in paths {
        handles.push(tokio::spawn(async move { LibraryFile::from_path(path) }));
    }

    let mut result = Vec::new();

    for handle in handles {
        if let Ok(item) = handle.await {
            let _ = item.map(|x| result.push(x));
        } else {
            tracing::error!("One of the metadata collectors paniced");
        }
    }

    return Ok(result);
}

#[derive(Debug, Clone)]
pub struct LibraryFile<T: Media> {
    pub identifier: T,
    pub source: Source,
    pub data_folder: DataFolder,
}

#[derive(Debug, Clone)]
pub struct DataFolder {
    pub size: u64,
    pub time_hash: String,
}

impl DataFolder {
    fn time_hash(time: SystemTime) -> u32 {
        use crc32fast::Hasher;
        use std::hash::Hash;
        let mut hasher = Hasher::new();
        time.hash(&mut hasher);
        hasher.finalize()
    }

    pub fn from_metadata(metadata: &std::fs::Metadata) -> Self {
        let size = metadata.len();
        let created_time = metadata.created().unwrap_or(SystemTime::UNIX_EPOCH);
        let hash = Self::time_hash(created_time).to_string();
        Self {
            time_hash: hash,
            size,
        }
    }

    pub fn from_path(path: &PathBuf) -> Option<Self> {
        let name = path.iter().last()?.to_string_lossy().to_string();
        let (size, hash) = name.split_once('.')?;
        Some(Self {
            size: size.parse().ok()?,
            time_hash: hash.to_string(),
        })
    }

    pub fn verify_existance(&self) -> bool {
        let mut episode_dir_path =
            PathBuf::from(std::env::var("RESOURCES_PATH").expect("env to be set"));
        episode_dir_path.push("content");
        episode_dir_path.push(self.to_string());
        episode_dir_path.try_exists().unwrap_or(false)
    }
}

impl Display for DataFolder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.size, self.time_hash)
    }
}

impl<T: Media> LibraryFile<T> {
    pub fn from_path(path: PathBuf) -> Result<Self, anyhow::Error> {
        let video = Video::from_path(&path)?;
        let metadata = path.metadata()?;
        let file_name = path
            .file_name()
            .ok_or(anyhow::anyhow!("failed to get filename"))?
            .to_string_lossy()
            .to_string();
        let data_folder = DataFolder::from_metadata(&metadata);
        let resources_path = generate_resources_path(&data_folder);
        utils::generate_resources(&resources_path)?;
        let metadata_title = video.metadata.format.tags.title.clone();
        let source = Source::from_video(video, &resources_path)?;
        let path_tokens = utils::tokenize_filename(file_name);
        let identifier = T::identify(path_tokens)
            .or_else(|| {
                let video_metadata_title = metadata_title;
                T::identify(
                    video_metadata_title
                        .split_whitespace()
                        .map(|x| x.to_string())
                        .collect(),
                )
            })
            .ok_or(anyhow::anyhow!("Could not identify file"))?;
        Ok(Self {
            identifier,
            source,
            data_folder,
        })
    }
}

fn generate_resources_path(folder: &DataFolder) -> PathBuf {
    let mut episode_dir_path =
        PathBuf::from(std::env::var("RESOURCES_PATH").expect("env to be set"));
    episode_dir_path.push("content");
    episode_dir_path.push(folder.to_string());
    episode_dir_path
}

impl Library {
    pub fn new(
        media_folders: MediaFolders,
        shows: Vec<LibraryFile<ShowIdentifier>>,
        movies: Vec<LibraryFile<MovieIdentifier>>,
    ) -> Self {
        let mut summary = Vec::new();
        for item in &shows {
            summary.push(extract_summary(item));
        }
        for item in &movies {
            summary.push(extract_summary(item));
        }
        Self {
            media_folders,
            shows,
            movies,
            summary,
        }
    }

    pub fn add_show(&mut self, path: PathBuf) -> anyhow::Result<LibraryFile<ShowIdentifier>> {
        LibraryFile::from_path(path).map(|show| {
            self.shows.push(show.clone());
            show
        })
    }

    pub fn add_movie(&mut self, path: PathBuf) -> anyhow::Result<LibraryFile<MovieIdentifier>> {
        LibraryFile::from_path(path).map(|movie| {
            self.movies.push(movie.clone());
            movie
        })
    }

    pub fn add_variant(&mut self, path: impl AsRef<Path>, video: Video) {
        let item = self.find_source_mut(path).unwrap();
        item.add_variant(video)
    }

    pub fn remove_variant(&mut self, video_path: impl AsRef<Path>, variant_id: &str) {
        let item = self.find_source_mut(video_path).unwrap();
        item.delete_variant(variant_id);
    }

    pub fn remove_show(&mut self, path: impl AsRef<Path>) {
        self.shows
            .iter()
            .position(|f| f.source.source_path() == path.as_ref())
            .map(|pos| self.shows.remove(pos));
    }

    pub fn remove_movie(&mut self, path: impl AsRef<Path>) {
        self.movies
            .iter()
            .position(|f| f.source.source_path() == path.as_ref())
            .map(|pos| self.movies.remove(pos));
    }

    pub fn remove_file(&mut self, path: impl AsRef<Path>) {
        self.remove_show(&path);
        self.remove_movie(path);
    }

    pub fn get_summary(&self) -> Vec<Summary> {
        self.summary.clone()
    }

    pub fn find_source(&self, path: impl AsRef<Path>) -> Option<&Source> {
        let show = self
            .shows
            .iter()
            .find(|f| f.source.source_path() == path.as_ref())
            .map(|x| &x.source);
        if show.is_none() {
            return self
                .movies
                .iter()
                .find(|f| f.source.source_path() == path.as_ref())
                .map(|x| &x.source);
        }
        show
    }

    pub fn find_source_mut(&mut self, path: impl AsRef<Path>) -> Option<&mut Source> {
        let show = self
            .shows
            .iter_mut()
            .find(|f| f.source.source_path() == path.as_ref())
            .map(|x| &mut x.source);
        if show.is_none() {
            return self
                .movies
                .iter_mut()
                .find(|f| f.source.source_path() == path.as_ref())
                .map(|x| &mut x.source);
        }
        show
    }

    pub async fn full_refresh(&mut self) {
        let mut shows = Vec::new();
        for folder in &self.media_folders.shows {
            if let Ok(items) = explore_folder(folder).await {
                shows.extend(items);
            }
        }
        self.shows = shows;

        let mut movies = Vec::new();
        for folder in &self.media_folders.movies {
            if let Ok(items) = explore_folder(folder).await {
                movies.extend(items);
            }
        }
        self.movies = movies;
    }
}

#[derive(Debug, Clone)]
pub struct MediaFolders {
    pub shows: Vec<PathBuf>,
    pub movies: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum MediaType {
    Show,
    Movie,
}

impl MediaFolders {
    pub fn all(&self) -> Vec<&PathBuf> {
        let mut out = Vec::with_capacity(self.shows.len() + self.movies.len());
        out.extend(self.shows.iter());
        out.extend(self.movies.iter());
        out
    }

    pub fn folder_type(&self, path: &PathBuf) -> Option<MediaType> {
        for show_dir in &self.shows {
            if path.starts_with(show_dir) {
                return Some(MediaType::Show);
            };
        }
        for movie_dir in &self.movies {
            if path.starts_with(movie_dir) {
                return Some(MediaType::Movie);
            };
        }
        None
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Chapter {
    pub title: String,
    pub start_time: String,
}

pub trait Media {
    fn identify(tokens: Vec<String>) -> Option<Self>
    where
        Self: Sized;
    fn title(&self) -> &str;
}

#[derive(Debug, Clone, Serialize)]
pub struct Source {
    pub origin: Video,
    pub variants: Vec<Video>,
    pub resources_path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Video {
    pub path: PathBuf,
    metadata: FFprobeOutput,
}

/// Ignores failed Video::new results
fn get_variants(folder: impl AsRef<Path>) -> anyhow::Result<Vec<Video>> {
    let dir = std::fs::read_dir(folder).context("failed to read dir")?;
    let vec = dir
        .into_iter()
        .filter_map(|f| {
            let file = f.ok()?;
            let filetype = file.file_type().ok()?;
            if filetype.is_file() {
                Some(Video::from_path(file.path()).ok()?)
            } else {
                None
            }
        })
        .collect();
    Ok(vec)
}

impl Source {
    pub fn new(
        source_path: impl AsRef<Path>,
        resources_path: impl AsRef<Path>,
    ) -> anyhow::Result<Self> {
        let resources_path = resources_path.as_ref().to_path_buf();
        let origin = Video::from_path(source_path)?;
        let variants =
            get_variants(&resources_path.join("variants")).context("failed to get variants")?;
        Ok(Self {
            origin,
            variants,
            resources_path,
        })
    }

    pub fn from_video(
        source_video: Video,
        resources_path: impl AsRef<Path>,
    ) -> anyhow::Result<Self> {
        let resources_path = resources_path.as_ref().to_path_buf();
        let variants =
            get_variants(&resources_path.join("variants")).context("failed to get variants")?;
        Ok(Self {
            origin: source_video,
            variants,
            resources_path,
        })
    }

    pub fn resources_folder_name(&self) -> String {
        self.resources_path
            .file_name()
            .expect("resources_folder to exist")
            .to_string_lossy()
            .to_string()
    }

    /// Find variant
    pub fn find_variant(&self, file_name: &str) -> Option<Video> {
        self.variants
            .iter()
            .find(|v| v.path.file_stem().map(|s| s == file_name).unwrap_or(false))
            .cloned()
    }

    /// Add variant
    pub fn add_variant(&mut self, video: Video) {
        self.variants.push(video);
    }

    /// Delete variant
    pub fn delete_variant(&mut self, variant_id: &str) {
        let position = self.variants.iter().position(|v| {
            v.path
                .file_stem()
                .map(|stem| stem.to_string_lossy().to_string() == variant_id)
                .unwrap_or(false)
        });
        if let Some(position) = position {
            if std::fs::remove_file(&self.variants[position].path).is_ok() {
                self.variants.remove(position);
            };
        }
    }

    /// Remove all files that belong to source
    pub fn delete(&self) -> Result<(), std::io::Error> {
        self.origin.delete()?;
        self.delete_resources()
    }

    /// Get origin video duration
    pub fn duration(&self) -> Duration {
        self.origin.duration()
    }

    /// Source file folder path
    pub fn source_path(&self) -> &Path {
        &self.origin.path
    }

    /// Previews folder path
    pub fn previews_path(&self) -> PathBuf {
        self.resources_path.join("previews")
    }

    /// Subtitles forder path
    pub fn subtitles_path(&self) -> PathBuf {
        self.resources_path.join("subs")
    }

    /// Variants forder path
    pub fn variants_path(&self) -> PathBuf {
        self.resources_path.join("variants")
    }

    /// Get prewies count
    pub fn previews_count(&self) -> usize {
        return std::fs::read_dir(self.previews_path()).unwrap().count();
    }

    /// Clean all generated resources
    pub fn delete_resources(&self) -> Result<(), std::io::Error> {
        std::fs::remove_dir_all(&self.resources_path)
    }

    /// Get title included in file metadata
    pub fn metadata_title(&self) -> String {
        self.origin.metadata.format.tags.title.clone()
    }

    /// Get subtitles list
    pub fn get_subs(&self) -> Vec<String> {
        std::fs::read_dir(self.subtitles_path())
            .unwrap()
            .map(|sub| {
                sub.unwrap()
                    .path()
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect()
    }

    /// Generate previews for file
    pub fn generate_previews(&self) -> FFmpegRunningJob<PreviewsJob> {
        let job = PreviewsJob::from_source(&self);
        FFmpegRunningJob::new_running(job, self.source_path().into(), self.duration())
    }

    /// Generate subtitles for file
    pub fn generate_subtitles(
        &self,
        track: i32,
    ) -> Result<FFmpegRunningJob<SubtitlesJob>, anyhow::Error> {
        SubtitlesJob::from_source(&self, track as usize).map(|job| {
            FFmpegRunningJob::new_running(job, self.source_path().into(), self.duration())
        })
    }

    /// Transcode file
    pub fn transcode_video(
        &self,
        payload: TranscodePayload,
    ) -> Result<FFmpegRunningJob<TranscodeJob>, anyhow::Error> {
        TranscodeJob::from_source(&self, payload).map(|job| {
            FFmpegRunningJob::new_running(job, self.source_path().into(), self.duration())
        })
    }

    /// Returns struct compatible with database Video table
    pub fn into_db_video(&self) -> DbVideo {
        let origin = &self.origin;
        let now = time::OffsetDateTime::now_utc();

        DbVideo {
            id: None,
            path: self.source_path().to_string_lossy().to_string(),
            resources_folder: self.resources_folder_name(),
            size: origin.file_size() as i64,
            scan_date: now.to_string(),
        }
    }
}

impl Video {
    /// Create self from path
    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        get_metadata(&path).map(|metadata| Self {
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
        return Ok(hash);
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
    pub fn delete(&self) -> Result<(), std::io::Error> {
        std::fs::remove_file(&self.path)
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, PartialEq)]
pub enum AudioCodec {
    AAC,
    AC3,
    Other(String),
}

impl Serialize for AudioCodec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
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

#[derive(Debug, Clone, PartialEq)]
pub enum VideoCodec {
    Hevc,
    H264,
    Other(String),
}

impl Serialize for VideoCodec {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
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

#[derive(Debug, Deserialize, Clone, PartialEq)]
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

#[tokio::test]
async fn cancel_transcode() {
    use crate::progress::{TaskKind, TaskResource};
    use crate::testing::TestResource;
    use std::time::Duration;
    use tokio::fs;
    use tokio::time;

    let testing_resource = TestResource::new(true);
    let subject = testing_resource.test_show.clone();
    let task_resource = TaskResource::new();
    let size_before = fs::metadata(&subject.source.origin.path)
        .await
        .unwrap()
        .len();
    let video_path = subject.source.source_path().to_path_buf();
    let payload = TranscodePayloadBuilder::new()
        .video_codec(VideoCodec::Hevc)
        .build();

    let process = subject.source.transcode_video(payload).unwrap();
    {
        let task_resource = task_resource.clone();
        tokio::spawn(async move {
            time::sleep(Duration::from_secs(2)).await;
            let id = {
                let tasks = task_resource.tasks.lock().unwrap();
                tasks.first().unwrap().id
            };
            task_resource.cancel_task(id).unwrap();
        });
    }
    let target_buffer = process.job.output_path.clone();
    let result = task_resource
        .observe_ffmpeg_task(process, TaskKind::Transcode)
        .await;
    // task is canceled
    assert!(result.is_err());

    let size_after = fs::metadata(&video_path).await.unwrap().len();
    let is_cleaned = !fs::try_exists(target_buffer).await.unwrap();
    // file is untouched
    assert_eq!(size_before, size_after);
    // variant is cleared
    assert!(is_cleaned);
}

#[tokio::test]
async fn transcode_video() {
    use crate::testing::TestResource;

    let testing_resource = TestResource::new(false);
    let subject = testing_resource.test_show.clone();
    let source = subject.source;
    let default_audio_idx = source.origin.default_audio().unwrap().index as usize;
    let desired_video_codec = VideoCodec::Hevc;
    let desired_audio_codec = AudioCodec::AAC;
    let desired_resoultion = Resolution((80, 60));
    let payload = TranscodePayloadBuilder::new()
        .video_codec(desired_video_codec)
        .audio_codec(desired_audio_codec)
        .resolution(desired_resoultion)
        .audio_track(default_audio_idx)
        .build();
    let mut job = source.transcode_video(payload).unwrap();
    job.wait().await.unwrap();
}
