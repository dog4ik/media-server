use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::fmt::Display;
use std::process::{Command, ExitStatus};
use std::str::FromStr;
use std::time::Duration;
use std::{path::PathBuf, str::from_utf8};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc;
use tracing::trace;

use crate::progress::TaskKind;

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CodecType {
    Video,
    Audio,
    Subtitle,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeStream {
    pub index: i32,
    pub codec_name: String,
    pub codec_long_name: String,
    pub codec_type: String,
    pub codec_tag_string: String,
    pub codec_tag: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub coded_width: Option<i32>,
    pub coded_height: Option<i32>,
    pub sample_aspect_ratio: Option<String>,
    pub display_aspect_ratio: Option<String>,
    pub id: Option<String>,
    pub start_time: Option<String>,
    pub duration_ts: Option<i64>,
    pub duration: Option<String>,
    pub bit_rate: Option<String>,
    pub disposition: FFprobeDisposition,
    pub tags: Option<FFprobeTags>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeFormat {
    pub duration: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeTags {
    pub language: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeChapterTags {
    pub title: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeChapter {
    pub id: isize,
    pub time_base: String,
    pub start: isize,
    pub start_time: String,
    pub end: isize,
    pub end_time: String,
    pub tags: FFprobeChapterTags,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeDisposition {
    pub default: i32,
    pub dub: i32,
    pub original: i32,
    pub comment: i32,
    pub lyrics: i32,
    pub karaoke: i32,
    pub forced: i32,
    pub hearing_impaired: i32,
    pub visual_impaired: i32,
    pub clean_effects: i32,
    pub attached_pic: i32,
    pub timed_thumbnails: i32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeOutput {
    pub streams: Vec<FFprobeStream>,
    pub format: FFprobeFormat,
    pub chapters: Vec<FFprobeChapter>,
}

impl FFprobeOutput {
    pub fn video_streams(&self) -> Vec<&FFprobeStream> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "video")
            .collect()
    }

    pub fn audio_streams(&self) -> Vec<&FFprobeStream> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "audio")
            .collect()
    }

    pub fn subtitle_streams(&self) -> Vec<&FFprobeStream> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "subtitle")
            .collect()
    }

    /// Default audio stream
    pub fn default_audio(&self) -> Option<&FFprobeStream> {
        self.audio_streams()
            .into_iter()
            .find(|v| v.disposition.default == 1)
    }

    /// Default video stream
    pub fn default_video(&self) -> Option<&FFprobeStream> {
        self.video_streams()
            .into_iter()
            .find(|v| v.disposition.default == 1)
    }
}

impl FFprobeStream {
    pub fn audio_codec(&self) -> AudioCodec {
        AudioCodec::from_str(&self.codec_name).expect("any string to be valid")
    }

    pub fn video_codec(&self) -> VideoCodec {
        VideoCodec::from_str(&self.codec_name).expect("any string to be valid")
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub enum AudioCodec {
    AAC,
    AC3,
    Other(String),
}

impl Display for AudioCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioCodec::AAC => write!(f, "aac"),
            AudioCodec::AC3 => write!(f, "ac3"),
            AudioCodec::Other(codec) => write!(f, "{codec}"),
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

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
pub enum VideoCodec {
    Hevc,
    H264,
    Other(String),
}

impl Display for VideoCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoCodec::Hevc => write!(f, "hevc"),
            VideoCodec::H264 => write!(f, "h264"),
            VideoCodec::Other(codec) => write!(f, "{codec}"),
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

pub fn get_metadata(path: &PathBuf) -> Result<FFprobeOutput, anyhow::Error> {
    trace!(
        "Getting metadata for a file: {}",
        path.iter().last().unwrap().to_str().unwrap()
    );
    let output = Command::new("ffprobe")
        .args(&[
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-print_format",
            "json",
            "-show_streams",
            "-show_chapters",
            path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let output = from_utf8(&output.stdout)?;
    let metadata: FFprobeOutput = serde_json::from_str(output)?;
    Ok(metadata)
}

#[derive(Debug)]
pub struct FFmpegJob {
    process: Child,
    duration: Duration,
    pub target: PathBuf,
}

impl FFmpegJob {
    pub fn new(child: Child, duration: Duration, target: PathBuf) -> Self {
        Self {
            process: child,
            duration,
            target,
        }
    }

    pub async fn kill(mut self) {
        if let Err(_) = self.process.kill().await {
            tracing::error!("Failed to kill ffmpeg job")
        };
    }

    pub async fn wait(mut self) -> Result<ExitStatus, std::io::Error> {
        self.process.wait().await
    }

    pub fn progress(&mut self) -> mpsc::Receiver<usize> {
        let (tx, rx) = mpsc::channel(100);
        let out = self.process.stdout.take().expect("ffmpeg have stdout");
        let reader = BufReader::new(out);
        let mut lines = reader.lines();
        let duration = self.duration.clone();
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
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
                            Duration::from_micros(value.parse().unwrap()).as_secs();
                        let percent =
                            (current_duration as f64 / duration.as_secs() as f64) as f64 * 100.0;
                        let percent = percent.floor() as usize;
                        if percent == 100 {
                            break;
                        }
                        let _ = tx.send(percent).await;
                    }
                    _ => {}
                }
            }
            let _ = tx.send(100).await;
        });
        return rx;
    }
}

#[tokio::test]
async fn cancel_transcode() {
    use crate::library::LibraryItem;
    use crate::process_file::VideoCodec;
    use crate::progress::{TaskKind, TaskResource};
    use crate::testing::TestResource;
    use std::time::Duration;
    use tokio::fs;
    use tokio::sync::oneshot;
    use tokio::time;

    let testing_resource = TestResource::new();
    let subject = testing_resource.test_show.clone();
    let task_resource = TaskResource::new();
    let size_before = fs::metadata(&subject.video_path).await.unwrap().len();
    let video_path = subject.source_path().to_path_buf();
    let (tx, rx) = oneshot::channel();
    let task_id = task_resource
        .add_new_task(video_path.to_path_buf(), TaskKind::Transcode, Some(tx))
        .await
        .unwrap();
    let mut process = subject
        .transcode_video(Some(VideoCodec::H264), None)
        .unwrap();
    {
        let task_resource = task_resource.clone();
        let task_id = task_id.clone();
        tokio::spawn(async move {
            time::sleep(Duration::from_secs(2)).await;
            task_resource.cancel_task(task_id).await.unwrap();
        });
    }
    let original_buffer = format!("{}buffer", video_path.to_str().unwrap());
    tokio::select! {
        _ = process.wait() => {},
        _ = rx => {
            process.kill().await;
            task_resource.remove_task(task_id).await;
            fs::remove_file(&video_path).await.unwrap();
            fs::rename(&original_buffer, &video_path).await.unwrap();
        }
    }

    let size_after = fs::metadata(&video_path).await.unwrap().len();
    let is_cleaned = !fs::try_exists(original_buffer).await.unwrap_or(false);
    assert_eq!(size_before, size_after);
    assert!(is_cleaned);
}

#[tokio::test]
async fn progress_going() {
    todo!()
}
