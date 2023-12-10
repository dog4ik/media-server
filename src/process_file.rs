use anyhow::anyhow;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::fmt::Display;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::str::FromStr;
use std::time::Duration;
use std::{path::PathBuf, str::from_utf8};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc;
use tracing::trace;

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CodecType {
    Video,
    Audio,
    Subtitle,
}

/// General track stream provided by FFprobe
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeStream {
    pub index: i32,
    pub codec_name: String,
    pub codec_long_name: String,
    pub codec_type: String,
    pub codec_tag_string: String,
    pub codec_tag: String,
    pub channels: Option<i32>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub coded_width: Option<i32>,
    pub coded_height: Option<i32>,
    pub sample_rate: Option<String>,
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

#[derive(Debug, Serialize, Clone)]
pub struct FFprobeVideoStream<'a> {
    pub index: i32,
    pub codec_name: &'a str,
    pub codec_long_name: &'a str,
    pub display_aspect_ratio: &'a str,
    pub width: i32,
    pub height: i32,
    pub disposition: &'a FFprobeDisposition,
}

#[derive(Debug, Serialize, Clone)]
pub struct FFprobeAudioStream<'a> {
    pub index: i32,
    pub codec_name: &'a str,
    pub codec_long_name: &'a str,
    pub channels: i32,
    pub sample_rate: &'a str,
    pub disposition: &'a FFprobeDisposition,
}

#[derive(Debug, Serialize, Clone)]
pub struct FFprobeSubtitleStream<'a> {
    pub index: i32,
    pub codec_name: &'a str,
    pub codec_long_name: &'a str,
    pub disposition: &'a FFprobeDisposition,
    pub language: &'a str,
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

impl<'a> FFprobeAudioStream<'a> {
    pub fn codec(&self) -> AudioCodec {
        AudioCodec::from_str(self.codec_name).expect("audio stream codec")
    }
}

impl<'a> FFprobeVideoStream<'a> {
    pub fn codec(&self) -> VideoCodec {
        VideoCodec::from_str(self.codec_name).expect("video stream codec")
    }

    pub fn resoultion(&self) -> Resolution {
        (self.width as usize, self.height as usize).into()
    }
}

impl<'a> FFprobeSubtitleStream<'a> {
    pub fn codec(&self) -> SubtitlesCodec {
        SubtitlesCodec::from_str(self.codec_name).expect("subtitles stream codec")
    }
}

impl FFprobeOutput {
    pub fn video_streams(&self) -> Vec<FFprobeVideoStream> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "video")
            .map(|s| s.video_stream().expect("video stream"))
            .collect()
    }

    pub fn audio_streams(&self) -> Vec<FFprobeAudioStream> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "audio")
            .map(|s| s.audio_stream().expect("audio stream"))
            .collect()
    }

    pub fn subtitle_streams(&self) -> Vec<FFprobeSubtitleStream> {
        self.streams
            .iter()
            .filter(|s| s.codec_type == "subtitle")
            .map(|s| s.subtitles_stream().expect("subtitles stream"))
            .collect()
    }

    /// Default audio stream
    pub fn default_audio(&self) -> Option<FFprobeAudioStream> {
        self.audio_streams()
            .into_iter()
            .find(|a| a.disposition.default == 1)
    }

    /// Default video stream
    pub fn default_video(&self) -> Option<FFprobeVideoStream> {
        self.video_streams()
            .into_iter()
            .find(|v| v.disposition.default == 1)
    }

    /// Default subtitles stream
    pub fn default_subtitles(&self) -> Option<FFprobeSubtitleStream> {
        self.subtitle_streams()
            .into_iter()
            .find(|s| s.disposition.default == 1)
    }

    /// Video resoultion
    pub fn resolution(&self) -> Option<Resolution> {
        self.default_video().map(|v| v.resoultion())
    }

    /// Duration
    pub fn duration(&self) -> Duration {
        std::time::Duration::from_secs(
            self.format
                .duration
                .parse::<f64>()
                .expect("duration to look like 123.1233")
                .round() as u64,
        )
    }
}

impl FFprobeStream {
    pub fn audio_stream<'a>(&'a self) -> Result<FFprobeAudioStream<'a>, anyhow::Error> {
        Ok(FFprobeAudioStream {
            index: self.index,
            codec_name: &self.codec_name,
            codec_long_name: &self.codec_long_name,
            channels: self.channels.ok_or(anyhow!("channels are absent"))?,
            sample_rate: &self
                .sample_rate
                .as_ref()
                .ok_or(anyhow!("sample rate is absent"))?,
            disposition: &self.disposition,
        })
    }

    pub fn video_stream<'a>(&'a self) -> Result<FFprobeVideoStream<'a>, anyhow::Error> {
        let video = FFprobeVideoStream {
            index: self.index,
            codec_name: &self.codec_name,
            codec_long_name: &self.codec_long_name,
            display_aspect_ratio: &self
                .display_aspect_ratio
                .as_ref()
                .ok_or(anyhow!("aspect ratio is absent"))?,
            width: self.width.ok_or(anyhow!("width is absent"))?,
            height: self.height.ok_or(anyhow!("height is absent"))?,
            disposition: &self.disposition,
        };
        return Ok(video);
    }

    pub fn subtitles_stream<'a>(&'a self) -> Result<FFprobeSubtitleStream<'a>, anyhow::Error> {
        let tags = &self.tags.as_ref().ok_or(anyhow!("tags are absent"))?;
        let video = FFprobeSubtitleStream {
            index: self.index,
            codec_name: &self.codec_name,
            codec_long_name: &self.codec_long_name,
            language: &tags
                .language
                .as_ref()
                .ok_or(anyhow!("language tag is absent"))?,
            disposition: &self.disposition,
        };
        return Ok(video);
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

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase", untagged)]
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

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "lowercase", untagged)]
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

#[derive(Clone, Debug, Deserialize)]
pub struct Resolution(pub (usize, usize));

impl Serialize for Resolution {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let (x, y) = self.0;
        serializer.serialize_str(&format!("{}x{}", x, y))
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
            .ok_or(anyhow!("str must be seperated with 'x'"))?;
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

pub fn get_metadata(path: impl AsRef<Path>) -> Result<FFprobeOutput, anyhow::Error> {
    let path = path.as_ref();
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

// NOTE: cleanup callback? (after job is done)
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

    pub async fn kill(&mut self) {
        if let Err(_) = self.process.kill().await {
            tracing::error!("Failed to kill ffmpeg job")
        };
    }

    pub async fn wait(&mut self) -> Result<ExitStatus, std::io::Error> {
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
    use crate::process_file::VideoCodec;
    use crate::progress::{TaskKind, TaskResource};
    use crate::testing::TestResource;
    use std::time::Duration;
    use tokio::fs;
    use tokio::sync::oneshot;
    use tokio::time;

    let testing_resource = TestResource::new(true);
    let subject = testing_resource.test_show.clone();
    let task_resource = TaskResource::new();
    let size_before = fs::metadata(&subject.source.origin.path)
        .await
        .unwrap()
        .len();
    let video_path = subject.source.source_path().to_path_buf();
    let (tx, rx) = oneshot::channel();
    let task_id = task_resource
        .add_new_task(video_path.to_path_buf(), TaskKind::Transcode, Some(tx))
        .await
        .unwrap();
    let payload = TranscodePayloadBuilder::new()
        .video_codec(VideoCodec::Hevc)
        .build();
    let mut process = subject.source.transcode_video(payload).unwrap();
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
            fs::rename(&original_buffer, &video_path).await.unwrap()
        },
    }

    let size_after = fs::metadata(&video_path).await.unwrap().len();
    let is_cleaned = !fs::try_exists(original_buffer).await.unwrap_or(false);
    assert_eq!(size_before, size_after);
    assert!(is_cleaned);
}

#[tokio::test]
async fn transcode_video() {
    use crate::library::LibraryItem;
    use crate::process_file::{AudioCodec, Resolution, VideoCodec};
    use crate::testing::TestResource;

    let testing_resource = TestResource::new(false);
    let subject = testing_resource.test_show.clone();
    let source = subject.source();
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
