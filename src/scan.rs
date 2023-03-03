use serde::Deserialize;
use std::fs;
use std::path::PathBuf;
use std::str::from_utf8;
use tokio::process::Command;

const SUPPORTED_FILES: &[&str] = &["mkv", "webm", "mp4"];

#[derive(Debug, Deserialize)]
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
}

#[derive(Debug, Deserialize)]
pub struct FFprobeFormat {
    pub duration: String,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]

pub struct FFprobeOutput {
    pub streams: Vec<FFprobeStream>,
    pub format: FFprobeFormat,
}

#[derive(Debug)]
pub struct ShowFile {
    pub title: String,
    pub episode: u8,
    pub season: u8,
    pub path: PathBuf,
}

pub struct ProcessData {
    pub duration: i32,
    pub have_subs: bool,
}

impl ShowFile {
    pub fn new(path: PathBuf) -> Option<ShowFile> {
        let file_name = path.file_name().unwrap().to_str().unwrap();
        let mut is_spaced = false;
        if file_name.contains(" ") {
            is_spaced = true
        }
        let tokens = match is_spaced {
            true => file_name.split(" "),
            false => file_name.split("."),
        };
        let mut name: Option<String> = None;
        let mut season: Option<u8> = None;
        let mut episode: Option<u8> = None;
        for token in tokens.map(|x| x.to_string().to_lowercase()) {
            if token.len() == 6 && token.starts_with("s") {
                match (
                    Some(token.get(1..3).unwrap().parse().unwrap()),
                    Some(token.get(4..6).unwrap().parse().unwrap()),
                ) {
                    (Some(se), Some(ep)) => {
                        season = Some(se);
                        episode = Some(ep);
                        break;
                    }
                    _ => (),
                };
            }
            match name {
                Some(ref mut n) => n.push_str(format!(" {}", token).as_str()),
                None => name = Some(token),
            }
        }
        if name.is_none() || season.is_none() || episode.is_none() {
            println!("Failed to build {:?} {:?} {:?}", name, season, episode);
            return None;
        }
        Some(ShowFile {
            title: name.unwrap(),
            episode: episode.unwrap(),
            season: season.unwrap(),
            path,
        })
    }
    pub async fn get_metadata(&self) -> Result<FFprobeOutput, anyhow::Error> {
        let output = Command::new("ffprobe")
            .args(&[
                "-v",
                "quiet",
                "-show_entries",
                "format=duration",
                "-print_format",
                "json",
                "-show_streams",
                self.path.to_str().unwrap(),
            ])
            .output()
            .await?;
        let output = from_utf8(&output.stdout)?;
        let metadata: FFprobeOutput = serde_json::from_str(output)?;
        Ok(metadata)
    }
}

fn walk_recursive(path_buf: &PathBuf) -> Result<Vec<ShowFile>, std::io::Error> {
    let mut local_paths: Vec<ShowFile> = vec![];
    let dir = fs::read_dir(&path_buf)?;
    for file in dir {
        let file = file.unwrap().path();
        if file.is_file() {
            if SUPPORTED_FILES.contains(&file.extension().unwrap().to_str().unwrap()) {
                let entry = ShowFile::new(file);
                if let Some(entry) = entry {
                    local_paths.push(entry)
                }
                continue;
            }
        }
        if file.is_dir() {
            let mut new_path = PathBuf::new();
            new_path.push(path_buf);
            new_path.push(file);
            local_paths.append(walk_recursive(&new_path)?.as_mut());
        }
    }
    return Ok(local_paths);
}
pub async fn process_file(path: &PathBuf) -> Result<FFprobeOutput, anyhow::Error> {
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
    let output = from_utf8(&output.stdout)?;
    let metadata: FFprobeOutput = serde_json::from_str(output)?;
    Ok(metadata)
}

pub fn scan(folders: Vec<PathBuf>) -> Vec<ShowFile> {
    let mut result = vec![];
    for dir in folders {
        match walk_recursive(&dir) {
            Ok(res) => result.extend(res),
            Err(err) => {
                println!("Failed to scan dir {:?} {:?}", dir, err);
                continue;
            }
        };
    }
    result
}
