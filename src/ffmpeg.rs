use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::str::FromStr;
use std::time::Duration;
use std::{path::Path, str::from_utf8};

use base64::engine::general_purpose;
use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout};
use tokio::sync::Semaphore;

use crate::config::APP_RESOURCES;
use crate::library::{
    AudioCodec, Resolution, Source, SubtitlesCodec, TranscodePayload, Video, VideoCodec,
};
use crate::utils;
use anyhow::{anyhow, Context};

const FFMPEG_IMAGE_CODECS: [&str; 6] = ["png", "jpeg", "mjpeg", "gif", "tiff", "bmp"];

/// General track stream provided by FFprobe
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeStream {
    pub index: i32,
    pub codec_name: String,
    pub codec_long_name: String,
    pub profile: Option<String>,
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
    pub level: Option<i32>,
    pub id: Option<String>,
    pub avg_frame_rate: Option<String>,
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
    pub profile: &'a str,
    pub display_aspect_ratio: &'a str,
    pub level: i32,
    pub avg_frame_rate: &'a str,
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
    pub profile: Option<&'a str>,
    pub sample_rate: &'a str,
    pub bit_rate: Option<&'a str>,
    pub disposition: &'a FFprobeDisposition,
}

#[derive(Debug, Serialize, Clone)]
pub struct FFprobeSubtitleStream<'a> {
    pub index: i32,
    pub codec_name: &'a str,
    pub codec_long_name: &'a str,
    pub disposition: &'a FFprobeDisposition,
    pub language: Option<&'a str>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeFormat {
    pub duration: String,
    pub format_name: String,
    pub bit_rate: String,
    pub tags: FormatTags,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FormatTags {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeTags {
    pub language: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeChapterTags {
    pub title: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FFprobeChapter {
    pub id: isize,
    pub time_base: String,
    pub start: isize,
    pub start_time: String,
    pub end: isize,
    pub end_time: String,
    pub tags: Option<FFprobeChapterTags>,
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

    pub fn is_default(&self) -> bool {
        self.disposition.default == 1
    }
}

impl<'a> FFprobeVideoStream<'a> {
    pub fn codec(&self) -> VideoCodec {
        VideoCodec::from_str(self.codec_name).expect("video stream codec")
    }

    pub fn resoultion(&self) -> Resolution {
        (self.width as usize, self.height as usize).into()
    }

    pub fn is_default(&self) -> bool {
        self.disposition.default == 1
    }

    pub fn framerate(&self) -> f64 {
        let (frames, ms): (f64, f64) = self
            .avg_frame_rate
            .split_once('/')
            .map(|(frames, ms)| (frames.parse().unwrap(), ms.parse().unwrap()))
            .expect("look like 24000/1001");
        frames / ms
    }
}

impl<'a> FFprobeSubtitleStream<'a> {
    pub fn codec(&self) -> SubtitlesCodec {
        SubtitlesCodec::from_str(self.codec_name).expect("subtitles stream codec")
    }

    pub fn is_defalut(&self) -> bool {
        self.disposition.default == 1
    }
}

impl FFprobeOutput {
    pub fn video_streams(&self) -> Vec<FFprobeVideoStream> {
        self.streams
            .iter()
            .filter(|s| {
                s.codec_type == "video" && !FFMPEG_IMAGE_CODECS.contains(&s.codec_name.as_str())
            })
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
        self.audio_streams().into_iter().find(|a| a.is_default())
    }

    /// Default video stream
    pub fn default_video(&self) -> Option<FFprobeVideoStream> {
        self.video_streams().into_iter().find(|v| v.is_default())
    }

    /// Default subtitles stream
    pub fn default_subtitles(&self) -> Option<FFprobeSubtitleStream> {
        self.subtitle_streams().into_iter().find(|s| s.is_defalut())
    }

    /// Video resoultion
    pub fn resolution(&self) -> Option<Resolution> {
        self.default_video().map(|v| v.resoultion())
    }

    /// Video bitrate
    pub fn bitrate(&self) -> usize {
        self.format.bit_rate.parse().expect("bitrate to be number")
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

    /// Get mime type
    pub fn guess_mime(&self) -> &'static str {
        let format_name = &self.format.format_name;
        match format_name.as_str() {
            "matroska,webm" => "video/x-matroska",
            "mov,mp4,m4a,3gp,3g2,mj2" => "video/mp4",
            _ => "video/x-matroska",
        }
    }
}

impl FFprobeStream {
    pub fn audio_stream(&self) -> Result<FFprobeAudioStream<'_>, anyhow::Error> {
        Ok(FFprobeAudioStream {
            index: self.index,
            codec_name: &self.codec_name,
            codec_long_name: &self.codec_long_name,
            bit_rate: self.bit_rate.as_deref(),
            channels: *self
                .channels
                .as_ref()
                .ok_or(anyhow!("channels are absent"))?,
            profile: self.profile.as_deref(),
            sample_rate: self
                .sample_rate
                .as_ref()
                .ok_or(anyhow!("sample rate is absent"))?,
            disposition: &self.disposition,
        })
    }

    pub fn video_stream(&self) -> Result<FFprobeVideoStream<'_>, anyhow::Error> {
        let video = FFprobeVideoStream {
            index: self.index,
            codec_name: &self.codec_name,
            codec_long_name: &self.codec_long_name,
            profile: self.profile.as_ref().ok_or(anyhow!("profile is absent"))?,
            level: self.level.ok_or(anyhow!("level is absent"))?,
            avg_frame_rate: self
                .avg_frame_rate
                .as_ref()
                .ok_or(anyhow!("avg_frame_rate is absent"))?,
            display_aspect_ratio: self
                .display_aspect_ratio
                .as_ref()
                .ok_or(anyhow!("aspect ratio is absent"))?,
            width: self.width.ok_or(anyhow!("width is absent"))?,
            height: self.height.ok_or(anyhow!("height is absent"))?,
            disposition: &self.disposition,
        };
        Ok(video)
    }

    pub fn subtitles_stream(&self) -> Result<FFprobeSubtitleStream<'_>, anyhow::Error> {
        let tags = &self.tags.as_ref().ok_or(anyhow!("tags are absent"))?;
        let video = FFprobeSubtitleStream {
            index: self.index,
            codec_name: &self.codec_name,
            codec_long_name: &self.codec_long_name,
            language: tags.language.as_deref(),
            disposition: &self.disposition,
        };
        Ok(video)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum H264Preset {
    Ultrafast,
    Superfast,
    Veryfast,
    Faster,
    Fast,
    #[default]
    Medium,
    Slow,
    Slower,
    Veryslow,
    Placebo,
}

impl FromStr for H264Preset {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ultrafast" => Ok(H264Preset::Ultrafast),
            "superfast" => Ok(H264Preset::Superfast),
            "veryfast" => Ok(H264Preset::Veryfast),
            "faster" => Ok(H264Preset::Faster),
            "fast" => Ok(H264Preset::Fast),
            "medium" => Ok(H264Preset::Medium),
            "slow" => Ok(H264Preset::Slow),
            "slower" => Ok(H264Preset::Slower),
            "veryslow" => Ok(H264Preset::Veryslow),
            "placebo" => Ok(H264Preset::Placebo),
            _ => Err(anyhow::anyhow!("{} is not valid h264 preset", s)),
        }
    }
}

pub async fn get_metadata(path: impl AsRef<Path>) -> Result<FFprobeOutput, anyhow::Error> {
    use tokio::process::Command;
    let path = path.as_ref();
    tracing::trace!(
        "Getting metadata for a file: {}",
        path.iter().last().unwrap().to_str().unwrap()
    );
    let ffprobe = &APP_RESOURCES
        .get()
        .unwrap()
        .ffprobe_path
        .as_ref()
        .ok_or(anyhow!("ffprobe is missing in resources"))?;
    let output = Command::new(ffprobe)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-show_chapters",
            "-show_format",
            path.to_str().unwrap(),
        ])
        .output()
        .await
        .unwrap();
    let output = from_utf8(&output.stdout)?;
    let metadata: FFprobeOutput = serde_json::from_str(output)?;
    Ok(metadata)
}

#[derive(Debug, Clone)]
pub enum JobType {
    Previews,
    Transcode,
    Subtitles,
    ImageResize,
}

pub trait FFmpegTask {
    fn args(&self) -> Vec<String>;
    #[allow(async_fn_in_trait)]
    async fn cancel(self) -> Result<(), anyhow::Error>;
    fn run(self, source_path: PathBuf, duration: Duration) -> anyhow::Result<FFmpegRunningJob<Self>>
    where
        Self: Sized,
    {
        FFmpegRunningJob::new(self, source_path, duration)
    }
}

#[derive(Debug)]
pub struct PreviewsJob {
    output_folder: PathBuf,
    source_path: PathBuf,
}

impl PreviewsJob {
    pub fn new(source_path: impl AsRef<Path>, output_folder: PathBuf) -> Self {
        Self {
            output_folder,
            source_path: source_path.as_ref().to_path_buf(),
        }
    }
}

impl FFmpegTask for PreviewsJob {
    fn args(&self) -> Vec<String> {
        vec![
            "-i".into(),
            self.source_path.to_string_lossy().to_string(),
            "-vf".into(),
            "fps=1/10,scale=120:-1".into(),
            format!(
                "{}{}%d.jpg",
                self.output_folder.to_string_lossy().to_string(),
                std::path::MAIN_SEPARATOR
            ),
        ]
    }

    async fn cancel(self) -> Result<(), anyhow::Error> {
        utils::clear_directory(self.output_folder).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct SubtitlesJob {
    track: usize,
    source_path: PathBuf,
    pub output_file_path: PathBuf,
}

impl SubtitlesJob {
    pub fn from_source(
        input: &Video,
        output_dir: impl AsRef<Path>,
        track: i32,
    ) -> Result<Self, anyhow::Error> {
        let output_path = |lang: Option<String>| {
            let file_name = lang.unwrap_or(uuid::Uuid::new_v4().to_string());
            output_dir.as_ref().join(
                PathBuf::new()
                    .with_file_name(file_name)
                    .with_extension("srt"),
            )
        };

        input
            .subtitle_streams()
            .iter()
            .find(|s| s.index == track && s.codec().supports_text())
            .map(|s| Self {
                source_path: input.path().to_path_buf(),
                track: s.index as usize,
                output_file_path: output_path(s.language.map(|x| x.to_string())),
            })
            .ok_or(anyhow::anyhow!("cant find track in file"))
    }

    pub fn new(source_path: PathBuf, output_file: PathBuf, track: usize) -> Self {
        Self {
            source_path,
            track,
            output_file_path: output_file,
        }
    }
}

impl FFmpegTask for SubtitlesJob {
    fn args(&self) -> Vec<String> {
        let args = vec![
            "-i".into(),
            self.source_path.to_string_lossy().to_string(),
            "-map".into(),
            format!("0:{}", self.track),
            self.output_file_path.to_string_lossy().to_string(),
            "-c:s".into(),
            "copy".into(),
            "-y".into(),
        ];
        args
    }

    async fn cancel(self) -> Result<(), anyhow::Error> {
        use tokio::fs;
        fs::remove_file(self.output_file_path).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct TranscodeJob {
    pub output_path: PathBuf,
    pub source_path: PathBuf,
    payload: TranscodePayload,
    hw_accel: bool,
}

impl TranscodeJob {
    pub fn from_source(
        source: &Source,
        output: impl AsRef<Path>,
        payload: TranscodePayload,
        hw_accel: bool,
    ) -> Result<Self, anyhow::Error> {
        let source_path = source.video.path().to_path_buf();
        let extention = source_path.extension().context("file extension")?;

        let file_name = PathBuf::new()
            .with_file_name(uuid::Uuid::new_v4().to_string())
            .with_extension(extention);
        Ok(Self {
            source_path,
            payload,
            output_path: output.as_ref().join(file_name),
            hw_accel,
        })
    }

    pub fn new(
        input_path: PathBuf,
        output_path: PathBuf,
        payload: TranscodePayload,
        hw_accel: bool,
    ) -> Self {
        Self {
            output_path,
            source_path: input_path,
            payload,
            hw_accel,
        }
    }
}

impl FFmpegTask for TranscodeJob {
    fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if self.hw_accel {
            args.push("-hwaccel".into());
            args.push("auto".into());
        }
        args.push("-i".into());
        args.push(self.source_path.to_string_lossy().to_string());
        if let Some(audio_codec) = &self.payload.audio_codec {
            args.push("-c:a".into());
            args.push(audio_codec.to_string());
        } else {
            args.push("-c:a".into());
            args.push("copy".into());
        }
        args.push("-c:v".into());
        if let Some(video_codec) = &self.payload.video_codec {
            args.push(video_codec.to_string());
        } else {
            args.push("copy".into());
        }
        if let Some(resolution) = &self.payload.resolution {
            args.push("-s".into());
            args.push(resolution.to_string());
        }
        args.push("-c:s".into());
        args.push("copy".into());
        args.push(self.output_path.to_string_lossy().to_string());
        args
    }
    async fn cancel(self) -> Result<(), anyhow::Error> {
        use tokio::fs;
        fs::remove_file(self.output_path).await?;
        Ok(())
    }
}

// NOTE: cleanup callback? (after job is done)
#[derive(Debug)]
pub struct FFmpegRunningJob<T: FFmpegTask> {
    process: Child,
    pub target: PathBuf,
    pub job: T,
    duration: Duration,
}

impl<T: FFmpegTask> FFmpegRunningJob<T> {
    pub fn new(
        job: T,
        source_path: PathBuf,
        duration: Duration,
    ) -> anyhow::Result<FFmpegRunningJob<T>> {
        let process = Self::run(&job.args())?;
        Ok(Self {
            process,
            target: source_path,
            duration,
            job,
        })
    }

    /// Run ffmpeg command. Returns handle to process
    fn run<I, S>(args: I) -> anyhow::Result<Child>
    where
        I: IntoIterator<Item = S> + Copy + std::fmt::Debug,
        S: AsRef<std::ffi::OsStr>,
    {
        let ffmpeg = &APP_RESOURCES
            .get()
            .unwrap()
            .ffmpeg_path
            .as_ref()
            .ok_or(anyhow!("ffmpeg is missing in resources"))?;
        Ok(tokio::process::Command::new(ffmpeg)
            .kill_on_drop(true)
            .args(["-progress", "pipe:1", "-nostats"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?)
    }

    /// Kill the job
    pub async fn kill(&mut self) {
        if self.process.kill().await.is_err() {
            tracing::error!("Failed to kill ffmpeg job")
        };
    }

    /// Wait until process fully complete or terminated
    pub async fn wait(&mut self) -> Result<ExitStatus, std::io::Error> {
        self.process.wait().await
    }

    /// Kill task cleaning up garbage
    pub async fn cancel(mut self) -> Result<(), anyhow::Error> {
        self.kill().await;
        self.job.cancel().await?;
        Ok(())
    }

    /// Take child's stdout.
    pub fn take_stdout(&mut self) -> Option<FFmpegProgressStdout> {
        let stdout = self.process.stdout.take()?;
        Some(FFmpegProgressStdout::new(stdout))
    }

    pub fn target_duration(&self) -> Duration {
        self.duration
    }
}

#[derive(Debug)]
pub struct FFmpegProgressStdout {
    lines: Lines<BufReader<ChildStdout>>,
    time: Option<Duration>,
    speed: Option<f32>,
}

impl FFmpegProgressStdout {
    pub fn new(stdout: ChildStdout) -> Self {
        let lines = BufReader::new(stdout).lines();

        Self {
            lines,
            time: None,
            speed: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FFmpegProgress {
    /// Speed of operation relative to video playback
    speed: f32,
    /// Current time of the generated file
    time: Duration,
}

impl FFmpegProgress {
    /// Calculate percent of current point relative to given duration
    pub fn percent(&self, total_duration: Duration) -> usize {
        let current_duration = self.time.as_secs();
        let percent = (current_duration as f64 / total_duration.as_secs() as f64) * 100.0;

        percent.floor() as usize
    }

    /// Get speed of operation relative to the video playback
    pub fn relative_speed(&self) -> f32 {
        self.speed
    }
}

impl FFmpegProgressStdout {
    /// Yield next progress chunk.
    /// This method is cancellation safe.
    pub async fn next_progress_chunk(&mut self) -> Option<FFmpegProgress> {
        while let Ok(Some(line)) = self.lines.next_line().await {
            let (key, value) = line.trim().split_once('=').expect("output to be key=value");
            // example output chunk:
            // bitrate=5234.1kbits/s
            // total_size=2456901632
            // out_time_us=3755250000
            // out_time_ms=3755250000
            // out_time=01:02:35.250000
            // dup_frames=0
            // drop_frames=0
            // speed=28.6x
            // progress=continue

            match key {
                // The last key of a sequence of progress information is always "progress".
                // end | continue
                "progress" => {
                    if let (Some(time), Some(speed)) = (self.time, self.speed) {
                        (self.time, self.speed) = (None, None);
                        return Some(FFmpegProgress { speed, time });
                    } else {
                        tracing::warn!(
                            "Skipping incomplete progress: time: {:?}, speed: {:?}",
                            self.time,
                            self.speed
                        );
                        (self.time, self.speed) = (None, None);
                    }
                }
                // speed looks like `10.3x`
                // sometimes have space at the front
                "speed" => match value[..value.len() - 1].trim_start().parse() {
                    Ok(v) => self.speed = Some(v),
                    Err(e) => {
                        if value == "N/A" {
                            self.speed = Some(f32::default());
                        } else {
                            tracing::debug!("Failed to parse {key}={value} in ffmpeg progress: {e}")
                        }
                    }
                },
                // just a number, time in microseconds
                "out_time_ms" => match value.parse() {
                    Ok(v) => self.time = Some(Duration::from_micros(v)),
                    Err(e) => {
                        if value == "N/A" {
                            self.time = Some(Duration::default())
                        } else {
                            tracing::debug!("Failed to parse {key}={value} in ffmpeg progress: {e}")
                        }
                    }
                },
                _ => {}
            }
        }
        None
    }
}

/// Resize and base64 encode image using ffmpeg image2pipe format
pub async fn resize_image_ffmpeg(
    bytes: bytes::Bytes,
    width: i32,
    height: Option<i32>,
) -> Result<String, anyhow::Error> {
    let scale = format!("scale={}:{}", width, height.unwrap_or(-1));
    let ffmpeg = &APP_RESOURCES
        .get()
        .unwrap()
        .ffmpeg_path
        .as_ref()
        .ok_or(anyhow!("ffmpeg is missing in resources"))?;
    let mut child = tokio::process::Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            "-",
            "-vf",
            &scale,
            "-f",
            "image2pipe",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    {
        let mut stdin = child.stdin.take().ok_or(anyhow!("failed to take stdin"))?;
        stdin.write_all(&bytes).await?;
    }
    let output = child.wait_with_output().await?;
    if output.status.code().unwrap_or(1) == 0 {
        Ok(general_purpose::STANDARD_NO_PAD.encode(output.stdout))
    } else {
        Err(anyhow::anyhow!(
            "resize process was unexpectedly terminated"
        ))
    }
}

/// Extract subtitle track from provided file. Takes in desired track
pub async fn pull_subtitles(input_file: impl AsRef<Path>, track: i32) -> anyhow::Result<String> {
    let ffmpeg = &APP_RESOURCES
        .get()
        .unwrap()
        .ffmpeg_path
        .as_ref()
        .ok_or(anyhow!("ffmpeg is missing in resources"))?;
    let child = tokio::process::Command::new(ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &input_file.as_ref().to_string_lossy(),
            "-f",
            "srt",
            "-map",
            &format!("0:{}", track),
            "-vn",
            "-an",
            "-c:s",
            "text",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let output = child.wait_with_output().await?;
    if output.status.code().unwrap_or(1) == 0 {
        Ok(String::from_utf8(output.stdout).expect("ffmpeg output utf-8"))
    } else {
        Err(anyhow::anyhow!(
            "ffmpeg process was unexpectedly terminated"
        ))
    }
}

static PULL_FRAME_PERMITS: Semaphore = Semaphore::const_new(4);

/// Pull the frame at specified time location
pub async fn pull_frame(
    input_file: impl AsRef<Path>,
    output_file: impl AsRef<Path>,
    timing: Duration,
) -> anyhow::Result<()> {
    let _guard = PULL_FRAME_PERMITS.acquire().await.unwrap();
    let ffmpeg = &APP_RESOURCES
        .get()
        .unwrap()
        .ffmpeg_path
        .as_ref()
        .ok_or(anyhow!("ffmpeg is missing in resources"))?;
    let format_time = |duration: Duration| {
        let seconds = duration.as_secs();
        let minutes = seconds / 60;
        let hours = minutes / 60;
        format!("{:0>2}:{:0>2}:{:0>2}", hours, minutes % 60, seconds % 60)
    };
    let time = format_time(timing);
    let args: &[&OsStr] = &[
        "-hide_banner".as_ref(),
        "-loglevel".as_ref(),
        "error".as_ref(),
        "-ss".as_ref(),
        time.as_ref(),
        "-i".as_ref(),
        input_file.as_ref().as_os_str(),
        "-frames:v".as_ref(),
        "1".as_ref(),
        output_file.as_ref().as_os_str(),
        "-y".as_ref(),
    ];
    let mut child = tokio::process::Command::new(ffmpeg)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let status = child.wait().await?;
    if status.code().unwrap_or(1) == 0 {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "ffmpeg process was unexpectedly terminated"
        ))
    }
}

pub fn healthcheck_ffmpeg_command(path: impl AsRef<OsStr>) -> anyhow::Result<String> {
    let output = std::process::Command::new(&path)
        .args(["-version"])
        .output()?;
    if output.status.success() {
        parse_version(std::str::from_utf8(&output.stdout).expect("output to be valid utf-8"))
    } else {
        Err(anyhow::anyhow!(
            "{} errored with {} status code",
            path.as_ref().to_string_lossy(),
            output.status.code().expect("status code")
        ))
    }
}

fn parse_version(output: &str) -> anyhow::Result<String> {
    let first_line = output.lines().next().expect("at least one line");
    let version = first_line.split_ascii_whitespace().nth(2).unwrap();
    Ok(version.to_string())
}
