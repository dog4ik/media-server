use axum::{routing::get, Router};
use media_server::serve_file;

#[tokio::main]
async fn main() {
    let app = Router::new().route("/video.mkv", get(serve_file));
    let addr = "127.0.0.1:5000".parse().unwrap();
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}
