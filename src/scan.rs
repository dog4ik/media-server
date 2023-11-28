use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::db::Db;
use crate::library::Chapter;
use crate::library::LibraryFile;
use crate::library::LibraryItem;

use crate::library::MediaFolders;
use crate::metadata_provider::MovieMetadataProvider;
use crate::metadata_provider::ShowMetadataProvider;
use crate::movie_file::MovieFile;
use crate::process_file::AudioCodec;
use crate::process_file::VideoCodec;
use crate::show_file::ShowFile;

const SUPPORTED_FILES: &[&str] = &["mkv", "webm", "mp4"];
const SUPPORTED_VIDEO_CODECS: &[&str] = &["h264", "mp4"];
const SUPPORTED_AUDIO_CODECS: &[&str] = &["aac", "mp3"];
const MAX_THREADS: usize = 2;

#[derive(Debug, Serialize, Clone)]
pub struct Summary {
    pub href: String,
    pub subs: Vec<String>,
    pub previews: usize,
    pub duration: String,
    pub title: String,
    pub chapters: Vec<Chapter>,
}

#[derive(Debug)]
pub struct Library {
    pub shows: Vec<ShowFile>,
    pub movies: Vec<MovieFile>,
    pub media_folders: MediaFolders,
    summary: Vec<Summary>,
}

pub async fn handle_show(
    show: ShowFile,
    db: Db,
    metadata_provider: &impl ShowMetadataProvider,
) -> Result<(), sqlx::Error> {
    // BUG: what happens if local title changes? Duplicate shows in db.
    // We'll be fine if we avoid dublicate and insert video with different title.
    // After failure it will lookup title in provider and match it again
    let show_query = sqlx::query!(
        r#"SELECT shows.id, shows.metadata_id, shows.metadata_provider FROM episodes 
                    JOIN videos ON videos.id = episodes.video_id
                    JOIN seasons ON seasons.id = episodes.season_id
                    JOIN shows ON shows.id = seasons.show_id
                    WHERE videos.local_title = ?;"#,
        show.title
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| (x.id, x.metadata_id.unwrap(), x.metadata_provider));

    let (show_id, show_metadata_id, _show_metadata_provider) = match show_query {
        Ok(data) => data,
        Err(e) => match e {
            sqlx::Error::RowNotFound => {
                let metadata = metadata_provider.show(&show.title).await.unwrap();
                let provider = metadata.metadata_provider.to_string();
                let metadata_id = metadata.metadata_id.clone().unwrap();
                let db_show = metadata.into_db_show().await;
                (db.insert_show(db_show).await?, metadata_id, provider)
            }
            _ => {
                return Err(e);
            }
        },
    };

    let season_id = sqlx::query!(
        "SELECT id FROM seasons WHERE show_id = ? AND number = ?",
        show_id,
        show.season
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| x.id);

    let season_id = match season_id {
        Ok(season_id) => season_id,
        Err(e) => match e {
            sqlx::Error::RowNotFound => {
                let metadata = metadata_provider
                    .season(&show_metadata_id, show.season.into())
                    .await
                    .unwrap();
                let db_season = metadata.into_db_season(show_id).await;
                db.insert_season(db_season).await?
            }
            _ => {
                return Err(e);
            }
        },
    };

    let episode_id = sqlx::query!(
        "SELECT id FROM episodes WHERE season_id = ? AND number = ?;",
        season_id,
        show.episode
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| x.id);

    if let Err(e) = episode_id {
        if let sqlx::Error::RowNotFound = e {
            let metadata = metadata_provider
                .episode(
                    &show_metadata_id,
                    show.season as usize,
                    show.episode as usize,
                )
                .await
                .unwrap();
            let db_video = show.into_db_video();
            let video_id = db.insert_video(db_video).await?;
            let db_episode = metadata.into_db_episode(season_id, video_id).await;
            db.insert_episode(db_episode).await?;
        } else {
            dbg!("unexpected error while fetching episode");
            return Err(e);
        }
    };
    Ok(())
}

pub async fn handle_movie(
    movie: MovieFile,
    db: Db,
    metadata_provider: &impl MovieMetadataProvider,
) -> Result<(), sqlx::Error> {
    let movie_query = sqlx::query!(
        r#"SELECT movies.id as "id!", movies.metadata_id, movies.metadata_provider FROM movies 
                    JOIN videos ON videos.id = movies.video_id
                    WHERE videos.local_title = ?;"#,
        movie.title
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| (x.id, x.metadata_id.unwrap(), x.metadata_provider));

    match movie_query {
        Err(e) => {
            if let sqlx::Error::RowNotFound = e {
                let db_video = movie.into_db_video();
                let video_id = db.insert_video(db_video).await?;
                let metadata = metadata_provider.movie(&movie.title).await.unwrap();
                let provider = metadata.metadata_provider.to_string();
                let metadata_id = metadata.metadata_id.clone().unwrap();
                let db_movie = metadata.into_db_movie(video_id).await;
                (db.insert_movie(db_movie).await?, metadata_id, provider);
            } else {
                return Err(e);
            }
        }
        _ => (),
    };

    Ok(())
}

impl Library {
    pub fn new(media_folders: MediaFolders, shows: Vec<ShowFile>, movies: Vec<MovieFile>) -> Self {
        let mut summary = Vec::new();
        for item in &shows {
            summary.push(extract_summary(item));
        }
        for item in &movies {
            summary.push(extract_summary(item));
        }
        Self {
            media_folders,
            shows,
            movies,
            summary,
        }
    }

    pub fn add_show(&mut self, path: PathBuf) -> anyhow::Result<ShowFile> {
        ShowFile::new(path).map(|show| {
            self.shows.push(show.clone());
            show
        })
    }

    pub fn add_movie(&mut self, path: PathBuf) -> anyhow::Result<MovieFile> {
        MovieFile::new(path).map(|movie| {
            self.movies.push(movie.clone());
            movie
        })
    }

    pub fn remove_show(&mut self, path: impl AsRef<Path>) {
        self.shows
            .iter()
            .position(|f| f.source_path() == path.as_ref())
            .map(|pos| self.shows.remove(pos));
    }

    pub fn remove_movie(&mut self, path: impl AsRef<Path>) {
        self.movies
            .iter()
            .position(|f| f.source_path() == path.as_ref())
            .map(|pos| self.movies.remove(pos));
    }

    pub fn remove_file(&mut self, path: impl AsRef<Path>) {
        self.remove_show(&path);
        self.remove_movie(path);
    }

    pub fn get_summary(&self) -> Vec<Summary> {
        self.summary.clone()
    }

    pub fn find(&self, path: impl AsRef<Path>) -> Option<&dyn LibraryItem> {
        let show = self
            .shows
            .iter()
            .find(|f| f.source_path() == path.as_ref())
            .map(|x| x as &dyn LibraryItem);
        if show.is_none() {
            return self
                .movies
                .iter()
                .find(|f| f.source_path() == path.as_ref())
                .map(|x| x as &dyn LibraryItem);
        }
        return show;
    }

    pub fn find_library_file(&self, path: impl AsRef<Path>) -> Option<LibraryFile> {
        let show = self
            .shows
            .iter()
            .find(|f| f.source_path() == path.as_ref())
            .map(|x| LibraryFile::Show(x.clone()));
        if show.is_none() {
            return self
                .movies
                .iter()
                .find(|f| f.source_path() == path.as_ref())
                .map(|x| LibraryFile::Movie(x.clone()));
        }
        return show;
    }

    pub fn all_files(&self) -> Vec<&dyn LibraryItem> {
        let mut result = Vec::new();
        self.shows
            .iter()
            .for_each(|s| result.push(s as &dyn LibraryItem));
        self.movies
            .iter()
            .for_each(|m| result.push(m as &dyn LibraryItem));
        return result;
    }

    pub fn full_refresh(&mut self) {
        let shows = self
            .media_folders
            .shows
            .iter()
            .fold(Vec::new(), |mut acc, show_path| {
                read_library_items(&show_path)
                    .into_iter()
                    .for_each(|mut files| acc.append(&mut files));
                return acc;
            });
        self.shows = shows;

        let movies = self
            .media_folders
            .movies
            .iter()
            .fold(Vec::new(), |mut acc, movie_path| {
                read_library_items(&movie_path)
                    .into_iter()
                    .for_each(|mut files| acc.append(&mut files));
                return acc;
            });
        self.movies = movies;
    }

    pub async fn reconciliate_library(
        &mut self,
        db: &Db,
        metadata_provider: impl ShowMetadataProvider + Send + Sync + 'static,
    ) -> Result<(), sqlx::Error> {
        let metadata_provider = Arc::new(metadata_provider);
        let db_episodes_videos = sqlx::query!(
            r#"SELECT videos.*, episodes.id as "episode_id!" FROM videos
        JOIN episodes ON videos.id = episodes.video_id"#
        )
        .fetch_all(&db.pool)
        .await?;
        self.full_refresh();
        let local_episodes = &self.shows;
        let mut common_paths: HashSet<&str> = HashSet::new();

        let local_episodes: HashMap<String, &ShowFile> = local_episodes
            .iter()
            .map(|ep| (ep.source_path().to_str().unwrap().into(), ep))
            .collect();

        for db_episode_video in &db_episodes_videos {
            let exists_in_db;
            let size_match;
            if let Some(local_eqivalent) = local_episodes.get(&db_episode_video.path) {
                exists_in_db = true;
                size_match = local_eqivalent.get_file_size() == db_episode_video.size as u64;
            } else {
                size_match = false;
                exists_in_db = false;
            }

            if exists_in_db && size_match {
                common_paths.insert(db_episode_video.path.as_str());
            };
        }

        // clean up db
        for db_episode_video in &db_episodes_videos {
            if !common_paths.contains(db_episode_video.path.as_str()) {
                println!("removing episode id {}", db_episode_video.episode_id);
                db.remove_episode(db_episode_video.episode_id).await?;
            }
        }

        let mut handles = Vec::new();

        for (local_ep_path, local_ep) in local_episodes {
            // skip existing media
            if common_paths.contains(local_ep_path.as_str()) {
                continue;
            }

            let local_ep = local_ep.clone();
            let db = db.clone();
            let metadata_provider = metadata_provider.clone();
            let handle = tokio::spawn(async move {
                let _ = handle_show(local_ep, db, &*metadata_provider).await;
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }
        Ok(())
    }
}

fn extract_summary(file: &impl LibraryItem) -> Summary {
    return Summary {
        previews: file.previews_count(),
        subs: file.get_subs(),
        duration: file.metadata().format.duration.clone(),
        href: file.url(),
        title: file.title(),
        chapters: file.chapters(),
    };
}

pub fn is_format_supported(path: &impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .map(|ex| SUPPORTED_FILES.contains(&ex.to_str().unwrap()))
        .unwrap_or(false)
}

pub fn walk_recursive<F>(
    folder: &PathBuf,
    filter_fn: Option<F>,
) -> Result<Vec<PathBuf>, std::io::Error>
where
    F: Fn(&PathBuf) -> bool + std::marker::Copy,
{
    let mut local_paths = Vec::new();
    let dir = fs::read_dir(&folder)?;
    for file in dir {
        let path = file?.path();
        if path.is_file() {
            if let Some(filter_fn) = filter_fn {
                if filter_fn(&path) {
                    local_paths.push(path);
                }
            } else {
                local_paths.push(path);
            }
        } else if path.is_dir() {
            local_paths.append(walk_recursive(&path.to_path_buf(), filter_fn)?.as_mut());
        }
    }
    return Ok(local_paths);
}

pub fn read_library_items<T: LibraryItem>(folder: &PathBuf) -> Result<Vec<T>, anyhow::Error> {
    let files = walk_recursive(folder, Some(is_format_supported))?;
    Ok(files
        .into_iter()
        .filter_map(|f| T::from_path(f).ok())
        .collect())
}

pub async fn transcode(files: &Vec<impl LibraryItem + Clone + Send + Sync + 'static>) {
    let mut handles = Vec::new();
    let semaphore = Arc::new(Semaphore::new(MAX_THREADS));

    for file in files.clone() {
        let semaphore = semaphore.clone();
        let handle = tokio::spawn(async move {
            let permit = semaphore.acquire_owned().await.unwrap();
            let metadata = &file.metadata();
            //handle subs
            for stream in metadata.subtitle_streams().iter() {
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
                            let mut process = file.generate_subtitles(stream.index, lang);
                            process.wait().await.unwrap();
                        } else {
                            continue;
                        }
                    } else if !PathBuf::from(format!(
                        "{}/{}.srt",
                        &file.subtitles_path().to_str().unwrap(),
                        "unknown"
                    ))
                    .try_exists()
                    .unwrap_or(false)
                    {
                        let mut process = file.generate_subtitles(stream.index, "unknown");
                        process.wait().await.unwrap();
                        break;
                    } else {
                        continue;
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
                let mut process = file.generate_previews();
                process.wait().await.unwrap();
            }

            //BUG: there is a bug when file has video tags that does not contain
            //eng track and we are not able to transcode audio

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
                let video_codec = match should_transcode_video {
                    true => Some(VideoCodec::H264),
                    false => None,
                };
                let mut process = file
                    .transcode_video(
                        video_codec,
                        transcode_audio_track.map(|t| (t as usize, AudioCodec::AAC)),
                    )
                    .unwrap();
                process.wait().await.unwrap();
            }
            drop(permit);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }
}

/// formats episode like S01E01
#[allow(unused)]
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

#[allow(unused)]
fn clean_up(files: &Vec<impl LibraryItem>) {
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
