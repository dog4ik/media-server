use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::app_state::AppError;
use crate::db::Db;
use crate::library::Chapter;
use crate::library::LibraryFile;
use crate::library::LibraryItem;

use crate::library::MediaFolders;
use crate::metadata_provider::MovieMetadataProvider;
use crate::metadata_provider::ShowMetadataProvider;
use crate::movie_file::MovieFile;
use crate::process_file::VideoCodec;
use crate::show_file::ShowFile;
use crate::source::Source;
use crate::utils;

const SUPPORTED_FILES: &[&str] = &["mkv", "webm", "mp4"];
const SUPPORTED_VIDEO_CODECS: &[&str] = &["h264", "mp4"];
const SUPPORTED_AUDIO_CODECS: &[&str] = &["aac", "mp3"];
const MAX_THREADS: usize = 2;

#[derive(Debug, Serialize, Clone)]
pub struct Summary {
    pub href: String,
    pub subs: Vec<String>,
    pub previews: usize,
    pub duration: Duration,
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
) -> Result<(), AppError> {
    // BUG: what happens if local title changes? Duplicate shows in db.
    // We'll be fine if we avoid dublicate and insert video with different title.
    // After failure it will lookup title in provider and match it again
    let show_query = sqlx::query!(
        r#"SELECT shows.id, shows.metadata_id, shows.metadata_provider FROM episodes 
                    JOIN videos ON videos.id = episodes.video_id
                    JOIN seasons ON seasons.id = episodes.season_id
                    JOIN shows ON shows.id = seasons.show_id
                    WHERE videos.local_title = ?;"#,
        show.local_title
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| (x.id, x.metadata_id.unwrap(), x.metadata_provider));

    let (show_id, show_metadata_id, _show_metadata_provider) = match show_query {
        Ok(data) => data,
        Err(e) => match e {
            sqlx::Error::RowNotFound => {
                let metadata = metadata_provider
                    .show(&show.local_title)
                    .await
                    .map_err(|err| {
                        tracing::error!(
                            "Metadata lookup failed for file with local title: {}",
                            show.local_title
                        );
                        err
                    })?;
                let provider = metadata.metadata_provider.to_string();
                let metadata_id = metadata.metadata_id.clone().unwrap();
                let db_show = metadata.into_db_show().await;
                (db.insert_show(db_show).await?, metadata_id, provider)
            }
            _ => {
                return Err(e)?;
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
                return Err(e)?;
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
            let db_video = show.source.into_db_video(show.local_title.clone());
            let video_id = db.insert_video(db_video).await?;
            for variant in &show.source.variants {
                let db_variant = variant.into_db_variant(video_id);
                db.insert_variant(db_variant).await?;
            }
            let db_episode = metadata.into_db_episode(season_id, video_id).await;
            db.insert_episode(db_episode).await?;
        } else {
            tracing::error!("Unexpected error while fetching episode {}", e);
            return Err(e)?;
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
        movie.local_title
    )
    .fetch_one(&db.pool)
    .await
    .map(|x| (x.id, x.metadata_id.unwrap(), x.metadata_provider));

    match movie_query {
        Err(e) => {
            if let sqlx::Error::RowNotFound = e {
                let metadata = metadata_provider.movie(&movie.local_title).await.unwrap();
                let db_video = movie.source.into_db_video(movie.local_title);
                let video_id = db.insert_video(db_video).await?;
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

    pub fn find_source(&self, path: impl AsRef<Path>) -> Option<&Source> {
        let show = self
            .shows
            .iter()
            .find(|f| f.source_path() == path.as_ref())
            .map(|x| &x.source);
        if show.is_none() {
            return self
                .movies
                .iter()
                .find(|f| f.source_path() == path.as_ref())
                .map(|x| &x.source);
        }
        show
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

    pub async fn full_refresh(&mut self) {
        let mut shows = Vec::new();
        for folder in &self.media_folders.shows {
            if let Ok(items) = explore_folder(folder).await {
                shows.extend(items);
            }
        }
        self.shows = shows;

        let mut movies = Vec::new();
        for folder in &self.media_folders.movies {
            if let Ok(items) = explore_folder(folder).await {
                movies.extend(items);
            }
        }
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
        self.full_refresh().await;
        let local_episodes = &self.shows;
        let mut common_paths: HashSet<&str> = HashSet::new();

        let local_episodes: HashMap<String, &ShowFile> = local_episodes
            .iter()
            .map(|ep| (ep.source.source_path().to_str().unwrap().into(), ep))
            .collect();

        for db_episode_video in &db_episodes_videos {
            let exists_in_db;
            let size_match;
            if let Some(local_eqivalent) = local_episodes.get(&db_episode_video.path) {
                exists_in_db = true;
                size_match =
                    local_eqivalent.source.origin.file_size() == db_episode_video.size as u64;
            } else {
                size_match = false;
                exists_in_db = false;
            }

            if exists_in_db && size_match {
                common_paths.insert(db_episode_video.path.as_str());
            };
        }

        // clean up variants

        // clean up db
        for db_episode_video in &db_episodes_videos {
            if !common_paths.contains(db_episode_video.path.as_str()) {
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
    let source = file.source();
    return Summary {
        previews: source.previews_count(),
        subs: source.get_subs(),
        duration: source.origin.duration(),
        href: file.url(),
        title: file.title(),
        chapters: source.origin.chapters(),
    };
}

pub fn is_format_supported(path: &impl AsRef<Path>) -> bool {
    path.as_ref()
        .extension()
        .map_or(false, |ex| SUPPORTED_FILES.contains(&ex.to_str().unwrap()))
}

#[tracing::instrument(level = "trace", name = "explore library folder")]
pub async fn explore_folder<T: LibraryItem + Send + 'static>(
    folder: &PathBuf,
) -> Result<Vec<T>, anyhow::Error> {
    let paths = utils::walk_recursive(folder, Some(is_format_supported))?;
    let mut handles = Vec::new();

    for path in paths {
        handles.push(tokio::spawn(async move { T::from_path(path) }));
    }

    let mut result = Vec::new();

    for handle in handles {
        if let Ok(item) = handle.await {
            let _ = item.map(|x| result.push(x));
        } else {
            tracing::error!("One of the metadata collectors paniced");
        }
    }

    return Ok(result);
}

pub async fn transcode(files: &Vec<Source>) {
    let mut handles = Vec::new();
    let semaphore = Arc::new(Semaphore::new(MAX_THREADS));

    for file in files.clone() {
        let semaphore = semaphore.clone();
        let handle = tokio::spawn(async move {
            let permit = semaphore.acquire_owned().await.unwrap();
            //handle subs
            for stream in file.origin.subtitle_streams().iter() {
                let mut subtitles_path = file.subtitles_path();
                subtitles_path.push(format!("{}.srt", stream.language));
                if !subtitles_path.try_exists().unwrap_or(false) {
                    let mut process = file.generate_subtitles(stream.index, stream.language);
                    process.wait().await.unwrap();
                }
            }

            // handle previews
            let previews_count = file.previews_count();
            let duration = file.origin.duration();

            if (previews_count as f64) < (duration.as_secs() as f64 / 10.0).round() {
                let mut process = file.generate_previews();
                process.wait().await.unwrap();
            }

            // handle last one: codecs
            let mut transcode_audio_track: Option<i32> = None;
            let mut should_transcode_video = false;
            if let Some(v_codec) = file.origin.default_video().map(|s| s.codec()) {
                if !should_transcode_video {
                    if !SUPPORTED_VIDEO_CODECS.contains(&v_codec.to_string().as_str()) {
                        should_transcode_video = true;
                    }
                }
            }

            if let Some(a_stream) = file.origin.default_audio() {
                if !SUPPORTED_AUDIO_CODECS.contains(&a_stream.codec().to_string().as_str()) {
                    transcode_audio_track = Some(a_stream.index);
                }
            }
            if should_transcode_video || transcode_audio_track.is_some() {
                let _video_codec = match should_transcode_video {
                    true => Some(VideoCodec::H264),
                    false => None,
                };
                // transcode here
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
