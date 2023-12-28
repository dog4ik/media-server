use std::io::Cursor;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::str::FromStr;
use std::time::Duration;
use std::{path::Path, str::from_utf8};

use base64::engine::general_purpose;
use base64::Engine;
use image::imageops::FilterType;
use image::GenericImageView;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;
use tokio::sync::mpsc;

use crate::library::{
    AudioCodec, Resolution, Source, SubtitlesCodec, TranscodePayload, VideoCodec,
};
use crate::utils;
use anyhow::anyhow;

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
    pub bit_rate: &'a str,
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
    pub format_name: String,
    pub bit_rate: String,
    pub tags: FormatTags,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct FormatTags {
    pub title: String,
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
    pub fn audio_stream<'a>(&'a self) -> Result<FFprobeAudioStream<'a>, anyhow::Error> {
        Ok(FFprobeAudioStream {
            index: self.index,
            codec_name: &self.codec_name,
            codec_long_name: &self.codec_long_name,
            bit_rate: self.bit_rate.as_ref().ok_or(anyhow!("bitrate is absent"))?,
            channels: *self
                .channels
                .as_ref()
                .ok_or(anyhow!("channels are absent"))?,
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
            profile: &self.profile.as_ref().ok_or(anyhow!("profile is absent"))?,
            level: self.level.ok_or(anyhow!("level is absent"))?,
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

pub fn get_metadata(path: impl AsRef<Path>) -> Result<FFprobeOutput, anyhow::Error> {
    let path = path.as_ref();
    tracing::trace!(
        "Getting metadata for a file: {}",
        path.iter().last().unwrap().to_str().unwrap()
    );
    let output = Command::new("ffprobe")
        .args(&[
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
}

#[derive(Debug)]
pub struct PreviewsJob {
    output_folder: PathBuf,
    source_path: PathBuf,
}

impl PreviewsJob {
    pub fn from_source(source: &Source) -> Self {
        Self {
            source_path: source.source_path().to_path_buf(),
            output_folder: source.previews_path(),
        }
    }
    pub fn new(source_path: PathBuf, output_folder: PathBuf) -> Self {
        Self {
            output_folder,
            source_path,
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
    output_file_path: PathBuf,
}

impl SubtitlesJob {
    pub fn from_source(input: &Source, track: usize) -> Result<Self, anyhow::Error> {
        let output_path = |lang: &str| {
            input
                .subtitles_path()
                .join(PathBuf::new().with_file_name(lang).with_extension("srt"))
        };

        input
            .origin
            .subtitle_streams()
            .iter()
            .find(|s| s.index == track as i32 && s.codec().supports_text())
            .map(|s| Self {
                source_path: input.source_path().to_path_buf(),
                track: s.index as usize,
                output_file_path: output_path(s.language),
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
        return args;
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
}

impl TranscodeJob {
    pub fn from_source(source: &Source, payload: TranscodePayload) -> Result<Self, anyhow::Error> {
        let source_path = source.source_path();
        let extention = source_path
            .extension()
            .ok_or(anyhow::anyhow!("extention missing"))?;

        let output_name = PathBuf::new()
            .with_file_name(uuid::Uuid::new_v4().to_string())
            .with_extension(extention);
        let output_path = source.variants_path().join(output_name);
        Ok(Self {
            source_path: source.source_path().to_path_buf(),
            payload,
            output_path,
        })
    }

    pub fn new(input_path: PathBuf, output_path: PathBuf, payload: TranscodePayload) -> Self {
        Self {
            output_path,
            source_path: input_path,
            payload,
        }
    }
}

impl FFmpegTask for TranscodeJob {
    fn args(&self) -> Vec<String> {
        let mut args = Vec::new();
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
        return args;
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
    pub fn new_running(job: T, source_path: PathBuf, duration: Duration) -> FFmpegRunningJob<T> {
        let process = Self::run(&job.args());
        Self {
            process,
            target: source_path,
            duration,
            job,
        }
    }

    /// Run ffmpeg command. Returns handle to process
    fn run<I, S>(args: I) -> Child
    where
        I: IntoIterator<Item = S> + Copy,
        S: AsRef<std::ffi::OsStr>,
    {
        tokio::process::Command::new("ffmpeg")
            .kill_on_drop(true)
            .args(["-progress", "pipe:1", "-nostats"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("process to spawn")
    }

    /// Kill the job
    pub async fn kill(&mut self) {
        if let Err(_) = self.process.kill().await {
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

    /// Channel with job progress in percents. Consumes stdout of process
    /// Returns `None` if stdout is empty
    pub fn progress(&mut self) -> mpsc::Receiver<usize> {
        let (tx, rx) = mpsc::channel(100);
        let out = self.process.stdout.take().expect("ffmpeg to have stdout");
        let reader = BufReader::new(out);
        let mut lines = reader.lines();
        let duration = self.duration;
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

/// Resize and base64 encode image using ffmpeg image2pipe format
pub async fn resize_image_ffmpeg(
    bytes: bytes::Bytes,
    width: i32,
    height: Option<i32>,
) -> Result<String, anyhow::Error> {
    let scale = format!("scale={}:{}", width, height.unwrap_or(-1));
    let mut child = tokio::process::Command::new("ffmpeg")
        .args(&[
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

pub fn resize_image_crate(
    bytes: Vec<u8>,
    width: u32,
    height: Option<u32>,
) -> Result<String, anyhow::Error> {
    let image = image::load_from_memory(&bytes)?;
    let (img_width, img_height) = image.dimensions();
    let img_aspect_ratio: f64 = img_width as f64 / img_height as f64;
    let resized_image = image.resize(
        width as u32,
        height.unwrap_or((width as f64 / img_aspect_ratio).floor() as u32),
        FilterType::Triangle,
    );
    let mut image_data = Vec::new();
    resized_image.write_to(
        &mut Cursor::new(&mut image_data),
        image::ImageOutputFormat::Jpeg(80),
    )?;
    Ok(general_purpose::STANDARD_NO_PAD.encode(image_data))
}

#[tokio::test]
async fn resize_image_ffmpeg_test() {
    use tokio::fs;
    let bytes = fs::read("test-dir/test.jpeg").await.unwrap();
    let base64 = resize_image_ffmpeg(bytes.into(), 16, None).await.unwrap();
    dbg!(&base64);
    assert!(base64.len() > 100);
}

#[tokio::test]
async fn resize_image_crate_test() {
    use tokio::fs;
    let bytes = fs::read("test-dir/test.jpeg").await.unwrap();
    let base64 = resize_image_crate(bytes, 16, None).unwrap();
    dbg!(&base64);
    assert!(base64.len() > 100);
}
