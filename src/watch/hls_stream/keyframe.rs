use std::{path::Path, process::Stdio};

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};

#[derive(Debug, Clone)]
pub struct KeyFrames {
    pub key_frames: Vec<Frame>,
    pub last_frame: Frame,
}

impl KeyFrames {
    pub fn closest_frame_with_offset(&self, position: u64) -> &Frame {
        let idx = match self
            .key_frames
            .binary_search_by(|k| k.position.cmp(&position))
        {
            Ok(idx) => idx,
            Err(insert_idx) => insert_idx.saturating_sub(1),
        };
        &self.key_frames[idx]
    }

    pub fn duration(&self, idx: usize) -> f64 {
        let target = self.key_frames[idx];
        if let Some(next) = self.key_frames.get(idx + 1) {
            next.time - target.time
        } else {
            self.last_frame.time - target.time
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub time: f64,
    position: u64,
    is_key: bool,
}

impl Frame {
    pub fn from_ffprobe_csv_output_line(line: String) -> Option<Self> {
        let mut split = line.splitn(3, ',');
        let time = split.next()?.parse().ok()?;
        let position = split.next()?.parse().ok()?;
        let is_key = split.next()?.starts_with('K');
        Some(Self {
            time,
            position,
            is_key,
        })
    }
}

pub async fn retrieve_keyframes(
    input_file: impl AsRef<Path>,
    video_track: usize,
    min_delay: f64,
) -> anyhow::Result<KeyFrames> {
    let mut cmd = tokio::process::Command::new("ffprobe");
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
            "packet=pts_time,flags,pos",
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
    let mut key_frames: Vec<Frame> = Vec::new();
    let mut last_frame = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(frame) = Frame::from_ffprobe_csv_output_line(line) {
            last_frame = Some(frame);
            if frame.is_key {
                if let Some(last) = key_frames.last() {
                    let delta = frame.time - last.time;
                    if delta < min_delay {
                        continue;
                    }
                };
                key_frames.push(frame);
            }
        };
    }
    Ok(KeyFrames {
        key_frames,
        last_frame: last_frame.expect("at least one frame"),
    })
}
