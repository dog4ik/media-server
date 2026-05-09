use std::{
    collections::HashMap,
    ffi::OsStr,
    path::{Path, PathBuf},
    time::Instant,
};

use anyhow::Context;
use identification::{walk_movie_dirs, walk_show_dirs};
use serde::{Deserialize, Serialize};

use self::media::codec::audio::AudioCodec;
use self::media::codec::subtitles::SubtitlesCodec;
use self::media::codec::video::VideoCodec;
use self::media::container::VideoContainer;
use self::media::{Resolution, Video};
use crate::db::Db;

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

/// Saved local assets like posters
pub mod assets;
/// Identification for extras file names
#[allow(unused)]
pub mod extras;
/// Local files tokenizer
pub mod identification;
/// Libarry videos and it's components
pub mod media;
/// Identification for movie file names
pub mod movie;
/// Identification for show file names
pub mod show;

const EXTRAS_FOLDERS: [&str; 14] = [
    "behind the scenes",
    "deleted scenes",
    "interviews",
    "scenes",
    "screens",
    "samples",
    "sample",
    "shorts",
    "featurettes",
    "clips",
    "other",
    "extras",
    "trailers",
    "special",
];

/// Mapping between database videos and local files
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
        .is_some_and(|ex| VideoContainer::try_from(ex).is_ok());
    !is_extra && supports_extension
}

pub async fn explore_show_dirs(
    folders: Vec<PathBuf>,
    db: &crate::db::Db,
    library: &mut HashMap<i64, LibraryFile>,
    exclude: &[PathBuf],
) {
    let videos = walk_show_dirs(folders);
    let mut tx = db.begin().await.expect("transaction to begin");
    let start = Instant::now();
    for (video, identifier) in videos {
        let path = video.path();
        if exclude.iter().any(|p| p == path) {
            continue;
        }
        let source = match Source::from_video(video, &mut tx).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to construct source: {e}");
                continue;
            }
        };
        let id = source.id;
        let library_file = LibraryItem { identifier, source };
        library.insert(id, library_file.into());
    }

    tx.commit().await.expect("if it fails, we are cooked");
    tracing::debug!(took = ?start.elapsed(), "Finished video reconcilliation");
}

pub async fn explore_movie_dirs(
    folders: Vec<PathBuf>,
    db: &crate::db::Db,
    library: &mut HashMap<i64, LibraryFile>,
    exclude: &[PathBuf],
) {
    let videos = walk_movie_dirs(folders).await;
    let mut tx = db.begin().await.expect("transaction begin");
    for (video, identifier) in videos {
        let path = video.path();
        if exclude.iter().any(|p| p == path) {
            continue;
        }
        let source = match Source::from_video(video, &mut tx).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("Failed to construct source: {e}");
                continue;
            }
        };
        let id = source.id;
        let library_file = LibraryItem { identifier, source };
        library.insert(id, library_file.into());
    }
    tx.commit().await.expect("if this fails we are cooked");
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
    pub async fn from_video(
        video: Video,
        db: &mut crate::db::DbTransaction,
    ) -> anyhow::Result<Self> {
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
    pub async fn from_path(
        path: impl AsRef<Path>,
        db: &mut crate::db::DbTransaction,
    ) -> anyhow::Result<Self> {
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

    pub fn find_variant_video(&self, id: &str) -> Option<&Video> {
        self.variants.iter().find(|v| {
            v.path()
                .file_stem()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name == id)
        })
    }

    pub fn subtitles_dir(&self) -> SubtitlesDirAsset {
        SubtitlesDirAsset::new(self.id)
    }

    pub fn subtitle(&self, id: i64) -> SubtitleAsset {
        SubtitleAsset::new(self.id, id)
    }
}

impl<T: Media> LibraryItem<T> {
    // Identification in this
    pub async fn from_path(path: PathBuf, db: &crate::db::Db) -> Result<Self, anyhow::Error> {
        let video = Video::from_path(&path).await?;
        let file_name = path.file_name().context("get filename")?;
        let identifier = match T::identify(file_name) {
            Ok(val) => val,
            Err(_) => {
                let metadata = video.metadata().await?;
                metadata
                    .tag_title()
                    .and_then(|metadata_title| T::identify(metadata_title).ok())
                    .context("Try to identify content from container metadata")?
            }
        };
        let mut tx = db.begin().await?;
        let source = Source::from_video(video, &mut tx).await?;
        tx.commit().await?;
        Ok(Self { identifier, source })
    }
}

impl Library {
    pub fn new(videos: HashMap<i64, LibraryFile>) -> Self {
        Self { videos }
    }

    pub async fn init_from_folders(
        show_dirs: Vec<PathBuf>,
        movie_dirs: Vec<PathBuf>,
        db: &Db,
    ) -> Self {
        let mut videos = HashMap::new();
        explore_show_dirs(show_dirs, db, &mut videos, &[]).await;

        explore_movie_dirs(movie_dirs, db, &mut videos, &[]).await;
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

    pub fn remove_video(&mut self, id: i64) -> Option<LibraryFile> {
        self.videos.remove(&id)
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

    pub fn episodes(&self) -> impl Iterator<Item = LibraryItem<ShowIdentifier>> + use<'_> {
        self.videos.values().filter_map(|v| match &v.identifier {
            ContentIdentifier::Show(i) => Some(LibraryItem {
                identifier: i.clone(),
                source: v.source.clone(),
            }),
            _ => None,
        })
    }

    pub fn movies(&self) -> impl Iterator<Item = LibraryItem<MovieIdentifier>> + use<'_> {
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
    type Ident;
    fn identify(path: impl AsRef<Path>) -> Result<Self, Self::Ident>
    where
        Self: Sized;
    fn title(&self) -> &str;
}

#[derive(Debug, Clone, Deserialize, Serialize, utoipa::ToSchema, PartialEq, Eq)]
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
