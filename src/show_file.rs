use std::sync::Arc;
use std::{fs, path::PathBuf};

use anyhow::anyhow;
use axum::extract::{Path, State};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::process_file::{get_metadata, FFprobeOutput};
use crate::scan::Library;

pub struct ShowExtractor(pub ShowFile);

#[derive(Debug, Deserialize, Clone)]
pub struct ShowParams {
    pub show_name: String,
    pub season: usize,
    pub episode: usize,
}

pub async fn show_path_middleware<B>(
    Path(params): Path<ShowParams>,
    State(state): State<Arc<Mutex<Library>>>,
    mut request: Request<B>,
    next: Next<B>,
) -> Response {
    request.extensions_mut().insert(Path(params));
    request.extensions_mut().insert(State(state));
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
            .ok_or(anyhow!("fail to get filename"))?
            .to_str()
            .ok_or(anyhow!("fail convert filename"))?;
        let mut is_spaced = false;
        if file_name.contains(" ") {
            is_spaced = true
        }
        let tokens = match is_spaced {
            true => file_name.split(" "),
            false => file_name.split("."),
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
                match (Some(token[1..3].parse()?), Some(token[4..6].parse()?)) {
                    (Some(se), Some(ep)) => {
                        season = Some(se);
                        episode = Some(ep);
                        break;
                    }
                    _ => (),
                };
            }
            match name {
                Some(ref mut n) => n.push_str(&format!(" {}", token)),
                None => name = Some(token),
            }
        }
        if let (Some(name), Some(season), Some(episode)) = (name.clone(), season, episode) {
            let resource = generate_resources(&name, season, episode)?;
            let metadata = get_metadata(&path)?;
            let show_file = Self {
                title: name.replace('-', " ").trim().into(),
                episode,
                season,
                video_path: path,
                resources_path: resource,
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

pub fn generate_resources(title: &str, season: u8, episode: u8) -> Result<PathBuf, std::io::Error> {
    let episode_dir_path = format!(
        "{}/{}/{}/{}",
        std::env::var("RESOURCES_PATH").expect("env to be set"),
        title,
        season,
        episode
    );
    fs::create_dir_all(format!("{}/subs", &episode_dir_path))?;
    fs::create_dir_all(format!("{}/previews", &episode_dir_path))?;
    let folder = PathBuf::from(episode_dir_path);
    Ok(folder)
}
