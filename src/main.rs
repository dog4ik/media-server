use axum::routing::{delete, get, post, put};
use axum::{Extension, Router};
use clap::Parser;
use dotenvy::dotenv;
use media_server::app_state::AppState;
use media_server::config::{AppResources, Args, ConfigFile, ServerConfiguration, APP_RESOURCES};
use media_server::db::Db;
use media_server::library::{explore_folder, Library, MediaFolders};
use media_server::metadata::tmdb_api::TmdbApi;
use media_server::metadata::MetadataProvidersStack;
use media_server::progress::TaskResource;
use media_server::server::{admin_api, public_api};
use media_server::torrent_index::tpb::TpbApi;
use media_server::tracing::{init_tracer, LogChannel};
use media_server::watch::monitor_library;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
use torrent::ClientConfig;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tracing::Level;

#[tokio::main]
async fn main() {
    let log_channel = init_tracer(Level::TRACE);
    if let Ok(path) = dotenv() {
        tracing::info!("Loaded env variables from: {}", path.display());
    } else {
        tracing::warn!("Could not load env variables from dotfile");
    }

    let cancellation_token = CancellationToken::new();

    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_origin(Any)
        .allow_headers(Any);

    let database_url = std::env::var("DATABASE_URL").unwrap();
    let db = Db::connect(&database_url)
        .await
        .expect("database to be found");
    let db = Box::leak(Box::new(db));

    let torrent_config = ClientConfig::default();
    let torrent_client = torrent::Client::new(torrent_config).await.unwrap();

    let args = Args::parse();

    let config_path = args
        .config_path
        .clone()
        .unwrap_or(AppResources::default_config_path());
    tracing::debug!("Selected config path: {}", &config_path.display());
    let config = ConfigFile::open(config_path).unwrap();
    let mut configuration = ServerConfiguration::new(config).unwrap();
    configuration.apply_args(args);
    if let Err(err) = configuration.resources.initiate() {
        tracing::error!("Failed to initiate resources {}", err);
        panic!("Could not initate app resources");
    };
    APP_RESOURCES
        .set(configuration.resources.clone())
        .expect("resources are not initiated yet");

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
    let tmdb_api = TmdbApi::new(std::env::var("TMDB_TOKEN").unwrap());
    let tmdb_api = Box::leak(Box::new(tmdb_api));
    let tpb_api = TpbApi::new();
    let tpb_api = Box::leak(Box::new(tpb_api));
    let torrent_client = Box::leak(Box::new(torrent_client));

    let providers_stack = MetadataProvidersStack {
        discover_providers_stack: Mutex::new(vec![db, tmdb_api]),
        show_providers_stack: Mutex::new(vec![db, tmdb_api]),
        movie_providers_stack: Mutex::new(vec![db, tmdb_api]),
        torrent_indexes_stack: Mutex::new(vec![tpb_api]),
    };

    let providers_stack = Box::leak(Box::new(providers_stack));

    let tasks = TaskResource::new(cancellation_token.clone());
    let tracker = tasks.tracker.clone();

    let app_state = AppState {
        library,
        configuration,
        db,
        tasks,
        tmdb_api,
        tpb_api,
        providers_stack,
        torrent_client,
        cancelation_token: cancellation_token.clone(),
    };

    #[cfg(feature = "windows-tray")]
    tokio::spawn(media_server::tray::spawn_tray_icon(app_state.clone()));
    monitor_library(app_state.clone(), media_folders).await;

    let app = Router::new()
        .route("/admin/log", get(LogChannel::into_sse_stream))
        .layer(Extension(log_channel))
        .route("/summary", get(public_api::get_summary))
        .route("/api/watch", get(public_api::watch))
        .route("/api/local_shows", get(public_api::all_local_shows))
        .route(
            "/api/external_to_local/:id",
            get(public_api::external_to_local_id),
        )
        .route("/api/external_ids/:id", get(public_api::external_ids))
        .route("/api/previews", get(public_api::previews))
        .route("/api/subs", get(public_api::subtitles))
        .route("/api/show/:show_id", get(public_api::get_show))
        .route("/api/show/:show_id/:season", get(public_api::get_season))
        .route(
            "/api/show/:show_id/:season/:episode",
            get(public_api::get_episode),
        )
        .route("/api/variants", get(public_api::get_all_variants))
        .route("/api/video/:id", get(public_api::get_video_by_id))
        .route("/api/contents_video/:id", get(public_api::contents_video))
        .route("/api/search_torrent", get(public_api::search_torrent))
        .route("/api/search_content", get(public_api::search_content))
        .route(
            "/admin/alter_show_metadata",
            put(admin_api::alter_show_metadata),
        )
        .route(
            "/admin/alter_season_metadata",
            put(admin_api::alter_season_metadata),
        )
        .route(
            "/admin/alter_episode_metadata",
            put(admin_api::alter_episode_metadata),
        )
        .route(
            "/admin/alter_movie_metadata",
            put(admin_api::alter_movie_metadata),
        )
        .route("/admin/latest_log", get(admin_api::latest_log))
        .route("/admin/progress", get(admin_api::progress))
        .route("/admin/tasks", get(admin_api::get_tasks))
        .route("/admin/mock_progress", post(admin_api::mock_progress))
        .route("/admin/cancel_task", post(admin_api::cancel_task))
        .route("/admin/scan", post(admin_api::reconciliate_lib))
        .route("/admin/clear_db", delete(admin_api::clear_db))
        .route("/admin/remove_video", delete(admin_api::remove_video))
        .route("/admin/remove_variant", delete(admin_api::remove_variant))
        .route("/admin/transcode", post(admin_api::transcode_video))
        .route("/admin/configuration", get(admin_api::server_configuration))
        .route("/admin/download_torrent", post(admin_api::download_torrent))
        .nest_service(
            "/",
            ServeDir::new("dist").fallback(ServeFile::new("dist/index.html")),
        )
        .layer(cors)
        .with_state(app_state);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    tracing::info!("Starting server on port {}", port);

    {
        let cancellation_token = cancellation_token.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(cancellation_token.cancelled_owned())
                .await
                .unwrap();
        });
    }
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            cancellation_token.cancel();
        }
        _ = cancellation_token.cancelled() => {}
    }
    tracing::trace!("Waiting all tasks to finish");
    tracker.close();
    tracker.wait().await;
    tracing::info!("Gracefully shutted down");
}
