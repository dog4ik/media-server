use std::{
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use axum::{
    extract::{Path as AxumPath, State},
    http::Request,
    response::IntoResponse,
};
use reqwest::StatusCode;
use serde::Serialize;
use tokio::{
    process::{Child, Command},
    sync::Mutex,
};

use crate::{
    db::DbVideo,
    movie_file::{MovieFile, MovieParams},
    process_file::{AudioCodec, FFmpegJob, FFprobeOutput, VideoCodec},
    scan::Library,
    serve_content::ServeContent,
    show_file::{ShowFile, ShowParams}, utils,
};

#[derive(Debug, serde::Deserialize, Clone)]
pub struct PreviewQuery {
    pub number: i32,
}

#[derive(Debug, Clone)]
pub enum LibraryFile {
    Show(ShowFile),
    Movie(MovieFile),
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
    pub fn get_all_folders(&self) -> Vec<&PathBuf> {
        let mut out = Vec::with_capacity(self.shows.len() + self.movies.len());
        out.extend(self.shows.iter());
        out.extend(self.movies.iter());
        out
    }

    pub fn get_folder_type(&self, path: &PathBuf) -> Option<MediaType> {
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

impl LibraryFile {
    pub async fn serve_video(&self, range: axum::headers::Range) -> impl IntoResponse {
        match self {
            LibraryFile::Show(s) => s.serve_video(range).await,
            LibraryFile::Movie(m) => m.serve_video(range).await,
        }
    }

    pub async fn serve_previews(&self, number: i32) -> impl IntoResponse {
        match self {
            LibraryFile::Show(s) => s.serve_previews(number).await,
            LibraryFile::Movie(m) => m.serve_previews(number).await,
        }
    }

    pub async fn serve_subs(&self, lang: Option<String>) -> impl IntoResponse {
        match self {
            LibraryFile::Show(s) => s.serve_subs(lang).await,
            LibraryFile::Movie(m) => m.serve_subs(lang).await,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Chapter {
    title: String,
    start_time: String,
}

pub struct LibraryFileExtractor(pub LibraryFile);

#[axum::async_trait]
impl<S, B> axum::extract::FromRequest<S, B> for LibraryFileExtractor
where
    // these bounds are required by `async_trait`
    B: Send + 'static,
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request(req: Request<B>, _s: &S) -> Result<Self, Self::Rejection> {
        let state = req.extensions().get::<State<Arc<Mutex<Library>>>>();
        let movie_path_params = req.extensions().get::<AxumPath<MovieParams>>();
        let show_path_params = req.extensions().get::<AxumPath<ShowParams>>();

        if let Some(state) = state {
            if let Some(path_params) = movie_path_params {
                let state = state.lock().await;
                let file = state
                    .movies
                    .iter()
                    .find(|item| item.title == path_params.movie_name.replace('-', " "));
                if let Some(file) = file {
                    return Ok(LibraryFileExtractor(LibraryFile::Movie(file.clone())));
                }
            }

            if let Some(path_params) = show_path_params {
                let state = state.lock().await;
                let file = state.shows.iter().find(|item| {
                    item.episode == path_params.episode as u8
                        && item.title == path_params.show_name.replace('-', " ")
                        && item.season == path_params.season as u8
                });
                if let Some(file) = file {
                    return Ok(LibraryFileExtractor(LibraryFile::Show(file.clone())));
                }
            }

            return Err(StatusCode::NOT_FOUND);
        }
        return Err(StatusCode::BAD_REQUEST);
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum VideoId {
    Movie(i32),
    Episode(i32),
}

/// Trait that must be implemented for all library items
pub trait LibraryItem {
    /// Resources folder path
    fn resources_path(&self) -> &Path;

    /// Source file folder path
    fn source_path(&self) -> &Path;

    /// Get file metadata
    fn metadata(&self) -> &FFprobeOutput;

    /// Url part of file
    fn url(&self) -> String;

    /// Construct self from path
    fn from_path(path: PathBuf) -> Result<Self, anyhow::Error>
    where
        Self: Sized;

    /// Title
    fn title(&self) -> String;

    /// Calculate hash for the video
    fn calculate_video_hash(&self) -> Result<u32, std::io::Error> {
        let path = self.source_path();
        let mut file = std::fs::File::open(path)?;
        let hash = utils::file_hash(&mut file)?;
        return Ok(hash);
    }

    /// Returns struct compatible with database Video table
    fn into_db_video(&self) -> DbVideo {
        let metadata = self.metadata();
        let now = time::OffsetDateTime::now_utc();
        let hash = self.calulate_hash().unwrap();

        DbVideo {
            id: None,
            path: self.source_path().to_str().unwrap().to_string(),
            hash: hash.to_string(),
            local_title: self.title(),
            size: self.get_file_size() as i64,
            duration: metadata.format.duration.parse::<f64>().unwrap() as i64,
            audio_codec: metadata.default_audio().map(|c| c.codec_name.to_string()),
            video_codec: metadata.default_video().map(|c| c.codec_name.to_string()),
            scan_date: now.to_string(),
        }
    }

    /// Chapters
    fn chapters(&self) -> Vec<Chapter> {
        self.metadata()
            .chapters
            .iter()
            .map(|ffprobe_chapter| Chapter {
                title: ffprobe_chapter.tags.title.clone(),
                start_time: ffprobe_chapter.start_time.clone(),
            })
            .collect()
    }

    /// Get source file size in bytes
    fn get_file_size(&self) -> u64 {
        std::fs::metadata(self.source_path()).expect("exist").len()
    }

    /// Previews folder path
    fn previews_path(&self) -> PathBuf {
        self.resources_path().join("previews")
    }

    /// Subtitles forder path
    fn subtitles_path(&self) -> PathBuf {
        self.resources_path().join("subs")
    }

    /// Get prewies count
    fn previews_count(&self) -> usize {
        return std::fs::read_dir(self.previews_path()).unwrap().count();
    }

    /// Clean resources
    fn delete_resources(&self) -> Result<(), std::io::Error> {
        std::fs::remove_dir_all(self.resources_path())
    }

    /// Get video duration
    fn get_duration(&self) -> Duration {
        std::time::Duration::from_secs(
            self.metadata()
                .format
                .duration
                .parse::<f64>()
                .expect("duration to look like 123.1233")
                .round() as u64,
        )
    }

    // Get subtitles list
    fn get_subs(&self) -> Vec<String> {
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

    /// Run ffmpeg command. Returns handle to transconding process and percent progress Receiver channel
    fn run_command(&self, args: Vec<String>) -> Child {
        let cmd = Command::new("ffmpeg")
            .kill_on_drop(true)
            .args(["-progress", "pipe:1", "-nostats"])
            .args(args.clone())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("process to spawn");
        return cmd;
    }

    /// Generate previews for file
    fn generate_previews(&self) -> Child {
        let args = vec![
            "-i".into(),
            self.source_path().to_str().unwrap().into(),
            "-vf".into(),
            "fps=1/10,scale=120:-1".into(),
            format!("{}/%d.jpg", self.previews_path().to_str().unwrap()),
        ];
        self.run_command(args)
    }

    /// Generate subtitles for file
    fn generate_subtitles(&self, track: i32, language: &str) -> Child {
        let args = vec![
            "-i".into(),
            self.source_path().to_str().unwrap().into(),
            "-map".into(),
            format!("0:{}", track),
            format!(
                "{}/{}.srt",
                self.subtitles_path().to_str().unwrap(),
                language
            ),
            "-y".into(),
        ];

        self.run_command(args)
    }

    /// Transcode file for browser compatability
    fn transcode_video(
        &self,
        video: Option<VideoCodec>,
        audio: Option<(usize, AudioCodec)>,
    ) -> Result<FFmpegJob, anyhow::Error> {
        let buffer_path = format!("{}buffer", self.source_path().to_str().unwrap(),);
        std::fs::rename(&self.source_path(), &buffer_path)?;
        let mut args = Vec::new();
        args.push("-i".into());
        args.push(buffer_path);
        args.push("-map".into());
        args.push("0:v:0".into());
        if let Some((audio_track, audio_codec)) = audio {
            args.push("-map".into());
            args.push(format!("0:{}", audio_track));
            args.push("-c:a".into());
            args.push(audio_codec.to_string());
        } else {
            args.push("-c:a".into());
            args.push("copy".into());
        }
        args.push("-c:v".into());
        if let Some(video_codec) = video {
            args.push(video_codec.to_string());
        } else {
            args.push("copy".into());
        }
        args.push("-c:s".into());
        args.push("copy".into());
        args.push(format!("{}", self.source_path().to_str().unwrap()));
        let child = self.run_command(args);
        let job = FFmpegJob::new(child, self.get_duration(), self.source_path().into());
        return Ok(job);
    }
}

impl LibraryItem for MovieFile {
    fn resources_path(&self) -> &Path {
        &self.resources_path
    }
    fn source_path(&self) -> &Path {
        &self.video_path
    }
    fn metadata(&self) -> &FFprobeOutput {
        &self.metadata
    }
    fn title(&self) -> String {
        self.title.clone()
    }
    fn url(&self) -> String {
        format!("/{}", self.title)
    }

    fn from_path(path: PathBuf) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Self::new(path)
    }
}

impl LibraryItem for ShowFile {
    fn resources_path(&self) -> &Path {
        &self.resources_path
    }
    fn source_path(&self) -> &Path {
        &self.video_path
    }
    fn metadata(&self) -> &FFprobeOutput {
        &self.metadata
    }
    fn title(&self) -> String {
        self.title.clone()
    }
    fn url(&self) -> String {
        format!("/{}/{}/{}", self.title, self.season, self.episode)
    }

    fn from_path(path: PathBuf) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Self::new(path)
    }
}
