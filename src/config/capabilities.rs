use std::{io::BufRead, path::Path};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

use super::{CONFIG, IntroDetectionFfmpegBuild};

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
