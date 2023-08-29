use axum::response::AppendHeaders;
use bytes::Bytes;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Debug, Deserialize, Serialize)]
pub struct PosterData {
    pub url: String,
    pub show: String,
    pub season: Option<i32>,
    pub episode: Option<i32>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BackdropData {
    pub url: String,
    pub show: String,
}

async fn get_img(url: &str) -> anyhow::Result<Bytes> {
    let response = reqwest::get(url).await?;
    let body = response.bytes().await?;
    Ok(body)
}

pub async fn save_poster(items: PosterData) -> (StatusCode, Option<String>) {
    let res_dir: PathBuf = std::env::var("RESOURCES_PATH").unwrap().parse().unwrap();
    let mut path = PathBuf::new();
    path.push(&items.show);
    if let Some(season) = items.season {
        path.push(season.to_string());
        if let Some(episode) = items.episode {
            path.push(episode.to_string());
        }
    }
    let mut joined_path = res_dir.join(&path);
    let exists = joined_path.try_exists().unwrap_or(false);
    if exists {
        let bytes = match get_img(&items.url).await {
            Ok(bytes) => bytes,
            Err(_) => return (StatusCode::BAD_REQUEST, None),
        };
        joined_path.push("poster.jpg");
        if let Ok(_) = fs::write(joined_path, bytes) {
            return (
                StatusCode::OK,
                Some(format!("/{}", path.to_str().unwrap().replace(" ", "-"))),
            );
        } else {
            return (StatusCode::INTERNAL_SERVER_ERROR, None);
        }
    } else {
        return (StatusCode::NOT_FOUND, None);
    }
}

pub async fn save_backrop(item: BackdropData) -> Result<String, StatusCode> {
    let res_dir: PathBuf = std::env::var("RESOURCES_PATH").unwrap().parse().unwrap();
    let mut path = PathBuf::new();
    path.push(item.show);
    let mut joined_path = res_dir.join(&path);
    let exists = joined_path.try_exists().unwrap_or(false);
    if exists {
        let bytes = match get_img(&item.url).await {
            Ok(bytes) => bytes,
            Err(_) => return Err(StatusCode::BAD_REQUEST),
        };
        joined_path.push("backdrop.jpg");
        match fs::write(joined_path, bytes) {
            Ok(_) => return Ok(format!("/{}", path.to_str().unwrap().replace(" ", "-"))),
            Err(_) => return Err(StatusCode::BAD_REQUEST),
        }
    } else {
        return Err(StatusCode::NOT_FOUND);
    }
}

pub fn get_poster<'a>(
    show: String,
    season: Option<i32>,
    episode: Option<i32>,
) -> (StatusCode, AppendHeaders<(&'a str, &'a str)>, Option<Bytes>) {
    let res_dir: String = std::env::var("RESOURCES_PATH").unwrap().parse().unwrap();
    let mut path = PathBuf::from(res_dir);
    path.push(show.replace("-", " "));
    if let Some(season) = season {
        path.push(season.to_string());
        if let Some(episode) = episode {
            path.push(episode.to_string());
        }
    }
    path.push("poster.jpg");
    if let Ok(bytes) = fs::read(path) {
        return (
            StatusCode::OK,
            AppendHeaders(("Content-Type", "image/jpeg")),
            Some(bytes.into()),
        );
    } else {
        return (
            StatusCode::NOT_FOUND,
            AppendHeaders(("Content-Type", "image/jpeg")),
            None,
        );
    }
}

pub fn get_backdrop<'a>(
    show: String,
) -> (StatusCode, AppendHeaders<(&'a str, &'a str)>, Option<Bytes>) {
    let res_dir: String = std::env::var("RESOURCES_PATH").unwrap().parse().unwrap();
    let mut path = PathBuf::from(res_dir);
    //TODO: handle shows with space
    path.push(show.replace("-", " "));
    path.push("backdrop.jpg");
    if let Ok(bytes) = fs::read(path) {
        return (
            StatusCode::OK,
            AppendHeaders(("Content-Type", "image/jpeg")),
            Some(bytes.into()),
        );
    } else {
        return (
            StatusCode::NOT_FOUND,
            AppendHeaders(("Content-Type", "image/jpeg")),
            None,
        );
    }
}
