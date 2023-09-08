use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::Sender,
    time::Duration,
};

use axum::{
    extract::{Path as AxumPath, State},
    http::Request,
};
use reqwest::StatusCode;

use crate::{
    movie_file::{MovieFile, MovieParams},
    process_file::FFprobeOutput,
    scan::{ProgressChunk, TaskType},
    show_file::ShowParams,
    Library, ShowFile,
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
        let state = req.extensions().get::<State<&'static Library>>();
        let movie_path_params = req.extensions().get::<AxumPath<MovieParams>>();
        let show_path_params = req.extensions().get::<AxumPath<ShowParams>>();

        if let Some(state) = state {
            if let Some(path_params) = movie_path_params {
                let file = state
                    .movies
                    .iter()
                    .find(|item| item.title == path_params.movie_name.replace('-', " "));
                if let Some(file) = file {
                    return Ok(LibraryFileExtractor(LibraryFile::Movie(file.clone())));
                }
            }

            if let Some(path_params) = show_path_params {
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

/// Resources folder path
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
    fn from_path(path: PathBuf) -> Self
    where
        Self: Sized;

    /// Title
    fn title(&self) -> String;

    /// Season
    fn season(&self) -> Option<u8> {
        None
    }

    /// Episode
    fn episode(&self) -> Option<u8> {
        None
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

    /// Run ffmpeg command
    fn run_command(
        &self,
        args: Vec<String>,
        task_type: TaskType,
        sender: Sender<ProgressChunk>,
    ) -> Result<(), anyhow::Error> {
        let overall_duration = self.metadata().format.duration.clone();
        let video_path = self.source_path();
        let mut cmd = Command::new("ffmpeg")
            .args(args)
            .args(["-progress", "pipe:1", "-nostats"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("process to spawn");
        let out = cmd.stdout.take().unwrap();
        let reader = BufReader::new(out);
        let mut lines = reader.lines();
        while let Ok(line) = lines.next().unwrap() {
            let (key, value) = line.trim().split_once('=').expect("output to be key=value");
            match key {
                "progress" => {
                    // end | continue
                    if value == "end" {
                        break;
                        // end logic is unhandled
                        // how do we handle channel close?
                    }
                }
                "speed" => {
                    // speed looks like 10x
                    // sometimes have wierd space in front
                }
                "out_time_ms" => {
                    // just a number
                    let current_duration =
                        Duration::from_micros(value.parse().expect("to parse")).as_secs();
                    let overall_duration = Duration::from_secs(
                        overall_duration.parse::<f64>().unwrap().floor() as u64,
                    )
                    .as_secs();
                    let percent =
                        (current_duration as f64 / overall_duration as f64) as f64 * 100.0;
                    let percent = percent.floor() as u32;
                    if percent == 100 {
                        break;
                    }
                    sender
                        .send(ProgressChunk {
                            task_type,
                            video_path: video_path.to_owned(),
                            percent,
                        })
                        .unwrap();
                }
                _ => {}
            }
        }
        cmd.wait().unwrap();
        sender
            .send(ProgressChunk {
                task_type,
                video_path: video_path.into(),
                percent: 100,
            })
            .unwrap();
        return Ok(());
    }

    /// Generate previews for file
    fn generate_previews(&self, sender: Sender<ProgressChunk>) -> Result<(), anyhow::Error> {
        let args = vec![
            "-i".into(),
            self.source_path().to_str().unwrap().into(),
            "-vf".into(),
            "fps=1/10,scale=120:-1".into(),
            format!("{}/%d.jpg", self.previews_path().to_str().unwrap()),
        ];
        self.run_command(args, TaskType::Preview, sender)
    }

    /// Generate subtitles for file
    fn generate_subtitles(
        &self,
        track: i32,
        language: &str,
        sender: Sender<ProgressChunk>,
    ) -> Result<(), anyhow::Error> {
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

        self.run_command(args, TaskType::Subtitles, sender)
    }

    /// Transcode file for browser compatability
    fn transcode_video(
        &self,
        audio_track: Option<i32>,
        transcode_video: bool,
        sender: Sender<ProgressChunk>,
    ) -> Result<(), anyhow::Error> {
        let buffer_path = format!("{}buffer", self.source_path().to_str().unwrap(),);
        std::fs::rename(&self.source_path(), &buffer_path)?;
        let mut args = Vec::new();
        args.push("-i".into());
        args.push(buffer_path.clone());
        args.push("-map".into());
        args.push("0:v:0".into());
        if let Some(track) = audio_track {
            args.push("-map".into());
            args.push(format!("0:{}", track));
            args.push("-c:a".into());
            args.push("aac".into());
        } else {
            args.push("-c:a".into());
            args.push("copy".into());
        }
        args.push("-c:v".into());
        if transcode_video {
            args.push("h264".into());
        } else {
            args.push("copy".into());
        }
        args.push("-c:s".into());
        args.push("copy".into());
        args.push(format!("{}", self.source_path().to_str().unwrap()));
        let result = self.run_command(args, TaskType::Video, sender);
        std::fs::remove_file(buffer_path)?;
        result
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

    fn from_path(path: PathBuf) -> Self
    where
        Self: Sized,
    {
        Self::new(path).unwrap()
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
    fn season(&self) -> Option<u8> {
        Some(self.season)
    }
    fn episode(&self) -> Option<u8> {
        Some(self.episode)
    }
    fn url(&self) -> String {
        format!("/{}/{}/{}", self.title, self.season, self.episode)
    }

    fn from_path(path: PathBuf) -> Self
    where
        Self: Sized,
    {
        Self::new(path).unwrap()
    }
}
