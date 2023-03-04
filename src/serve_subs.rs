use std::path::PathBuf;

use tokio::fs;
use warp::hyper::{Body, Response, StatusCode};

pub async fn serve_subs(
    title: String,
    season: i32,
    episode: i32,
) -> Result<Response<Body>, warp::Rejection> {
    let title = title.replace("-", " ");
    let path = PathBuf::from(format!(
        "/home/dog4ik/Documents/dev/rust/media-server/resources/{}/{}/{}/subs",
        title, season, episode
    ));
    println!("{:?}", path);
    let mut subs_dir = fs::read_dir(path).await.unwrap();
    let mut subs: Option<String> = None;
    let lang: Option<String> = None;
    loop {
        if let Some(file) = subs_dir.next_entry().await.unwrap() {
            let file_path = file.path();
            let file_name = file_path.file_stem().unwrap().to_str().unwrap();

            subs = match &lang {
                Some(lang) => {
                    if file_name == lang {
                        Some(fs::read_to_string(file.path()).await.unwrap())
                    } else {
                        continue;
                    }
                }
                None => {
                    if &file_name == &"unknown" || &file_name == &"eng" {
                        Some(fs::read_to_string(file_path).await.unwrap())
                    } else {
                        continue;
                    }
                }
            };
        } else {
            break;
        }
    }
    match subs {
        Some(subs) => Ok(Response::builder()
            .status(StatusCode::OK)
            .body(Body::from(subs))
            .unwrap()),
        None => Ok(Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(Body::empty())
            .unwrap()),
    }
}
