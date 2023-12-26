use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use clap::Parser;
use dotenvy::dotenv;
use media_server::app_state::AppState;
use media_server::config::{Args, ServerConfiguration};
use media_server::db::Db;
use media_server::library::{explore_folder, Library, MediaFolders};
use media_server::progress::TaskResource;
use media_server::server::{admin_api, public_api};
use media_server::tracing::{init_tracer, LogChannel};
use media_server::watch::monitor_library;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, Level};

#[tokio::main]
async fn main() {
    let log_channel = init_tracer(Level::TRACE);
    if let Ok(path) = dotenv() {
        info!("Loaded env variables from: {}", path.display());
    }

    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_origin(Any)
        .allow_headers(Any);

    let database_url = std::env::var("DATABASE_URL").unwrap();
    let db = Db::connect(&database_url)
        .await
        .expect("database to be found");

    let args = Args::parse();
    let mut configuration = ServerConfiguration::from_file("server-configuration.json").unwrap();
    configuration.apply_args(args);
    let port = configuration.port;

    let shows_dirs: Vec<PathBuf> = configuration
        .show_folders
        .clone()
        .into_iter()
        .filter(|d| d.try_exists().unwrap_or(false))
        .collect();
    let mut shows = Vec::new();
    for dir in &shows_dirs {
        shows.extend(explore_folder(dir).await.unwrap());
    }

    let movies_dirs: Vec<PathBuf> = configuration
        .movie_folders
        .clone()
        .into_iter()
        .filter(|d| d.try_exists().unwrap_or(false))
        .collect();
    let mut movies = Vec::new();
    for dir in &movies_dirs {
        movies.extend(explore_folder(dir).await.unwrap());
    }

    let media_folders = MediaFolders {
        shows: shows_dirs,
        movies: movies_dirs,
    };

    let library = Library::new(media_folders.clone(), shows, movies);
    let library = Box::leak(Box::new(Mutex::new(library)));
    let configuration = Box::leak(Box::new(Mutex::new(configuration)));

    let tasks = TaskResource::new();

    let app_state = AppState {
        library,
        configuration,
        db,
        tasks,
    };

    monitor_library(app_state.clone(), media_folders).await;

    let app = Router::new()
        .route("/admin/log", get(LogChannel::into_sse_stream))
        .layer(Extension(log_channel))
        .route("/summary", get(public_api::get_summary))
        .route("/watch", get(public_api::watch))
        .route("/watch/variant", get(public_api::watch_variant))
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
            "/admin/alter_show_metadata",
            post(admin_api::alter_show_metadata),
        )
        .route(
            "/admin/alter_season_metadata",
            post(admin_api::alter_season_metadata),
        )
        .route(
            "/admin/alter_episode_metadata",
            post(admin_api::alter_episode_metadata),
        )
        .route(
            "/admin/alter_movie_metadata",
            post(admin_api::alter_movie_metadata),
        )
        .route("/admin/latest_log", get(admin_api::latest_log))
        .route("/admin/progress", get(admin_api::progress))
        .route("/admin/get_tasks", get(admin_api::get_tasks))
        .route("/admin/mock_progress", post(admin_api::mock_progress))
        .route("/admin/cancel_task", post(admin_api::cancel_task))
        .route("/admin/scan", post(admin_api::reconciliate_lib))
        .route("/admin/clear_db", delete(admin_api::clear_db))
        .route("/admin/remove_video", delete(admin_api::remove_video))
        .route("/admin/configuration", get(admin_api::server_configuration))
        .layer(cors)
        .with_state(app_state);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    info!("Starting server on port {}", port);
    axum::serve(listener, app).await.unwrap();
}
