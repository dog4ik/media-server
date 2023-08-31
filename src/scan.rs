use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::{fmt::Display, fs};

use serde::Serialize;
use tokio::sync::{mpsc, Semaphore};

use crate::ShowFile;

const SUPPORTED_FILES: &[&str] = &["mkv", "webm", "mp4"];
const SUPPORTED_VIDEO_CODECS: &[&str] = &["h264", "mp4"];
const SUPPORTED_AUDIO_CODECS: &[&str] = &["aac", "mp3"];
const DISABLE_PRINT: bool = false;
const THREADS: usize = 2;

#[derive(Debug, Clone, Serialize)]
pub struct Library {
    pub items: Vec<ShowFile>,
    pub dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub enum TaskType {
    Preview,
    Video,
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
            TaskType::Video => write!(f, "Video"),
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
                item.title.replace(' ', "-"),
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
    let semaphore = Arc::new(Semaphore::new(THREADS));
    let (tx, mut rx) = mpsc::channel(100);

    for file in files.clone() {
        let file = file.clone();
        let semaphore = semaphore.clone();
        let tx = tx.clone();
        let handle = tokio::spawn(async move {
            let permit = semaphore.acquire_owned().await.unwrap();
            let metadata = &file.metadata;
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
                            &file.resources_path.to_str().unwrap(),
                            lang
                        ))
                        .try_exists()
                        .unwrap_or(false)
                        {
                            file.generate_subtitles(stream.index, lang, tx.clone())
                                .await
                                .unwrap();
                        } else {
                            continue;
                        }
                    } else {
                        if !PathBuf::from(format!(
                            "{}/subs/{}.srt",
                            &file.resources_path.to_str().unwrap(),
                            "unknown"
                        ))
                        .try_exists()
                        .unwrap_or(false)
                        {
                            file.generate_subtitles(stream.index, "unknown", tx.clone())
                                .await
                                .unwrap();
                            break;
                        } else {
                            continue;
                        }
                    };
                }
            }

            // handle previews
            let preview_folder = fs::read_dir(
                PathBuf::from_str(
                    format!("{}/previews/", file.resources_path.to_str().unwrap()).as_str(),
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
                file.metadata
                    .format
                    .duration
                    .parse::<f64>()
                    .expect("duration to look like 123.1233")
                    .round() as u64,
            );

            if (previews_count as f64) < (duration.as_secs() as f64 / 10.0).round() {
                file.generate_previews(tx.clone()).await.unwrap();
            }

            // handle last one: codecs
            let mut transcode_audio_track: Option<i32> = None;
            let mut should_transcode_video = false;
            for stream in metadata.streams.iter() {
                if stream.codec_type == "video" && !should_transcode_video {
                    if !SUPPORTED_VIDEO_CODECS.contains(&stream.codec_name.as_str()) {
                        should_transcode_video = true;
                    }
                }
                if stream.codec_type == "audio" {
                    if !SUPPORTED_AUDIO_CODECS.contains(&stream.codec_name.as_str()) {
                        if let Some(tags) = &stream.tags {
                            match &tags.language {
                                Some(lang) if lang == "eng" => {
                                    transcode_audio_track = Some(stream.index);
                                    break;
                                }
                                Some(_) => continue,
                                None => {
                                    transcode_audio_track = Some(stream.index);
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if should_transcode_video || transcode_audio_track.is_some() {
                file.transcode_file(transcode_audio_track, should_transcode_video, tx)
                    .await
                    .unwrap();
            }
            drop(permit);
        });
        handles.push(handle);
    }

    // handle progress
    // clear the screen
    print!("{}[2J", 27 as char);
    let files_copy = files.clone();
    let print_handle = tokio::spawn(async move {
        if DISABLE_PRINT {
            while let Some(_) = rx.recv().await {}
            return;
        };
        let mut tasks: Vec<ProgressItem<'_>> = Vec::with_capacity(files_copy.len());
        while let Some(progress) = rx.recv().await {
            // print changes
            let mut out = std::io::stdout();
            let std_out_lock = out.lock();
            // set top position
            out.write_all(format!("{}[H", 27 as char).as_bytes())
                .unwrap();

            for file in &files_copy {
                if let Some(task) = tasks
                    .iter_mut()
                    .find(|x| x.file.video_path == file.video_path)
                {
                    if progress.video_path == task.file.video_path {
                        task.percent = progress.percent;
                        task.task_type = progress.task_type;
                    }
                    out.write_all(
                        format!(
                            "{} {}: {} ({}%)                         \n",
                            file.title,
                            format_episode(&file),
                            task.task_type,
                            task.percent
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                } else {
                    out.write_all(
                        format!(
                            "{} {}: TBD                         \n",
                            file.title,
                            format_episode(&file)
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                };
                // add task
                if progress.video_path == file.video_path {
                    if tasks
                        .iter()
                        .find(|x| progress.video_path == x.file.video_path)
                        .is_none()
                    {
                        tasks.push(ProgressItem {
                            task_type: progress.task_type,
                            percent: progress.percent,
                            file,
                        })
                    }
                }
            }
            out.flush().unwrap();
            drop(std_out_lock);
        }
    });

    for handle in handles {
        handle.await.unwrap();
    }

    print_handle.abort();

    clean_up(&files);

    println!("Everything is ready to play");

    return files;
}

/// prints green text in terminal
#[allow(dead_code)]
fn format_success(text: String) -> String {
    format!("\x1B[32m{}\x1B[0m", text)
}

/// formats episode like S01E01
fn format_episode(file: &ShowFile) -> String {
    let mut res = String::new();
    match file.season {
        x if x < 10 => res.push_str(&format!("S0{}", x)),
        x => res.push_str(&format!("S{}", x)),
    };
    match file.episode {
        x if x < 10 => res.push_str(&format!("E0{}", x)),
        x => res.push_str(&format!("E{}", x)),
    };
    return res;
}

fn clean_up(files: &Vec<ShowFile>) {
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
}
