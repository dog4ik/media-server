use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::{fmt::Display, fs};

use serde::Serialize;
use tokio::sync::{mpsc, Semaphore};

use crate::ShowFile;

const SUPPORTED_FILES: &[&str] = &["mkv", "webm", "mp4"];

#[derive(Debug, Clone, Serialize)]
pub struct Library {
    pub items: Vec<ShowFile>,
    pub dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub enum TaskType {
    Preview,
    Sound,
    Subtitles,
}

#[derive(Debug, Clone)]
pub struct ProgressChunk {
    pub video_path: PathBuf,
    pub percent: u32,
    pub task_type: TaskType,
}

#[derive(Debug, Clone)]
struct ProgressItem<'a> {
    percent: u32,
    task_type: TaskType,
    file: &'a ShowFile,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub title: String,
    pub season: u8,
    pub episode: u8,
    pub previews: i32,
    pub subs: Vec<String>,
    pub duration: String,
    pub href: String,
}

impl Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskType::Preview => write!(f, "Preview"),
            TaskType::Sound => write!(f, "Sound"),
            TaskType::Subtitles => write!(f, "Subtitles"),
        }
    }
}

impl Library {
    pub async fn new(dirs: Vec<PathBuf>) -> Library {
        Library {
            items: scan(&dirs).await,
            dirs,
        }
    }
    pub async fn update(&mut self) {
        let result = scan(&self.dirs).await;
        self.items = result
    }
    pub fn as_json(&self) -> String {
        serde_json::to_string(&self.items).unwrap()
    }

    pub fn get_summary(&self) -> String {
        let mut result = vec![];
        for item in self.items.clone() {
            let mut path = item.resources_path;
            // handle Subs
            path.push("subs");
            let subs_dir = fs::read_dir(&path).unwrap();
            let subs: Vec<_> = subs_dir
                .map(|sub| {
                    sub.unwrap()
                        .path()
                        .file_stem()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_string()
                })
                .collect();
            path.pop();
            path.push("previews");
            let previews_count = fs::read_dir(&path).unwrap().count();
            let href = format!(
                "/{}/{}/{}",
                item.title.replace(" ", "-"),
                item.season,
                item.episode
            );
            result.push(Summary {
                previews: previews_count as i32,
                subs,
                title: item.title,
                season: item.season,
                episode: item.episode,
                duration: item.metadata.format.duration,
                href,
            })
        }
        serde_json::to_string(&result).unwrap()
    }
}
pub fn walk_recursive(path_buf: &PathBuf) -> Result<Vec<ShowFile>, std::io::Error> {
    let mut local_paths: Vec<ShowFile> = vec![];
    let dir = fs::read_dir(&path_buf)?;
    for file in dir {
        let file = file.unwrap().path();
        if file.is_file() {
            if SUPPORTED_FILES.contains(&file.extension().unwrap().to_str().unwrap()) {
                let entry = ShowFile::new(file);
                if let Ok(entry) = entry {
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

pub async fn scan(folders: &Vec<PathBuf>) -> Vec<ShowFile> {
    let mut files = Vec::new();
    for dir in folders {
        match walk_recursive(&dir) {
            Ok(res) => files.extend(res),
            Err(err) => {
                println!("Failed to scan dir {:?} {:?}", dir, err);
                continue;
            }
        };
    }
    let mut handles = Vec::new();
    let subs_semaphore = Arc::new(Semaphore::new(10));
    let audio_semaphore = Arc::new(Semaphore::new(3));
    let previews_semaphore = Arc::new(Semaphore::new(2));
    let (tx, mut rx) = mpsc::channel(100);

    for file in files.clone() {
        let file_copy = file.clone();
        let tx_clone = tx.clone();
        let subs_semaphore_clone = subs_semaphore.clone();
        let handle = tokio::spawn(async move {
            let permit = subs_semaphore_clone.acquire_owned().await.unwrap();
            let metadata = &file_copy.metadata;
            //handle subs
            for stream in metadata
                .streams
                .iter()
                .filter(|&s| s.codec_type == "subtitle")
            {
                if let Some(tags) = &stream.tags {
                    if let Some(lang) = &tags.language {
                        if !PathBuf::from(format!(
                            "{}/subs/{}.srt",
                            &file_copy.resources_path.to_str().unwrap(),
                            lang
                        ))
                        .try_exists()
                        .unwrap_or(false)
                        {
                            file_copy
                                .generate_subtitles(stream.index, lang, tx_clone.clone())
                                .await
                                .unwrap();
                        } else {
                            continue;
                        }
                    } else {
                        if !PathBuf::from(format!(
                            "{}/subs/{}.srt",
                            &file_copy.resources_path.to_str().unwrap(),
                            "unknown"
                        ))
                        .try_exists()
                        .unwrap_or(false)
                        {
                            file_copy
                                .generate_subtitles(stream.index, "unknown", tx_clone)
                                .await
                                .unwrap();
                            break;
                        } else {
                            continue;
                        }
                    };
                }
            }
            drop(permit);
        });
        handles.push(handle);

        let tx_clone = tx.clone();
        let file_copy = file.clone();
        let audio_semaphore_clone = audio_semaphore.clone();
        let handle = tokio::spawn(async move {
            let permit = audio_semaphore_clone.acquire_owned().await.unwrap();
            let metadata = &file_copy.metadata;
            //handle audio
            for stream in metadata.streams.iter().filter(|&s| s.codec_type == "audio") {
                if let Some(tags) = &stream.tags {
                    match &tags.language {
                        Some(lang) => {
                            if lang == "eng" {
                                if stream.codec_name != "aac" && stream.codec_name != "mp3" {
                                    file_copy
                                        .transcode_audio(stream.index, tx_clone.clone())
                                        .await
                                        .unwrap();
                                    break;
                                }
                                break;
                            }
                        }
                        None => {
                            if stream.codec_name != "aac" && stream.codec_name != "mp3" {
                                file_copy
                                    .transcode_audio(stream.index, tx_clone)
                                    .await
                                    .unwrap();
                            }
                            break;
                        }
                    }
                }
            }
            drop(permit);
        });
        handles.push(handle);

        // handle previews
        let tx_copy = tx.clone();
        let file_copy = file.clone();
        let previews_semaphore_clone = previews_semaphore.clone();
        let handle = tokio::spawn(async move {
            let permit = previews_semaphore_clone.acquire_owned().await.unwrap();
            let preview_folder = fs::read_dir(
                PathBuf::from_str(
                    format!("{}/previews/", file_copy.resources_path.to_str().unwrap()).as_str(),
                )
                .unwrap(),
            )
            .unwrap();
            let mut previews_count = 0;
            for preview in preview_folder {
                let file = preview.unwrap().path();
                if file.extension().unwrap() != "webm" {
                    previews_count += 1;
                }
            }
            let duration = std::time::Duration::from_secs(
                file_copy
                    .metadata
                    .format
                    .duration
                    .parse::<f64>()
                    .expect("duration to look like 123.1233")
                    .floor() as u64,
            );

            if previews_count < duration.as_secs() / 10 {
                file_copy.generate_previews(tx_copy).await.unwrap();
            }
            drop(permit);
        });
        handles.push(handle);
    }

    // handle progress
    // clear the screen
    print!("{}[2J", 27 as char);

    let capacity = files.len();

    let files_copy = files.clone();
    let mut tasks: Vec<ProgressItem<'_>> = Vec::with_capacity(capacity);

    while let Some(progress) = rx.recv().await {
        // print changes
        let mut founded = false;
        let mut should_end = true;
        let mut std_out = std::io::stdout();
        let std_out_lock = std_out.lock();
        // set top position
        std_out
            .write_all(format!("{}[H", 27 as char).as_bytes())
            .unwrap();

        for item in &mut tasks {
            if item.file.video_path == progress.video_path {
                item.percent = progress.percent;
                founded = true;
            }
            if item.percent == 100 {
                let out = format!(
                    "{} season {} episode {} >> {} is {}%\n",
                    item.file.title,
                    item.file.season,
                    item.file.episode,
                    item.task_type,
                    item.percent
                );
                std_out.write_all(format_success(out).as_bytes()).unwrap();
            } else {
                should_end = false;
                let out = format!(
                    "{} season {} episode {} >> {} is {}%\n",
                    item.file.title,
                    item.file.season,
                    item.file.episode,
                    item.task_type,
                    item.percent
                );
                std_out.write(out.as_bytes()).unwrap();
            }
        }
        std_out.flush().unwrap();
        drop(std_out_lock);
        if !founded {
            let f = files_copy
                .iter()
                .find(|x| x.video_path == progress.video_path)
                .expect("file is being processed thus it is being found");
            tasks.push(ProgressItem {
                task_type: progress.task_type,
                percent: progress.percent,
                file: f,
            });
        }
        if founded && should_end {
            break;
        }
    }

    for handle in handles {
        let _ = handle.await;
    }

    //clean up
    let resources_dir = fs::read_dir(std::env::var("RESOURCES_PATH").unwrap()).unwrap();
    for file in resources_dir {
        let file = file.unwrap().path();
        if file.is_dir()
            && files
                .iter()
                .any(|x| &x.title == file.file_name().unwrap().to_str().unwrap())
        {
            continue;
        }
        fs::remove_dir_all(file).unwrap();
    }

    println!("Everything is ready to play");

    return files;
}

/// prints green text in terminal
fn format_success(text: String) -> String {
    format!("\x1B[32m{}\x1B[0m", text)
}
