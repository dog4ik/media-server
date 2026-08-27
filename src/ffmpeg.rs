use std::ffi::OsStr;
use std::marker::PhantomData;
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout};
use tokio::sync::Semaphore;
use tokio_stream::StreamExt;

use crate::config::{self};
use crate::library::media::{
    Resolution,
    codec::{audio::AudioCodec, video::VideoCodec},
};
use crate::library::{Source, TranscodePayload};
use crate::progress::ProgressDispatch;
use crate::progress::ProgressStatus;
use crate::progress::TaskProgress;
use crate::progress::TaskTrait;
use crate::utils;
use anyhow::{Context, anyhow};

#[derive(Debug, Serialize, Clone, utoipa::ToSchema, PartialEq)]
pub struct TranscodeConfiguration {
    audio_codec: AudioCodec,
    video_codec: VideoCodec,
    resolution: Resolution,
}

#[derive(Debug, Serialize, Clone, utoipa::ToSchema, PartialEq)]
pub struct VideoProgress {
    relative_speed: f32,
    percent: f32,
}

pub trait FFmpegTask {
    fn args(&self) -> Vec<String>;
    fn cancel(
        output_path: &Path,
    ) -> impl std::future::Future<Output = Result<(), anyhow::Error>> + Send
    where
        Self: Sized;
}

impl TaskTrait for TranscodeJob {
    type Progress = VideoProgress;

    fn into_progress(status: ProgressStatus<Self>) -> TaskProgress
    where
        Self: Sized,
    {
        TaskProgress::Transcode(status)
    }
}

impl<T> ProgressDispatch<T> for FFmpegRunningJob<T>
where
    T: FFmpegTask + TaskTrait<Progress = VideoProgress> + Send,
{
    async fn progress(&mut self) -> Result<ProgressStatus<T>, crate::progress::TaskError> {
        tokio::select! {
            Some(progress) = self.stdout.next_progress_chunk() => {
                let progress = VideoProgress {
                    percent: progress.percent(&self.duration),
                    relative_speed: progress.relative_speed(),
                };
                Ok(ProgressStatus::Pending { progress } )
            }
            Ok(result) = self.process.wait() => {
                if result.success() {
                    Ok(ProgressStatus::Finish)
                } else {
                    Err(crate::progress::TaskError::Failure)
                }
            }
        }
    }

    async fn on_cancel(&mut self) -> anyhow::Result<()> {
        T::cancel(&self.output).await
    }
}

#[derive(Debug, Eq, PartialEq, Clone, utoipa::ToSchema, Serialize)]
pub struct PreviewsJob {
    video_id: i64,
    #[schema(value_type = Vec<String>)]
    output_path: PathBuf,
    #[schema(value_type = Vec<String>)]
    source_path: PathBuf,
}

impl PreviewsJob {
    pub fn new(
        video_id: i64,
        source_path: impl AsRef<Path>,
        output_path: impl AsRef<Path>,
    ) -> Self {
        Self {
            video_id,
            output_path: output_path.as_ref().to_path_buf(),
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
                self.output_path.to_string_lossy().to_string(),
                std::path::MAIN_SEPARATOR
            ),
        ]
    }

    async fn cancel(output_file: &Path) -> Result<(), anyhow::Error>
    where
        Self: Sized,
    {
        utils::clear_directory(output_file).await?;
        Ok(())
    }
}

impl TaskTrait for PreviewsJob {
    type Progress = VideoProgress;

    fn into_progress(status: ProgressStatus<Self>) -> TaskProgress
    where
        Self: Sized,
    {
        TaskProgress::Previews(status)
    }
}

#[derive(Debug, utoipa::ToSchema, Clone, Serialize, PartialEq)]
pub struct TranscodeJob {
    video_id: i64,
    #[schema(value_type = Vec<String>)]
    pub output_path: PathBuf,
    #[schema(value_type = Vec<String>)]
    pub source_path: PathBuf,
    payload: TranscodePayload,
    configuration: TranscodeConfiguration,
    hw_accel: bool,
}

impl TranscodeJob {
    pub async fn from_source(
        source: &Source,
        output: impl AsRef<Path>,
        payload: TranscodePayload,
        hw_accel: bool,
    ) -> Result<Self, anyhow::Error> {
        let source_path = source.video.path().to_path_buf();
        let metadata = source.video.metadata().await?;

        let default_audio = metadata.default_audio().context("missing default audio")?;
        let default_video = metadata.default_video().context("missing default video")?;
        let configuration = TranscodeConfiguration {
            resolution: payload.resolution.unwrap_or(default_video.resolution()),
            audio_codec: payload
                .audio_codec
                .as_ref()
                .unwrap_or(&default_audio.codec)
                .clone(),
            video_codec: payload
                .video_codec
                .as_ref()
                .unwrap_or(&default_video.codec)
                .clone(),
        };

        Ok(Self {
            video_id: source.id,
            source_path,
            payload,
            output_path: output.as_ref().to_path_buf(),
            configuration,
            hw_accel,
        })
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
        args.push("-sn".into());
        args.push(self.output_path.to_string_lossy().to_string());
        args
    }

    async fn cancel(output_file: &Path) -> Result<(), anyhow::Error>
    where
        Self: Sized,
    {
        use tokio::fs;
        fs::remove_file(output_file).await?;
        Ok(())
    }
}

// NOTE: resource move callback? (after job is done)
#[derive(Debug)]
pub struct FFmpegRunningJob<T: FFmpegTask> {
    process: Child,
    output: PathBuf,
    stdout: FFmpegProgressStdout,
    duration: Duration,
    _p: PhantomData<T>,
}

impl<T: FFmpegTask> FFmpegRunningJob<T> {
    pub fn spawn(
        job: &T,
        duration: Duration,
        output_path: PathBuf,
    ) -> anyhow::Result<FFmpegRunningJob<T>> {
        let mut process = Self::run(job.args())?;
        let stdout = FFmpegProgressStdout::new(process.stdout.take().unwrap());
        Ok(Self {
            output: output_path,
            process,
            stdout,
            duration,
            _p: PhantomData,
        })
    }

    /// Run ffmpeg command
    fn run<I, S>(args: I) -> anyhow::Result<Child>
    where
        I: IntoIterator<Item = S> + std::fmt::Debug,
        S: AsRef<OsStr>,
    {
        let ffmpeg: config::FFmpegPath = config::CONFIG.get_value();
        tracing::debug!("Spawning ffmpeg with args: {:?}", args);
        let mut cmd = tokio::process::Command::new(ffmpeg.as_ref());
        #[cfg(windows)]
        {
            cmd.creation_flags(crate::utils::CREATE_NO_WINDOW);
        }

        Ok(cmd
            .kill_on_drop(true)
            .args(["-progress", "pipe:1", "-nostats", "-y"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?)
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
    pub fn percent(&self, total_duration: &Duration) -> f32 {
        let current_duration = self.time.as_secs();
        (current_duration as f32 / total_duration.as_secs() as f32) * 100.
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
                    if let Some((time, speed)) = self.time.zip(self.speed) {
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

/// Save subtitles stream into the file
pub async fn convert_and_save_srt(
    output_path: impl AsRef<Path>,
    mut stream: impl tokio_stream::Stream<Item = std::io::Result<bytes::Bytes>> + Unpin,
) -> anyhow::Result<()> {
    let output_path = output_path.as_ref();
    if let Some(parent) = output_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let ffmpeg: config::FFmpegPath = config::CONFIG.get_value();
    let mut cmd = tokio::process::Command::new(ffmpeg.as_ref());

    #[cfg(windows)]
    {
        cmd.creation_flags(crate::utils::CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .args([
            OsStr::new("-hide_banner"),
            OsStr::new("-loglevel"),
            OsStr::new("error"),
            OsStr::new("-i"),
            OsStr::new("-"),
            OsStr::new("-c:s"),
            OsStr::new("text"),
            OsStr::new("-f"),
            OsStr::new("srt"),
            output_path.as_os_str(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut run = async || {
        {
            let mut stdin = child.stdin.take().expect("not taken");
            while let Some(Ok(bytes)) = stream.next().await {
                stdin.write_all(&bytes).await?;
            }
            stdin.shutdown().await?;
        }
        let output = child.wait().await?;
        if output.success() {
            Ok(())
        } else {
            Err(anyhow!("ffmpeg process was unexpectedly terminated"))
        }
    };
    if let Err(e) = run().await {
        let _ = tokio::fs::remove_file(output_path).await;
        return Err(e);
    }
    Ok(())
}

/// Extract subtitle track from provided file. Takes in desired track
pub async fn pull_subtitles(input_file: impl AsRef<Path>, track: usize) -> anyhow::Result<String> {
    let ffmpeg: config::FFmpegPath = config::CONFIG.get_value();
    let mut cmd = tokio::process::Command::new(ffmpeg.as_ref());

    #[cfg(windows)]
    {
        cmd.creation_flags(crate::utils::CREATE_NO_WINDOW);
    }
    let output = cmd
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
        .output()
        .await?;
    if output.status.success() {
        Ok(String::from_utf8(output.stdout).expect("ffmpeg output utf-8"))
    } else {
        Err(anyhow!("ffmpeg process was unexpectedly terminated"))
    }
}

static PULL_FRAME_PERMITS: Semaphore = Semaphore::const_new(4);

fn format_ffmpeg_time(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let hours = minutes / 60;
    format!("{:0>2}:{:0>2}:{:0>2}", hours, minutes % 60, seconds % 60)
}

/// Pull the frame at specified time location
pub async fn pull_frame(
    input_file: impl AsRef<Path>,
    output_file: impl AsRef<Path>,
    timing: Duration,
) -> anyhow::Result<()> {
    let _guard = PULL_FRAME_PERMITS.acquire().await.unwrap();
    let ffmpeg: config::FFmpegPath = config::CONFIG.get_value();
    let time = format_ffmpeg_time(timing);
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
    let mut cmd = tokio::process::Command::new(ffmpeg.as_ref());
    #[cfg(windows)]
    {
        cmd.creation_flags(crate::utils::CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let status = child.wait().await?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("ffmpeg process was unexpectedly terminated"))
    }
}

pub fn spawn_chromaprint_command(path: impl AsRef<Path>, take: Duration) -> std::io::Result<Child> {
    let path = path.as_ref().to_path_buf();
    let str_path = path.to_string_lossy();
    let ffmpeg: config::IntroDetectionFfmpegBuild = config::CONFIG.get_value();
    let mut cmd = tokio::process::Command::new(ffmpeg.0);
    #[cfg(windows)]
    {
        cmd.creation_flags(crate::utils::CREATE_NO_WINDOW);
    }
    cmd.args([
        "-hide_banner",
        "-i",
        &str_path,
        "-to",
        &format_ffmpeg_time(take),
        "-ac",
        "2",
        "-map",
        "0:a:0",
        "-f",
        "chromaprint",
        "-fp_format",
        "raw",
        "-",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
}
