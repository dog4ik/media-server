use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::Context;
use serde::Serialize;
use tokio::process::{Child, Command};

use crate::{
    db::{DbVariant, DbVideo},
    library::Chapter,
    process_file::{
        get_metadata, AudioCodec, FFmpegJob, FFprobeAudioStream, FFprobeOutput,
        FFprobeSubtitleStream, FFprobeVideoStream, Resolution, VideoCodec,
    },
    utils,
};

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

    /// Get prewies count
    pub fn previews_count(&self) -> usize {
        return std::fs::read_dir(self.previews_path()).unwrap().count();
    }

    /// Clean all generated resources
    pub fn delete_resources(&self) -> Result<(), std::io::Error> {
        std::fs::remove_dir_all(&self.resources_path)
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

    /// Run ffmpeg command. Returns handle to process
    pub fn run_command<I, S>(&self, args: I) -> Child
    where
        I: IntoIterator<Item = S> + Copy,
        S: AsRef<std::ffi::OsStr>,
    {
        Command::new("ffmpeg")
            .kill_on_drop(true)
            .args(["-progress", "pipe:1", "-nostats"])
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("process to spawn")
    }

    /// Generate previews for file
    pub fn generate_previews(&self) -> FFmpegJob {
        let args = vec![
            "-i".into(),
            self.source_path().to_str().unwrap().into(),
            "-vf".into(),
            "fps=1/10,scale=120:-1".into(),
            format!(
                "{}{}%d.jpg",
                self.previews_path().to_str().unwrap(),
                std::path::MAIN_SEPARATOR
            ),
        ];
        let child = self.run_command(&args);
        FFmpegJob::new(child, self.origin.duration(), self.source_path().into())
    }

    /// Generate subtitles for file
    pub fn generate_subtitles(&self, track: i32, language: &str) -> FFmpegJob {
        let args = vec![
            "-i".into(),
            self.source_path().to_str().unwrap().into(),
            "-map".into(),
            format!("0:{}", track),
            format!(
                "{}{}{}.srt",
                self.subtitles_path().to_str().unwrap(),
                std::path::MAIN_SEPARATOR,
                language
            ),
            "-c:s".into(),
            "copy".into(),
            "-y".into(),
        ];

        let child = self.run_command(&args);
        FFmpegJob::new(child, self.origin.duration(), self.source_path().into())
    }

    /// Transcode file
    pub fn transcode_video(
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
        let child = self.run_command(&args);
        let job = FFmpegJob::new(child, self.origin.duration(), self.source_path().into());
        return Ok(job);
    }

    /// Returns struct compatible with database Video table
    pub fn into_db_video(&self, local_title: String) -> DbVideo {
        let origin = &self.origin;
        let metadata = &origin.metadata;
        let now = time::OffsetDateTime::now_utc();
        let hash = origin.calculate_video_hash().unwrap();

        DbVideo {
            id: None,
            path: self.source_path().to_str().unwrap().to_string(),
            hash: hash.to_string(),
            local_title,
            size: origin.file_size() as i64,
            duration: metadata.format.duration.parse::<f64>().unwrap() as i64,
            audio_codec: origin.default_audio().map(|c| c.codec_name.to_string()),
            video_codec: origin.default_video().map(|c| c.codec_name.to_string()),
            resolution: origin.default_video().map(|c| c.resoultion().to_string()),
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
        std::fs::metadata(&self.path).expect("exist").len()
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

    /// Convert into database compatible struct
    pub fn into_db_variant(&self, video_id: i64) -> DbVariant {
        let hash = self
            .calculate_video_hash()
            .expect("source file to be found");
        let size = self.file_size();
        DbVariant {
            id: None,
            video_id,
            path: self.path.to_str().unwrap().to_string(),
            hash: hash.to_string(),
            size: size as i64,
            duration: self.duration().as_secs() as i64,
            video_codec: self.default_video().map(|c| c.codec().to_string()),
            audio_codec: self.default_audio().map(|c| c.codec().to_string()),
            resolution: self.resolution().map(|r| r.to_string()),
        }
    }
}
