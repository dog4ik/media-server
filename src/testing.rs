use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{
    library::{movie::MovieIdentifier, show::ShowIdentifier, DataFolder, LibraryFile, Source},
    utils,
};

pub struct TestResource {
    pub temp_dir: PathBuf,
    pub test_show: LibraryFile<ShowIdentifier>,
    pub test_movie: LibraryFile<MovieIdentifier>,
    clean_on_drop: bool,
}

impl TestResource {
    pub fn new(clean_on_drop: bool) -> Self {
        let test_folder: PathBuf = "test-dir".into();
        let temp_dir = generate_temp_dir_path();
        fs::create_dir_all(&temp_dir).unwrap();
        deep_copy_folder(&test_folder, &temp_dir);

        let show_video_path = temp_dir.join("Episode.S01E01.mkv");

        let resource_path = temp_dir.join("resources");
        let show_metadata = show_video_path.metadata().unwrap();
        let show_data_folder = DataFolder::from_metadata(&show_metadata);
        let show_resource_path = resource_path.join(show_data_folder.to_string());
        let movie_resource_path = resource_path.join("movie");
        dbg!(&show_resource_path, &movie_resource_path);

        utils::generate_resources(&show_resource_path).unwrap();
        utils::generate_resources(&movie_resource_path).unwrap();

        let show_file = LibraryFile {
            identifier: ShowIdentifier {
                title: "episode".into(),
                episode: 1,
                season: 1,
            },
            data_folder: show_data_folder.clone(),
            source: Source::new(&show_video_path, &show_resource_path).unwrap(),
        };

        let movie_file = LibraryFile {
            identifier: MovieIdentifier {
                year: None,
                title: "movie".into(),
            },
            data_folder: show_data_folder,
            source: Source::new(show_video_path, movie_resource_path).unwrap(),
        };
        Self {
            temp_dir,
            clean_on_drop,
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
        if self.clean_on_drop {
            let _ = fs::remove_dir_all(&self.temp_dir);
        }
    }
}
