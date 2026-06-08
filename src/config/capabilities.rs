use std::{io::BufRead, path::Path};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::{CONFIG, IntroDetectionFfmpegBuild};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CodecType {
    Audio,
    Video,
    Subtitle,
    Data,
    Attachment,
}

impl CodecType {
    pub fn from_char(char: char) -> Option<Self> {
        match char {
            'V' => Some(Self::Video),
            'A' => Some(Self::Audio),
            'S' => Some(Self::Subtitle),
            'D' => Some(Self::Data),
            'T' => Some(Self::Attachment),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Codec {
    pub codec_type: CodecType,
    pub name: String,
    pub long_name: String,
    pub decode_supported: bool,
    pub encode_supported: bool,
}

impl Codec {
    pub fn from_capability_line(line: String) -> Self {
        let mut split = line.split_terminator(' ').filter(|chunk| !chunk.is_empty());
        let mut params = split.next().unwrap().chars();
        let name = split.next().unwrap().to_string();
        let long_name = split.collect::<Vec<_>>().join(" ");
        let decode_supported = params.next().unwrap() == 'D';
        let encode_supported = params.next().unwrap() == 'E';
        let codec_type = CodecType::from_char(params.next().unwrap()).unwrap();
        Self {
            name,
            long_name,
            codec_type,
            encode_supported,
            decode_supported,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct Capabilities {
    pub chromaprint_enabled: bool,
}

impl Capabilities {
    pub async fn parse() -> Self {
        let chromaprint_ffmpeg: IntroDetectionFfmpegBuild = CONFIG.get_value();
        let chromaprint_enabled = Self::check_chromaprint_support(&chromaprint_ffmpeg.0)
            .await
            .inspect_err(|e| tracing::error!("Unable to fetch chromaprint support: {e}"))
            .unwrap_or(false);
        Self {
            chromaprint_enabled,
        }
    }

    async fn check_chromaprint_support(ffmpeg_path: &Path) -> anyhow::Result<bool> {
        let mut cmd = Command::new(ffmpeg_path);

        #[cfg(windows)]
        {
            cmd.creation_flags(crate::utils::CREATE_NO_WINDOW);
        }
        let out = cmd.arg("-version").output().await?;
        let mut lines = out.stdout.lines();
        let _ = lines.next().context("version line")??;
        let _ = lines.next();
        let configuration_line = lines.next().context("configuration line")??;
        Ok(configuration_line
            .split_ascii_whitespace()
            .skip(1)
            .any(|flag| flag == "--enable-chromaprint"))
    }
}
