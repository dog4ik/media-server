use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};
use warp::hyper::{Body, Response, StatusCode};

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

pub async fn save_poster(items: PosterData) -> Result<Response<Body>, warp::Rejection> {
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
            Err(_) => return Ok(Response::builder().status(400).body(Body::empty()).unwrap()),
        };
        joined_path.push("poster.jpg");
        match fs::write(joined_path, bytes) {
            Ok(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(format!(
                        "/{}",
                        path.to_str().unwrap().replace(" ", "-")
                    )))
                    .unwrap());
            }
            Err(_) => return Ok(Response::builder().status(400).body(Body::empty()).unwrap()),
        }
    } else {
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::empty())
            .unwrap());
    }
}

pub async fn save_backrop(item: BackdropData) -> Result<Response<Body>, warp::Rejection> {
    let res_dir: PathBuf = std::env::var("RESOURCES_PATH").unwrap().parse().unwrap();
    let mut path = PathBuf::new();
    path.push(item.show);
    let mut joined_path = res_dir.join(&path);
    let exists = joined_path.try_exists().unwrap_or(false);
    if exists {
        let bytes = match get_img(&item.url).await {
            Ok(bytes) => bytes,
            Err(_) => return Ok(Response::builder().status(400).body(Body::empty()).unwrap()),
        };
        joined_path.push("backdrop.jpg");
        match fs::write(joined_path, bytes) {
            Ok(_) => {
                return Ok(Response::builder()
                    .status(StatusCode::OK)
                    .body(Body::from(format!(
                        "/{}",
                        path.to_str().unwrap().replace(" ", "-")
                    )))
                    .unwrap());
            }
            Err(_) => return Ok(Response::builder().status(400).body(Body::empty()).unwrap()),
        }
    } else {
        return Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(Body::empty())
            .unwrap());
    }
}

pub fn get_poster(
    show: String,
    season: Option<i32>,
    episode: Option<i32>,
) -> Result<Response<Body>, warp::Rejection> {
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
    match fs::read(path) {
        Ok(bytes) => {
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "image/jpeg")
                .body(Body::from(bytes))
                .unwrap())
        }
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::empty())
                .unwrap())
        }
    }
}

pub fn get_backdrop(show: String) -> Result<Response<Body>, warp::Rejection> {
    let res_dir: String = std::env::var("RESOURCES_PATH").unwrap().parse().unwrap();
    let mut path = PathBuf::from(res_dir);
    //TODO: handle shows with space
    path.push(show.replace("-", " "));
    path.push("backdrop.jpg");
    match fs::read(path) {
        Ok(bytes) => {
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "image/jpeg")
                .body(Body::from(bytes))
                .unwrap())
        }
        Err(_) => {
            return Ok(Response::builder()
                .status(StatusCode::BAD_REQUEST)
                .body(Body::empty())
                .unwrap())
        }
    }
}
