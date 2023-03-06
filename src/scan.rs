use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use serde::Serialize;

use crate::ShowFile;

const SUPPORTED_FILES: &[&str] = &["mkv", "webm", "mp4"];

#[derive(Debug, Clone, Serialize)]
pub struct Library {
    pub items: Vec<ShowFile>,
    pub dirs: Vec<PathBuf>,
}

struct Summary {
    title: String,
    season: u8,
    episode: u8,
    have_subs: bool,
    have_previews: bool,
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
    //    pub fn get_summary(&self) -> Vec<Summary> {
    //        for item in self.items {}
    //    }
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
    for file in &result {
        let metadata = file.get_metadata().await.unwrap();

        //handle subs
        for stream in metadata
            .streams
            .iter()
            .filter(|&s| s.codec_type == "subtitle")
        {
            match &stream.tags.language {
                Some(lang) => {
                    if !PathBuf::from(format!(
                        "{}/subs/{}.srt",
                        &file.resources_path.to_str().unwrap(),
                        lang
                    ))
                    .try_exists()
                    .unwrap()
                    {
                        file.generate_subtitles(stream.index, lang).await.unwrap();
                    } else {
                        continue;
                    }
                }
                None => {
                    if !PathBuf::from(format!(
                        "{}/subs/{}.srt",
                        &file.resources_path.to_str().unwrap(),
                        "unknown"
                    ))
                    .exists()
                    {
                        continue;
                    }
                    file.generate_subtitles(stream.index, "unknown")
                        .await
                        .unwrap();
                    break;
                }
            };
        }

        //handle audio
        for stream in metadata.streams.iter().filter(|&s| s.codec_type == "audio") {
            match &stream.tags.language {
                Some(lang) => {
                    if lang == "eng" {
                        if stream.codec_name != "aac" && stream.codec_name != "mp3" {
                            file.transcode_audio(stream.index).await.unwrap();
                            break;
                        }
                        break;
                    }
                }
                None => {
                    if stream.codec_name != "aac" && stream.codec_name != "mp3" {
                        file.transcode_audio(stream.index).await.unwrap();
                    }
                    break;
                }
            }
        }
        let preview_folder = fs::read_dir(
            PathBuf::from_str(
                format!("{}/previews/", file.resources_path.to_str().unwrap()).as_str(),
            )
            .unwrap(),
        )
        .unwrap();
        let mut count_previews = 0;
        for file in preview_folder {
            let file = file.unwrap().path();
            if file.extension().unwrap() != "webm" {
                count_previews += 1;
            }
        }
        if count_previews == 0 {
            file.generate_previews().await.unwrap();
        }
    }

    //clean up
    let resources_dir =
        fs::read_dir("/home/dog4ik/Documents/dev/rust/media-server/resources").unwrap();
    for file in resources_dir {
        let file = file.unwrap().path();
        if file.is_dir()
            && result
                .iter()
                .any(|x| &x.title == file.file_name().unwrap().to_str().unwrap())
        {
            continue;
        }
        fs::remove_dir_all(file).unwrap();
    }

    result
}
