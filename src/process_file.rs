use serde::Deserialize;
use std::{path::PathBuf, str::from_utf8};
use tokio::process::Command;

#[derive(Debug, Deserialize, Clone)]
pub struct FFprobeStream {
    pub index: i32,
    pub codec_name: String,
    pub codec_long_name: String,
    pub codec_type: String,
    pub codec_tag_string: String,
    pub codec_tag: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub coded_width: Option<i32>,
    pub coded_height: Option<i32>,
    pub sample_aspect_ratio: Option<String>,
    pub display_aspect_ratio: Option<String>,
    pub id: Option<String>,
    pub start_time: Option<String>,
    pub duration_ts: Option<i64>,
    pub duration: Option<String>,
    pub bit_rate: Option<String>,
    pub tags: FFprobeTags,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FFprobeFormat {
    pub duration: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FFprobeTags {
    pub language: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FFprobeDisposition {
    pub default: Option<i32>,
    pub dub: Option<i32>,
    pub original: Option<i32>,
    pub comment: Option<i32>,
    pub lyrics: Option<i32>,
    pub karaoke: Option<i32>,
    pub forced: Option<i32>,
    pub hearing_impaired: Option<i32>,
    pub visual_impaired: Option<i32>,
    pub clean_effects: Option<i32>,
    pub attached_pic: Option<i32>,
    pub timed_thumbnails: Option<i32>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FFprobeOutput {
    pub streams: Vec<FFprobeStream>,
    pub format: FFprobeFormat,
}
pub async fn get_metadata(path: &PathBuf) -> Result<FFprobeOutput, anyhow::Error> {
    let output = Command::new("ffprobe")
        .args(&[
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-print_format",
            "json",
            "-show_streams",
            path.to_str().unwrap(),
        ])
        .output()
        .await?;
    println!("{:?}", path);
    let output = from_utf8(&output.stdout)?;
    let metadata: FFprobeOutput = serde_json::from_str(output)?;
    Ok(metadata)
}
