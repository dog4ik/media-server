use warp::hyper::{Body, Response, StatusCode};

use crate::ShowFile;

pub async fn serve_subs(
    episode: ShowFile,
    lang: Option<String>,
) -> Result<Response<Body>, warp::Rejection> {
    println!("{:?}", lang);
    let subs = episode.get_subtitles(lang).await;
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
