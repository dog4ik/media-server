use axum::extract::{Path, Query, State};
use axum::headers::{ContentType, Range};
use axum::http::Request;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Router, TypedHeader};
use bytes::Bytes;
use dotenvy::dotenv;
use media_server::show_file::{FileExtractor, PreviewQuery, ShowParams};
use media_server::Library;
use reqwest::StatusCode;
use std::{path::PathBuf, str::FromStr};

async fn path_middleware<B>(
    Path(params): Path<ShowParams>,
    State(state): State<&'static Library>,
    mut request: Request<B>,
    next: Next<B>,
) -> Response {
    request.extensions_mut().insert(Path(params));
    request.extensions_mut().insert(State(state));
    let response = next.run(request).await;
    response
}

async fn get_library(State(state): State<&'static Library>) -> String {
    return state.get_summary();
}

async fn previews(
    Query(query): Query<PreviewQuery>,
    FileExtractor(file): FileExtractor,
) -> Result<(TypedHeader<ContentType>, Bytes), StatusCode> {
    return file.serve_previews(query.number).await;
}

async fn video(
    TypedHeader(range): TypedHeader<Range>,
    FileExtractor(file): FileExtractor,
) -> impl IntoResponse {
    return file.serve_video(range).await;
}

async fn subtitles(FileExtractor(file): FileExtractor) -> Result<String, StatusCode> {
    return file.get_subtitles(None).await.ok_or(StatusCode::NO_CONTENT);
}

#[tokio::main]
async fn main() {
    dotenv().expect("env to load");
    let library_dir = std::env::var("LIBRARY_PATH").unwrap();
    let dirs = vec![PathBuf::from_str(&library_dir).unwrap()];
    let library: &'static Library = Box::leak(Box::new(Library::new(dirs).await));

    let app = Router::new()
        .route("/subs/:show_name/:season/:episode", get(subtitles))
        .route("/previews/:show_name/:season/:episode", get(previews))
        .route("/:show_name/:season/:episode", get(video))
        .layer(axum::middleware::from_fn_with_state(
            library,
            path_middleware,
        ))
        .route("/summary", get(get_library))
        // .route("/save-poster", post())
        // .route("/save-backdrop", post())
        // .route("/:show/backdrop", get())
        // .route("/:show/poster", get())
        // .route("/:show/:season/poster", get())
        // .route("/:show/:season/:episode/poster", get())
        .with_state(library);

    axum::Server::bind(&"127.0.0.1:6969".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}
