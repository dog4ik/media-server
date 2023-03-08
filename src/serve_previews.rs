use std::path::PathBuf;

use tokio::fs;
use warp::hyper::{Body, Response, StatusCode};

use crate::ShowFile;

pub async fn serve_previews(
    file: ShowFile,
    number: i32,
) -> Result<Response<Body>, warp::Rejection> {
    let title = file.title.replace("-", " ");
    let path = PathBuf::from(format!(
        "{}/{}/{}/{}/previews",
        file.resources_path.to_str().unwrap(),
        title,
        file.season,
        file.episode
    ));
    let mut previews_dir = fs::read_dir(path).await.unwrap();
    let mut preview: Option<Vec<u8>> = None;
    loop {
        if let Some(file) = previews_dir.next_entry().await.unwrap() {
            let file_path = file.path();
            let file_name = file_path.file_stem().unwrap().to_str().unwrap();
            if file_name.to_string().parse::<i32>().unwrap() == number {
                preview = Some(fs::read(file_path).await.unwrap());
                break;
            }
        } else {
            break;
        }
    }
    match preview {
        Some(img) => Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(img))
            .unwrap()),
        None => Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap()),
    }
}
