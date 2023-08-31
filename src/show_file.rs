use std::io::SeekFrom;
use std::process::Stdio;
use std::time::Duration;
use std::{fs, path::PathBuf};

use axum::body::StreamBody;
use axum::extract::{FromRequest, Path, State};
use axum::headers::{ContentType, Range};
use axum::http::{HeaderName, HeaderValue, Request};
use axum::response::AppendHeaders;
use axum::{async_trait, TypedHeader};
use bytes::Bytes;
use reqwest::{header, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio_util::codec::{BytesCodec, FramedRead};

use crate::scan::{ProgressChunk, TaskType};
use crate::Library;
use crate::{get_metadata, process_file::FFprobeOutput};

pub struct FileExtractor(pub ShowFile);

#[derive(Debug, Deserialize, Clone)]
pub struct ShowParams {
    show_name: String,
    season: usize,
    episode: usize,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PreviewQuery {
    pub number: i32,
}

#[async_trait]
impl<S, B> FromRequest<S, B> for FileExtractor
where
    // these bounds are required by `async_trait`
    B: Send + 'static,
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request(req: Request<B>, _s: &S) -> Result<Self, Self::Rejection> {
        let state = req.extensions().get::<State<&'static Library>>();
        let path_params = req.extensions().get::<Path<ShowParams>>();

        if let (Some(state), Some(path_params)) = (state, path_params) {
            let file = state.items.iter().find(|item| {
                item.episode == path_params.episode as u8
                    && item.title == path_params.show_name.replace('-', " ")
                    && item.season == path_params.season as u8
            });
            if let Some(file) = file {
                return Ok(FileExtractor(file.clone()));
            } else {
                return Err(StatusCode::NOT_FOUND);
            }
        }
        return Err(StatusCode::BAD_REQUEST);
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowFile {
    pub title: String,
    pub episode: u8,
    pub season: u8,
    pub video_path: PathBuf,
    pub resources_path: PathBuf,
    pub metadata: FFprobeOutput,
}

impl ShowFile {
    pub fn new(path: PathBuf) -> Result<ShowFile, anyhow::Error> {
        let file_name = path.file_name().unwrap().to_str().unwrap();
        let mut is_spaced = false;
        if file_name.contains(" ") {
            is_spaced = true
        }
        let tokens = match is_spaced {
            true => file_name.split(" "),
            false => file_name.split("."),
        };
        let mut name: Option<String> = None;
        let mut season: Option<u8> = None;
        let mut episode: Option<u8> = None;
        for token in tokens.map(|x| x.to_string().to_lowercase()) {
            let chars: Vec<char> = token.chars().into_iter().collect();
            if token.len() == 6
                && chars[0] == 's'
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3] == 'e'
                && chars[4].is_ascii_digit()
                && chars[5].is_ascii_digit()
            {
                match (
                    Some(token.get(1..3).unwrap().parse().unwrap()),
                    Some(token.get(4..6).unwrap().parse().unwrap()),
                ) {
                    (Some(se), Some(ep)) => {
                        season = Some(se);
                        episode = Some(ep);
                        break;
                    }
                    _ => (),
                };
            }
            match name {
                Some(ref mut n) => n.push_str(&format!(" {}", token)),
                None => name = Some(token),
            }
        }
        if let (Some(name), Some(season), Some(episode)) = (name, season, episode) {
            let resource = generate_resources(&name, season, episode)?;
            let metadata = get_metadata(&path).unwrap();
            let show_file = ShowFile {
                title: name,
                episode,
                season,
                video_path: path,
                resources_path: resource,
                metadata,
            };
            Ok(show_file)
        } else {
            return Err(anyhow::Error::msg("Failed to build"));
        }
    }

    pub async fn get_subtitles(&self, lang: Option<String>) -> Option<String> {
        let mut subs_dir =
            tokio::fs::read_dir(format!("{}/subs", self.resources_path.to_str().unwrap()))
                .await
                .unwrap();
        let mut subs: Option<String> = None;
        while let Some(file) = subs_dir.next_entry().await.unwrap() {
            let file_path = file.path();
            let file_name = file_path.file_stem().unwrap().to_str().unwrap();

            subs = match &lang {
                Some(lang) => {
                    if file_name == lang {
                        Some(tokio::fs::read_to_string(file.path()).await.unwrap())
                    } else {
                        continue;
                    }
                }
                None => {
                    if &file_name == &"unknown" || &file_name == &"eng" {
                        Some(tokio::fs::read_to_string(file_path).await.unwrap())
                    } else {
                        continue;
                    }
                }
            };
        }
        subs
    }

    pub fn get_previews(&self) -> Result<Vec<Vec<u8>>, std::io::Error> {
        let previews_dir = fs::read_dir(format!(
            "{}/previews",
            self.resources_path.to_str().unwrap()
        ))?;
        let mut previews_vec = vec![];
        for file in previews_dir {
            let file = file.unwrap().path();
            if file.extension().unwrap() == "str" {
                previews_vec.push(fs::read(file).unwrap());
            }
        }
        Ok(previews_vec)
    }

    pub async fn generate_subtitles(
        &self,
        track: i32,
        language: &str,
        sender: Sender<ProgressChunk>,
    ) -> Result<(), anyhow::Error> {
        let args = vec![
            "-i".into(),
            self.video_path.to_str().unwrap().into(),
            "-map".into(),
            format!("0:{}", track),
            format!(
                "{}/subs/{}.srt",
                self.resources_path.to_str().unwrap(),
                language
            ),
            "-y".into(),
        ];

        self.run_command(args, TaskType::Subtitles, sender)
            .await
            .unwrap();

        Ok(())
    }

    pub async fn transcode_file(
        &self,
        audio_track: Option<i32>,
        transcode_video: bool,
        sender: Sender<ProgressChunk>,
    ) -> Result<(), anyhow::Error> {
        let buffer_path = format!("{}buffer", self.video_path.to_str().unwrap(),);
        tokio::fs::rename(&self.video_path, &buffer_path).await?;
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
        args.push(format!("{}", self.video_path.to_str().unwrap()));
        self.run_command(args, TaskType::Video, sender).await?;

        tokio::fs::remove_file(buffer_path).await?;
        Ok(())
    }

    pub async fn get_metadata(&self) -> Result<FFprobeOutput, anyhow::Error> {
        get_metadata(&self.video_path)
    }

    pub async fn generate_previews(
        &self,
        sender: Sender<ProgressChunk>,
    ) -> Result<(), anyhow::Error> {
        let args = vec![
            "-i".into(),
            self.video_path.to_str().unwrap().into(),
            "-vf".into(),
            "fps=1/10,scale=120:-1".into(),
            format!("{}/previews/%d.jpg", self.resources_path.to_str().unwrap()),
        ];
        self.run_command(args, TaskType::Preview, sender)
            .await
            .unwrap();

        Ok(())
    }

    async fn run_command(
        &self,
        args: Vec<String>,
        task_type: TaskType,
        sender: Sender<ProgressChunk>,
    ) -> Result<(), anyhow::Error> {
        let overall_duration = self.metadata.format.duration.clone();
        let video_path = self.video_path.clone();
        let mut cmd = Command::new("ffmpeg")
            .args(args)
            .args(["-progress", "pipe:1", "-nostats"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("process to spawn");
        let out = cmd.stdout.take().unwrap();
        let reader = BufReader::new(out);
        let mut lines_stream = reader.lines();
        while let Some(line) = lines_stream.next_line().await.unwrap() {
            let (key, value) = line.trim().split_once('=').expect("key=value output");
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
                    // sometimes have wierd space in front of first number
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
                            video_path: video_path.clone(),
                            percent,
                        })
                        .await
                        .unwrap();
                }
                _ => {}
            }
        }
        cmd.wait().await.unwrap();
        sender
            .send(ProgressChunk {
                task_type,
                video_path: video_path.clone(),
                percent: 100,
            })
            .await
            .unwrap();
        return Ok(());
    }

    pub async fn serve_previews(
        &self,
        number: i32,
    ) -> Result<(TypedHeader<ContentType>, Bytes), StatusCode> {
        let path = PathBuf::from(format!(
            "{}/previews",
            self.resources_path.to_str().unwrap(),
        ));
        let mut previews_dir = tokio::fs::read_dir(path).await.unwrap();

        while let Some(file) = previews_dir.next_entry().await.unwrap() {
            let file_path = file.path();
            let file_number: i32 = file_path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .parse()
                .expect("file to contain only numbers");
            if file_number == number {
                let bytes: Bytes = tokio::fs::read(file_path).await.unwrap().into();
                return Ok((TypedHeader(ContentType::jpeg()), bytes));
            }
        }
        return Err(StatusCode::NO_CONTENT);
    }

    pub async fn serve_video(
        &self,
        range: Range,
    ) -> (
        StatusCode,
        AppendHeaders<[(HeaderName, HeaderValue); 6]>,
        StreamBody<FramedRead<tokio::io::Take<File>, BytesCodec>>,
    ) {
        let mut file = tokio::fs::File::open(&self.video_path).await.unwrap();
        let file_size = file.metadata().await.unwrap().len();
        let (start, end) = range.iter().next().expect("at least one tuple");
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

        let chunk_size = end - start + 1;
        file.seek(SeekFrom::Start(start)).await.unwrap();
        let stream_of_bytes = FramedRead::new(file.take(chunk_size), BytesCodec::new());

        return (
            StatusCode::PARTIAL_CONTENT,
            AppendHeaders([
                (
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end - 1, file_size))
                        .unwrap(),
                ),
                (header::CONTENT_LENGTH, HeaderValue::from(file_size)),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_str("public, max-age=0").unwrap(),
                ),
                (
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_str("*").unwrap(),
                ),
                (
                    header::ACCEPT_RANGES,
                    HeaderValue::from_str("bytes").unwrap(),
                ),
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_str("video/x-matroska").unwrap(),
                ),
            ]),
            stream_of_bytes.into(),
        );
    }
}

fn generate_resources(title: &str, season: u8, episode: u8) -> Result<PathBuf, std::io::Error> {
    let episode_dir_path = format!(
        "{}/{}/{}/{}",
        std::env::var("RESOURCES_PATH").unwrap(),
        title,
        season,
        episode
    );
    fs::create_dir_all(format!("{}/subs", &episode_dir_path))?;
    fs::create_dir_all(format!("{}/previews", &episode_dir_path))?;
    let folder = PathBuf::from(episode_dir_path);
    Ok(folder)
}
