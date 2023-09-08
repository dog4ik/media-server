use axum::extract::{Query, State};
use axum::headers::{ContentType, Range};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Router, TypedHeader};
use bytes::Bytes;
use dotenvy::dotenv;
use media_server::library::{LibraryFile, LibraryFileExtractor, PreviewQuery};
use media_server::movie_file::movie_path_middleware;
use media_server::scan::{transcode, walk_recursive};
use media_server::serve_content::ServeContent;
use media_server::show_file::show_path_middleware;
use media_server::Library;
use reqwest::StatusCode;
use std::path::PathBuf;

async fn get_library(State(state): State<&'static Library>) -> String {
    return state.get_summary();
}

async fn serve_previews(
    Query(query): Query<PreviewQuery>,
    LibraryFileExtractor(file): LibraryFileExtractor,
) -> Result<(TypedHeader<ContentType>, Bytes), StatusCode> {
    return match file {
        LibraryFile::Show(show) => show.serve_previews(query.number).await,
        LibraryFile::Movie(movie) => movie.serve_previews(query.number).await,
    };
}

async fn serve_video(
    TypedHeader(range): TypedHeader<Range>,
    LibraryFileExtractor(file): LibraryFileExtractor,
) -> impl IntoResponse {
    return match file {
        LibraryFile::Show(show) => show.serve_video(range).await,
        LibraryFile::Movie(movie) => movie.serve_video(range).await,
    };
}

async fn serve_subtitles(
    LibraryFileExtractor(file): LibraryFileExtractor,
) -> Result<String, StatusCode> {
    return match file {
        LibraryFile::Show(show) => show.serve_subs(None).await,
        LibraryFile::Movie(movie) => movie.serve_subs(None).await,
    };
}

#[tokio::main]
async fn main() {
    dotenv().expect("env to load");

    let movies_dir = PathBuf::from(std::env::var("MOVIES_PATH").unwrap());
    let shows_dir = PathBuf::from(std::env::var("SHOWS_PATH").unwrap());

    if !movies_dir.exists() || !shows_dir.exists() {
        panic!("paths in env file are not found on this machine");
    }

    let shows = walk_recursive(&shows_dir).unwrap();
    let movies = walk_recursive(&movies_dir).unwrap();

    let library = Library { movies, shows };
    let library: &'static Library = Box::leak(Box::new(library));

    transcode(&library.shows).await;
    transcode(&library.movies).await;

    let app = Router::new()
        .route("/movie/subs/:movie_name", get(serve_subtitles))
        .route("/movie/previews/:movie_name", get(serve_previews))
        .route("/movie/:movie_name", get(serve_video))
        .layer(axum::middleware::from_fn_with_state(
            library,
            movie_path_middleware,
        ))
        .route(
            "/show/subs/:show_name/:season/:episode",
            get(serve_subtitles),
        )
        .route(
            "/show/previews/:show_name/:season/:episode",
            get(serve_previews),
        )
        .route("/show/:show_name/:season/:episode", get(serve_video))
        .layer(axum::middleware::from_fn_with_state(
            library,
            show_path_middleware,
        ))
        .route("/summary", get(get_library))
        .with_state(library);

    axum::Server::bind(&"127.0.0.1:6969".parse().unwrap())
        .serve(app.into_make_service())
        .await
        .unwrap();
}
