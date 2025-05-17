use std::path::PathBuf;

use crate::{
    config::{self, APP_RESOURCES},
    library::{AudioCodec, VideoCodec},
};

pub mod command;
pub mod file_watcher;
pub mod job;
pub mod keyframe;
pub mod manifest;

#[derive(Debug, Clone)]
pub struct HlsTempPath(PathBuf);

impl HlsTempPath {
    pub fn hls_temp_dir() -> PathBuf {
        APP_RESOURCES.temp_path.join("hls")
    }

    pub fn new(task_id: uuid::Uuid) -> Self {
        Self(Self::hls_temp_dir().join(task_id.to_string()))
    }

    pub fn segment_path(&self, idx: usize) -> PathBuf {
        self.0.join(format!("{idx}.mp4"))
    }

    pub fn init_path(&self) -> PathBuf {
        self.0.join("init.mp4")
    }
}

/// Encoder configuration for hls live streams
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct HlsStreamConfiguration {
    /// Video encoder name
    ///
    /// If `None` selected video track will be copied
    video_encoder: Option<String>,
    /// Audio encoder name
    ///
    /// If `None` selected audio track will be copied
    audio_encoder: Option<String>,
    /// Video track index
    video_track: usize,
    /// Audio track index
    audio_track: usize,
}

impl HlsStreamConfiguration {
    pub async fn new(
        video: Option<VideoCodec>,
        audio: Option<AudioCodec>,
        video_track: usize,
        audio_track: usize,
    ) -> Self {
        let mut video_encoder = None;
        if let Some(video) = video {
            let hw_accel: config::HwAccel = config::CONFIG.get_value();
            if hw_accel.0 {
                video_encoder = Some(
                    video
                        .gpu_accelerated_encoder()
                        .await
                        .unwrap_or(video.default_encoder())
                        .to_string(),
                )
            } else {
                video_encoder = Some(video.default_encoder().to_string())
            }
        }

        let audio_encoder = audio.map(|a| a.to_string());

        Self {
            video_encoder,
            audio_encoder,
            audio_track,
            video_track,
        }
    }
}

impl Default for HlsStreamConfiguration {
    fn default() -> Self {
        Self {
            video_encoder: Some("libx264".to_owned()),
            audio_encoder: Some("aac".to_owned()),
            video_track: 0,
            audio_track: 0,
        }
    }
}
