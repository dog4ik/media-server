use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use axum_extra::headers::{ContentType, Range};
use axum_extra::TypedHeader;
use bytes::Bytes;
use dotenvy::dotenv;
use media_server::admin_api;
use media_server::app_state::AppState;
use media_server::db::Db;
use media_server::library::{LibraryFile, LibraryFileExtractor, MediaFolders, PreviewQuery};
use media_server::movie_file::movie_path_middleware;
use media_server::progress::TaskResource;
use media_server::public_api;
use media_server::scan::{read_library_items, Library, Summary};
use media_server::serve_content::ServeContent;
use media_server::show_file::show_path_middleware;
use media_server::tracing::{init_tracer, LogChannel};
use media_server::watch::monitor_library;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, Level};

async fn get_library(State(library): State<Arc<Mutex<Library>>>) -> Json<Vec<Summary>> {
    let library = library.lock().await;
    return Json(library.get_summary());
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

const PORT: u16 = 6969;

#[tokio::main]
async fn main() {
    let log_channel = init_tracer(Level::TRACE);
    if let Ok(path) = dotenv() {
        info!("Loaded env variables from: {:?}", path);
    }

    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_origin(Any)
        .allow_headers(Any);

    let database_url = std::env::var("DATABASE_URL").unwrap();
    let db = Db::connect(&database_url)
        .await
        .expect("database to be found");

    let movies_dir = PathBuf::from(std::env::var("MOVIES_PATH").unwrap());
    let shows_dir = PathBuf::from(std::env::var("SHOWS_PATH").unwrap());

    if !movies_dir.try_exists().unwrap_or(false) || !shows_dir.try_exists().unwrap_or(false) {
        panic!("one or more library paths does not exists");
    }

    let shows = explore_folder(&shows_dir).await.unwrap();
    let movies = explore_folder(&movies_dir).await.unwrap();

    let media_folders = MediaFolders {
        shows: vec![shows_dir],
        movies: vec![movies_dir],
    };

    let library = Library::new(media_folders.clone(), shows, movies);
    let library = Arc::new(Mutex::new(library));

    let tasks = TaskResource::new();

    let app_state = AppState {
        library: library.clone(),
        db,
        tasks,
    };

    // transcode(&library.shows).await;
    // transcode(&library.movies).await;

    monitor_library(app_state.clone(), media_folders).await;

    let app = Router::new()
        .route("/movie/subs/:movie_name", get(serve_subtitles))
        .route("/movie/previews/:movie_name", get(serve_previews))
        .route("/movie/:movie_name", get(serve_video))
        .layer(axum::middleware::from_fn_with_state(
            library.clone(),
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
        .route("/admin/log", get(LogChannel::into_sse_stream))
        .layer(Extension(log_channel))
        .route("/summary", get(get_library))
        .route("/api/get_all_shows", get(public_api::get_all_shows))
        .route("/api/watch", get(public_api::watch))
        .route("/api/previews", get(public_api::previews))
        .route("/api/subs", get(public_api::subtitles))
        .route("/api/get_show_by_id", get(public_api::get_show_by_id))
        .route("/api/get_seasons", get(public_api::get_seasons))
        .route("/api/get_season", get(public_api::get_season))
        .route("/api/get_season_by_id", get(public_api::get_season_by_id))
        .route("/api/get_episodes", get(public_api::get_episodes))
        .route("/api/get_episode", get(public_api::get_episode))
        .route("/api/get_episode_by_id", get(public_api::get_episode_by_id))
        .route("/api/get_video_by_id", get(public_api::get_video_by_id))
        .route(
            "/admin/refresh_show_metadata",
            post(admin_api::refresh_show_metadata),
        )
        .route("/admin/latest_log", get(admin_api::latest_log))
        .route("/admin/progress", get(admin_api::progress))
        .route("/admin/get_tasks", get(admin_api::get_tasks))
        .route("/admin/mock_progress", post(admin_api::mock_progress))
        .route("/admin/cancel_task", post(admin_api::cancel_task))
        .route("/admin/scan", get(admin_api::reconciliate_lib))
        .route("/admin/clear_db", get(admin_api::clear_db))
        .route("/admin/remove_video", delete(admin_api::remove_video))
        .layer(cors)
        .with_state(app_state);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), PORT);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    info!("Starting server on port {}", PORT);
    axum::serve(listener, app).await.unwrap();
}
