use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{movie_file::MovieFile, process_file::get_metadata, show_file::ShowFile};

pub struct TestResource {
    pub temp_dir: PathBuf,
    pub test_show: ShowFile,
    pub test_movie: MovieFile,
}

impl TestResource {
    pub fn new() -> Self {
        let test_folder: PathBuf = "/home/dog4ik/personal/rust-media-server/test-dir".into();
        let temp_dir = generate_temp_dir_path();
        fs::create_dir_all(&temp_dir).unwrap();
        deep_copy_folder(test_folder, &temp_dir);

        let mut show_video_path = temp_dir.clone();
        show_video_path.push("Episode.S01E01.mkv");

        let mut resource_path = temp_dir.clone();
        resource_path.push("resources");

        let metadata = get_metadata(&show_video_path).unwrap();

        let show_file = ShowFile {
            title: "episode".into(),
            episode: 1,
            season: 1,
            video_path: show_video_path.clone(),
            resources_path: resource_path.clone(),
            metadata: metadata.clone(),
        };

        let movie_file = MovieFile {
            video_path: show_video_path.clone(),
            metadata,
            title: "movie".into(),
            resources_path: resource_path.clone(),
        };

        Self {
            temp_dir,
            test_show: show_file,
            test_movie: movie_file,
        }
    }
}

fn generate_temp_dir_path() -> PathBuf {
    let mut temp_path = std::env::temp_dir();
    temp_path.push("media-server-test");
    let random_folder = format!("{}", uuid::Uuid::new_v4());
    temp_path.push(random_folder);
    return temp_path;
}

fn get_last_part(bigger: impl AsRef<Path>) -> PathBuf {
    let last = bigger.as_ref().iter().last().unwrap();
    PathBuf::from(last)
}

fn deep_copy_folder(from_path: impl AsRef<Path>, to_path: impl AsRef<Path>) {
    let dir = fs::read_dir(&from_path).unwrap();
    for entry in dir {
        if let Ok(entry) = entry {
            let file_type = entry.file_type().unwrap();

            let last_part = get_last_part(&entry.path());
            let mut to_path: PathBuf = to_path.as_ref().to_path_buf();
            to_path.push(last_part);
            if file_type.is_dir() {
                fs::create_dir(&to_path).unwrap();
                deep_copy_folder(entry.path(), &to_path);
            } else if file_type.is_file() {
                fs::copy(entry.path(), to_path).unwrap();
            }
        }
    }
}

impl Drop for TestResource {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}
