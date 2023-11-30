use std::path::PathBuf;
use std::sync::Arc;

use anyhow::anyhow;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::process_file::{get_metadata, FFprobeOutput};
use crate::scan::Library;
use crate::utils;

pub struct ShowExtractor(pub ShowFile);

#[derive(Debug, Deserialize, Clone)]
pub struct ShowParams {
    pub show_name: String,
    pub season: usize,
    pub episode: usize,
}

pub async fn show_path_middleware(
    Path(params): Path<ShowParams>,
    State(state): State<Arc<Mutex<Library>>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    request.extensions_mut().insert(Path(params).clone());
    request.extensions_mut().insert(State(state).clone());
    let response = next.run(request).await;
    response
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowFile {
    pub title: String,
    pub episode: u8,
    pub season: u8,
    pub video_path: PathBuf,
    pub resources_path: PathBuf,
    pub metadata: FFprobeOutput,
}

impl ShowFile {
    pub fn new(path: PathBuf) -> Result<Self, anyhow::Error> {
        let file_name = path
            .file_name()
            .ok_or(anyhow!("failed to get filename"))?
            .to_str()
            .ok_or(anyhow!("failed to convert filename"))?;
        let is_spaced = file_name.contains(' ');
        let tokens = match is_spaced {
            true => file_name.split(' '),
            false => file_name.split('.'),
        };
        let mut name: Option<String> = None;
        let mut season: Option<u8> = None;
        let mut episode: Option<u8> = None;
        for token in tokens.map(|x| x.to_string().to_lowercase()) {
            let chars: Vec<char> = token.chars().into_iter().collect();
            if token.len() == 6
                && chars[0] == 's'
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3] == 'e'
                && chars[4].is_ascii_digit()
                && chars[5].is_ascii_digit()
            {
                let s: Option<u8> = token[1..3].parse().ok();
                let e: Option<u8> = token[4..6].parse().ok();
                if let (Some(se), Some(ep)) = (s, e) {
                    season = Some(se);
                    episode = Some(ep);
                    break;
                };
            }
            match name {
                Some(ref mut n) => n.push_str(&format!(" {}", token)),
                None => name = Some(token),
            }
        }
        if let (Some(name), Some(season), Some(episode)) = (name.clone(), season, episode) {
            let resources_path = generate_resources_path(&name, season, episode);
            utils::generate_resources(&resources_path)?;
            let metadata = get_metadata(&path)?;
            let show_file = Self {
                title: name.replace('-', " ").trim().into(),
                episode,
                season,
                video_path: path,
                resources_path,
                metadata,
            };
            Ok(show_file)
        } else {
            return Err(anyhow::anyhow!(
                "Failed to construct a show name ({:?}, {:?}, {:?})",
                name,
                season,
                episode
            ));
        }
    }

    pub async fn get_metadata(&self) -> Result<FFprobeOutput, anyhow::Error> {
        get_metadata(&self.video_path)
    }
}

fn generate_resources_path(title: &str, season: u8, episode: u8) -> PathBuf {
    let mut episode_dir_path =
        PathBuf::from(std::env::var("RESOURCES_PATH").expect("env to be set"));
    episode_dir_path.push(title);
    episode_dir_path.push(season.to_string());
    episode_dir_path.push(episode.to_string());
    episode_dir_path
}
