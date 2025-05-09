use std::{path::Path, process::Stdio};

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::config;

#[derive(Debug, Clone)]
pub struct KeyFrames {
    pub key_frames: Vec<f64>,
}

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub time: f64,
    is_key: bool,
}

impl Frame {
    pub fn from_ffprobe_csv_output_line(line: String) -> Option<Self> {
        let mut split = line.splitn(3, ',');
        let time = split.next()?.parse().ok()?;
        let is_key = split.next()?.starts_with('K');
        Some(Self { time, is_key })
    }
}

pub async fn retrieve_keyframes(
    input_file: impl AsRef<Path>,
    video_track: usize,
) -> anyhow::Result<KeyFrames> {
    let ffprobe_path: config::FFprobePath = config::CONFIG.get_value();
    let mut cmd = tokio::process::Command::new(ffprobe_path.as_ref());
    #[cfg(windows)]
    {
        cmd.creation_flags(crate::utils::CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .args([
            "-loglevel",
            "error",
            "-select_streams",
            &format!("v:{}", video_track),
            "-show_entries",
            "packet=pts_time,flags",
            "-of",
            "csv=print_section=0",
            input_file.as_ref().to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawn ffprobe command")?;

    let stdout = child.stdout.take().expect("std out is not taken");
    let mut lines = BufReader::new(stdout).lines();
    let mut key_frames = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(frame) = Frame::from_ffprobe_csv_output_line(line) {
            if frame.is_key {
                key_frames.push(frame.time);
            }
        };
    }
    Ok(KeyFrames { key_frames })
}
