use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    extract::{FromRequest, Path as AxumPath, Request, State},
    http::StatusCode,
    response::IntoResponse,
};
use axum_extra::headers::Range;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::{
    movie_file::{MovieFile, MovieParams},
    scan::Library,
    serve_content::ServeContent,
    show_file::{ShowFile, ShowParams},
    source::Source,
};

#[derive(Debug, serde::Deserialize, Clone)]
pub struct PreviewQuery {
    pub number: i32,
}

#[derive(Debug, Clone)]
pub enum LibraryFile {
    Show(ShowFile),
    Movie(MovieFile),
}

#[derive(Debug, Clone)]
pub struct MediaFolders {
    pub shows: Vec<PathBuf>,
    pub movies: Vec<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum MediaType {
    Show,
    Movie,
}

impl MediaFolders {
    pub fn all(&self) -> Vec<&PathBuf> {
        let mut out = Vec::with_capacity(self.shows.len() + self.movies.len());
        out.extend(self.shows.iter());
        out.extend(self.movies.iter());
        out
    }

    pub fn folder_type(&self, path: &PathBuf) -> Option<MediaType> {
        for show_dir in &self.shows {
            if path.starts_with(show_dir) {
                return Some(MediaType::Show);
            };
        }
        for movie_dir in &self.movies {
            if path.starts_with(movie_dir) {
                return Some(MediaType::Movie);
            };
        }
        None
    }
}

impl LibraryFile {
    pub async fn serve_video(&self, range: Range) -> impl IntoResponse {
        match self {
            LibraryFile::Show(s) => s.serve_video(range).await,
            LibraryFile::Movie(m) => m.serve_video(range).await,
        }
    }

    pub async fn serve_previews(&self, number: i32) -> impl IntoResponse {
        match self {
            LibraryFile::Show(s) => s.serve_previews(number).await,
            LibraryFile::Movie(m) => m.serve_previews(number).await,
        }
    }

    pub async fn serve_subs(&self, lang: Option<String>) -> impl IntoResponse {
        match self {
            LibraryFile::Show(s) => s.serve_subs(lang).await,
            LibraryFile::Movie(m) => m.serve_subs(lang).await,
        }
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct Chapter {
    pub title: String,
    pub start_time: String,
}

pub struct LibraryFileExtractor(pub LibraryFile);

#[axum::async_trait]
impl<S> FromRequest<S> for LibraryFileExtractor
where
    // these bounds are required by `async_trait`
    S: Send + Sync,
{
    type Rejection = StatusCode;

    async fn from_request(req: Request, _s: &S) -> Result<Self, Self::Rejection> {
        let state = req.extensions().get::<State<Arc<Mutex<Library>>>>();
        let movie_path_params = req.extensions().get::<AxumPath<MovieParams>>();
        let show_path_params = req.extensions().get::<AxumPath<ShowParams>>();

        if let Some(state) = state {
            if let Some(path_params) = movie_path_params {
                let state = state.lock().await;
                let file = state
                    .movies
                    .iter()
                    .find(|item| item.local_title == path_params.movie_name.replace('-', " "));
                if let Some(file) = file {
                    return Ok(LibraryFileExtractor(LibraryFile::Movie(file.clone())));
                }
            }

            if let Some(path_params) = show_path_params {
                let state = state.lock().await;
                let file = state.shows.iter().find(|item| {
                    item.episode == path_params.episode as u8
                        && item.local_title == path_params.show_name.replace('-', " ")
                        && item.season == path_params.season as u8
                });
                if let Some(file) = file {
                    return Ok(LibraryFileExtractor(LibraryFile::Show(file.clone())));
                }
            }

            return Err(StatusCode::NOT_FOUND);
        }
        return Err(StatusCode::BAD_REQUEST);
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub enum VideoId {
    Movie(i32),
    Episode(i32),
}

/// Trait that must be implemented for all library items
pub trait LibraryItem {
    /// Resources folder path
    fn resources_path(&self) -> &Path;

    /// Get origin video
    fn source(&self) -> &Source;

    /// Url part of file
    fn url(&self) -> String;

    /// Construct self from path
    fn from_path(path: PathBuf) -> Result<Self, anyhow::Error>
    where
        Self: Sized;

    fn source_path(&self) -> &PathBuf {
        &self.source().origin.path
    }

    /// Title
    fn title(&self) -> String;
}

impl LibraryItem for MovieFile {
    fn resources_path(&self) -> &Path {
        &self.source.resources_path
    }
    fn source(&self) -> &Source {
        &self.source
    }
    fn title(&self) -> String {
        self.local_title.clone()
    }
    fn url(&self) -> String {
        format!("/{}", self.local_title)
    }

    fn from_path(path: PathBuf) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Self::new(path)
    }
}

impl LibraryItem for ShowFile {
    fn resources_path(&self) -> &Path {
        &self.source.resources_path
    }
    fn source(&self) -> &Source {
        &self.source
    }
    fn title(&self) -> String {
        self.local_title.clone()
    }
    fn url(&self) -> String {
        format!("/{}/{}/{}", self.local_title, self.season, self.episode)
    }

    fn from_path(path: PathBuf) -> Result<Self, anyhow::Error>
    where
        Self: Sized,
    {
        Self::new(path)
    }
}
