use std::{fs, path::PathBuf};

use axum::{
    extract::{Path, State},
    http::Request,
    middleware::Next,
    response::Response,
};
use serde::{Deserialize, Serialize};

use crate::{get_metadata, process_file::FFprobeOutput, Library};

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

pub async fn movie_path_middleware<B>(
    Path(params): Path<MovieParams>,
    State(state): State<&'static Library>,
    mut request: Request<B>,
    next: Next<B>,
) -> Response {
    request.extensions_mut().insert(Path(params));
    request.extensions_mut().insert(State(state));
    let response = next.run(request).await;
    response
}

impl MovieFile {
    pub fn new(path: PathBuf) -> Result<Self, anyhow::Error> {
        let file_name = path.file_name().unwrap().to_str().unwrap();
        let mut is_spaced = false;
        if file_name.contains(" ") {
            is_spaced = true
        }
        let tokens = match is_spaced {
            true => file_name.split(" "),
            false => file_name.split("."),
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
            let resource = generate_resources(&name)?;
            let metadata = get_metadata(&path).unwrap();
            let show_file = Self {
                title: name,
                video_path: path,
                resources_path: resource,
                metadata,
            };
            Ok(show_file)
        } else {
            return Err(anyhow::Error::msg("Failed to build"));
        }
    }

    pub async fn get_metadata(&self) -> Result<FFprobeOutput, anyhow::Error> {
        get_metadata(&self.video_path)
    }
}

fn generate_resources(title: &str) -> Result<PathBuf, std::io::Error> {
    let movie_dir_path = format!("{}/{}", std::env::var("RESOURCES_PATH").unwrap(), title,);
    fs::create_dir_all(format!("{}/subs", &movie_dir_path))?;
    fs::create_dir_all(format!("{}/previews", &movie_dir_path))?;
    let folder = PathBuf::from(movie_dir_path);
    Ok(folder)
}
