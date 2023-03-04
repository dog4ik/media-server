use std::path::PathBuf;

use tokio::fs;
use warp::hyper::{Body, Response, StatusCode};

pub async fn serve_previews(
    title: String,
    season: i32,
    episode: i32,
    number: i32,
) -> Result<Response<Body>, warp::Rejection> {
    let title = title.replace("-", " ");
    let path = PathBuf::from(format!(
        "/home/dog4ik/Documents/dev/rust/media-server/resources/{}/{}/{}/previews",
        title, season, episode
    ));
    let mut previews_dir = fs::read_dir(path).await.unwrap();
    let mut preview: Option<Vec<u8>> = None;
    loop {
        if let Some(file) = previews_dir.next_entry().await.unwrap() {
            let file_path = file.path();
            let file_name = file_path.file_stem().unwrap().to_str().unwrap();

            if file_name.to_string().parse::<i32>().unwrap() == number {
                preview = Some(fs::read(file_path).await.unwrap());
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
