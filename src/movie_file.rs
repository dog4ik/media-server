use std::{path::PathBuf, sync::Arc};

use anyhow::anyhow;
use axum::{
    body::Body,
    extract::{Path, State},
    http::Request,
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::{
    process_file::{get_metadata, FFprobeOutput},
    scan::Library,
    utils,
};

#[derive(Debug, Clone, Serialize)]
pub struct MovieFile {
    pub title: String,
    pub video_path: PathBuf,
    pub resources_path: PathBuf,
    pub metadata: FFprobeOutput,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MovieParams {
    pub movie_name: String,
}

pub async fn movie_path_middleware(
    Path(params): Path<MovieParams>,
    State(state): State<Arc<Mutex<Library>>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    request.extensions_mut().insert(Path(params).clone());
    request.extensions_mut().insert(State(state).clone());
    let response = next.run(request).await;
    response
}

impl MovieFile {
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
        for token in tokens.map(|x| x.to_string().to_lowercase()) {
            let chars: Vec<char> = token.chars().into_iter().collect();
            if (token.len() == 4 || token.len() == 5)
                && chars[0].is_ascii_digit()
                && chars[1].is_ascii_digit()
                && chars[2].is_ascii_digit()
                && chars[3].is_ascii_digit()
            {
                break;
            }
            match name {
                Some(ref mut n) => n.push_str(&format!(" {}", token)),
                None => name = Some(token),
            }
        }
        if let Some(name) = name {
            let resources_path = generate_resources_path(&name);
            utils::generate_resources(&resources_path)?;
            let metadata = get_metadata(&path)?;
            let show_file = Self {
                title: name,
                video_path: path,
                resources_path,
                metadata,
            };
            Ok(show_file)
        } else {
            return Err(anyhow::Error::msg("Failed to construct a movie name"));
        }
    }

    pub async fn get_metadata(&self) -> Result<FFprobeOutput, anyhow::Error> {
        get_metadata(&self.video_path)
    }
}

fn generate_resources_path(title: &str) -> PathBuf {
    let mut movie_dir_path = PathBuf::from(std::env::var("RESOURCES_PATH").expect("env to be set"));
    movie_dir_path.push(title);
    movie_dir_path
}
