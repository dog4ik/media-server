use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::{fmt::Display, fs};

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::library::LibraryItem;
use crate::movie_file::MovieFile;
use crate::ShowFile;

const SUPPORTED_FILES: &[&str] = &["mkv", "webm", "mp4"];
const SUPPORTED_VIDEO_CODECS: &[&str] = &["h264", "mp4"];
const SUPPORTED_AUDIO_CODECS: &[&str] = &["aac", "mp3"];
const DISABLE_PRINT: bool = false;
const THREADS: usize = 2;

#[derive(Debug, Serialize)]
struct Summary {
    href: String,
    subs: Vec<String>,
    previews: usize,
    duration: String,
    title: String,
    season: Option<u8>,
    episode: Option<u8>,
}

pub struct Library {
    pub shows: Vec<ShowFile>,
    pub movies: Vec<MovieFile>,
}

#[derive(Debug, Clone, Copy)]
pub enum TaskType {
    Preview,
    Video,
    Subtitles,
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

#[derive(Debug, Clone)]
pub struct ProgressChunk {
    pub video_path: PathBuf,
    pub percent: u32,
    pub task_type: TaskType,
}

#[derive(Clone)]
struct ProgressItem<'a> {
    percent: u32,
    task_type: TaskType,
    file: &'a dyn LibraryItem,
}

impl Library {
    pub fn get_summary(&self) -> String {
        let mut result = vec![];
        for item in self.shows.clone() {
            result.push(extract_summary(item));
        }
        for item in self.movies.clone() {
            result.push(extract_summary(item));
        }
        serde_json::to_string(&result).unwrap()
    }
}

fn extract_summary(thing: impl LibraryItem) -> Summary {
    return Summary {
        previews: thing.previews_count(),
        subs: thing.get_subs(),
        duration: thing.metadata().format.duration.clone(),
        href: thing.url(),
        title: thing.title(),
        season: thing.season(),
        episode: thing.episode(),
    };
}

pub fn walk_recursive<T: LibraryItem>(folder: &PathBuf) -> Result<Vec<T>, std::io::Error> {
    let mut local_paths = vec![];
    let dir = fs::read_dir(&folder)?;
    for file in dir {
        let file = file?.path();
        if file.is_file() {
            if SUPPORTED_FILES.contains(&file.extension().unwrap().to_str().unwrap()) {
                let out = T::from_path(folder.clone());
                local_paths.push(out);
            }
        }
        if file.is_dir() {
            let mut new_path = PathBuf::new();
            new_path.push(&folder);
            new_path.push(file);
            local_paths.append(walk_recursive(folder)?.as_mut());
        }
    }
    return Ok(local_paths);
}

pub async fn transcode(files: &Vec<impl LibraryItem + Clone + Send + Sync + 'static>) {
    let mut handles = Vec::new();
    let semaphore = Arc::new(Semaphore::new(THREADS));
    let (tx, rx) = mpsc::channel();

    for file in files.clone() {
        let semaphore = semaphore.clone();
        let tx = tx.clone();
        let handle = tokio::spawn(async move {
            let permit = semaphore.acquire_owned().await.unwrap();
            let metadata = &file.metadata();
            //handle subs
            for stream in metadata
                .streams
                .iter()
                .filter(|&s| s.codec_type == "subtitle")
            {
                if let Some(tags) = &stream.tags {
                    if let Some(lang) = &tags.language {
                        if !PathBuf::from(format!(
                            "{}/{}.srt",
                            &file.subtitles_path().to_str().unwrap(),
                            lang
                        ))
                        .try_exists()
                        .unwrap_or(false)
                        {
                            file.generate_subtitles(stream.index, lang, tx.clone())
                                .unwrap();
                        } else {
                            continue;
                        }
                    } else {
                        if !PathBuf::from(format!(
                            "{}/{}.srt",
                            &file.subtitles_path().to_str().unwrap(),
                            "unknown"
                        ))
                        .try_exists()
                        .unwrap_or(false)
                        {
                            file.generate_subtitles(stream.index, "unknown", tx.clone())
                                .unwrap();
                            break;
                        } else {
                            continue;
                        }
                    };
                }
            }

            // handle previews
            let preview_folder = fs::read_dir(file.previews_path()).unwrap();
            let mut previews_count = 0;
            for preview in preview_folder {
                let file = preview.unwrap().path();
                if file.extension().unwrap() != "webm" {
                    previews_count += 1;
                }
            }
            let duration = std::time::Duration::from_secs(
                file.metadata()
                    .format
                    .duration
                    .parse::<f64>()
                    .expect("duration to look like 123.1233")
                    .round() as u64,
            );

            if (previews_count as f64) < (duration.as_secs() as f64 / 10.0).round() {
                file.generate_previews(tx.clone()).unwrap();
            }

            //BUG: there is small chance when we are not able to transcode audio
            //when tags dont contanin english

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
                file.transcode_video(transcode_audio_track, should_transcode_video, tx)
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
            while let Ok(_) = rx.recv() {}
            return;
        };
        let mut tasks: Vec<ProgressItem<'_>> = Vec::with_capacity(files_copy.len());
        while let Ok(progress) = rx.recv() {
            // print changes
            let mut out = std::io::stdout();
            let std_out_lock = out.lock();
            // set top position
            out.write_all(format!("{}[H", 27 as char).as_bytes())
                .unwrap();

            for file in &files_copy {
                if let Some(task) = tasks
                    .iter_mut()
                    .find(|x| x.file.resources_path() == file.resources_path())
                {
                    if progress.video_path == task.file.source_path() {
                        task.percent = progress.percent;
                        task.task_type = progress.task_type;
                    }
                    out.write_all(
                        format!(
                            "{} {}: ({}%)                         \n",
                            file.title(),
                            task.task_type,
                            task.percent
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                } else {
                    out.write_all(
                        format!(
                            "{}: TBD                                           \n",
                            file.title(),
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                };
                // add task
                if progress.video_path == file.source_path() {
                    if tasks
                        .iter()
                        .find(|x| progress.video_path == x.file.source_path())
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

fn clean_up(files: &Vec<impl LibraryItem>) {
    //clean up
    let resources_dir = fs::read_dir(std::env::var("RESOURCES_PATH").unwrap()).unwrap();
    for file in resources_dir {
        let file = file.unwrap().path();
        if file.is_dir()
            && files
                .iter()
                .any(|x| &x.title() == file.file_name().unwrap().to_str().unwrap())
        {
            continue;
        }
        fs::remove_dir_all(file).unwrap();
    }
}
