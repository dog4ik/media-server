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
use media_server::server::{admin_api, public_api, OpenApiDoc};
use media_server::torrent::TorrentClient;
use media_server::torrent_index::tpb::TpbApi;
use media_server::tracing::{init_tracer, LogChannel};
use media_server::watch;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
use torrent::ClientConfig;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[tokio::main]
async fn main() {
    if let Err(err) = AppResources::initiate() {
        tracing::error!("Failed to initiate resources {}", err);
        panic!("Could not initate app resources");
    };
    let log_channel = init_tracer();
    tracing::info!("Selected log location: {}", AppResources::log().display());

    if let Ok(path) = dotenv() {
        tracing::info!("Loaded env variables from: {}", path.display());
    } else {
        tracing::warn!("Could not load env variables from dotfile");
    }

    let args = Args::parse();
    let config_path = args
        .config_path
        .clone()
        .unwrap_or(AppResources::default_config_path());
    tracing::debug!("Selected config path: {}", &config_path.display());
    let config = ConfigFile::open(&config_path).unwrap();
    let mut configuration = ServerConfiguration::new(config).unwrap();
    configuration.apply_args(args);
    APP_RESOURCES
        .set(configuration.resources.clone())
        .expect("resources are not initiated yet");

    let cancellation_token = CancellationToken::new();

    let cors = CorsLayer::new()
        .allow_methods(Any)
        .allow_origin(Any)
        .allow_headers(Any);

    let torrent_config = ClientConfig {
        cancellation_token: Some(cancellation_token.child_token()),
        ..Default::default()
    };
    let torrent_client = TorrentClient::new(torrent_config).await.unwrap();

    let db = Db::connect(configuration.resources.database_path.clone())
        .await
        .expect("database to be found");

    let db = Box::leak(Box::new(db));
    let port = configuration.port;

    let shows_dirs: Vec<PathBuf> = configuration
        .show_folders
        .clone()
        .into_iter()
        .filter(|d| d.try_exists().unwrap_or(false))
        .collect();
    let mut shows = HashMap::new();
    for dir in &shows_dirs {
        shows.extend(explore_folder(dir, &db, &Vec::new()).await.unwrap());
    }

    let movies_dirs: Vec<PathBuf> = configuration
        .movie_folders
        .clone()
        .into_iter()
        .filter(|d| d.try_exists().unwrap_or(false))
        .collect();
    let mut movies = HashMap::new();
    for dir in &movies_dirs {
        movies.extend(explore_folder(dir, &db, &Vec::new()).await.unwrap());
    }

    let media_folders = MediaFolders {
        shows: shows_dirs,
        movies: movies_dirs,
    };

    let program_files = configuration.resources.base_path.clone();

    let library = Library::new(media_folders.clone(), shows, movies);
    let library = Box::leak(Box::new(Mutex::new(library)));
    let tmdb_api = TmdbApi::new(configuration.tmdb_token.clone().unwrap());
    let configuration = Box::leak(Box::new(Mutex::new(configuration)));
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
    let tasks = Box::leak(Box::new(tasks));
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
    // tokio::spawn(watch::monitor_library(app_state.clone(), media_folders));
    tokio::spawn(watch::monitor_config(app_state.configuration, config_path));

    let public_api = Router::new()
        .route("/local_shows", get(public_api::all_local_shows))
        .route("/local_episode/:id", get(public_api::local_episode))
        .route(
            "/local_episode/by_video",
            get(public_api::local_episode_by_video_id),
        )
        .route(
            "/local_movie/by_video",
            get(public_api::local_movie_by_video_id),
        )
        .route("/local_movies", get(public_api::all_local_movies))
        .route(
            "/external_to_local/:id",
            get(public_api::external_to_local_id),
        )
        .route("/external_ids/:id", get(public_api::external_ids))
        .route("/movie/:movie_id", get(public_api::get_movie))
        .route("/movie/:movie_id", put(admin_api::alter_movie_metadata))
        .route(
            "/movie/:movie_id/fix_metadata",
            post(admin_api::fix_movie_metadata),
        )
        .route(
            "/movie/:movie_id/reset_metadata",
            post(admin_api::reset_movie_metadata),
        )
        .route("/movie/:movie_id/poster", get(public_api::movie_poster))
        .route("/movie/:movie_id/backdrop", get(public_api::movie_backdrop))
        .route("/show/:show_id", get(public_api::get_show))
        .route("/show/:show_id", put(admin_api::alter_show_metadata))
        .route(
            "/show/:show_id/fix_metadata",
            post(admin_api::fix_show_metadata),
        )
        .route(
            "/show/:show_id/reset_metadata",
            post(admin_api::reset_show_metadata),
        )
        .route("/show/:show_id/poster", get(public_api::show_poster))
        .route("/show/:show_id/backdrop", get(public_api::show_backdrop))
        .route("/show/:show_id/:season", get(public_api::get_season))
        .route(
            "/show/:show_id/:season/poster",
            put(public_api::season_poster),
        )
        .route(
            "/show/:show_id/:season",
            put(admin_api::alter_season_metadata),
        )
        .route(
            "/episode/:episode_id/poster",
            get(public_api::episode_poster),
        )
        .route(
            "/show/:show_id/:season/:episode",
            get(public_api::get_episode),
        )
        .route(
            "/show/:show_id/:season/:episode",
            put(admin_api::alter_episode_metadata),
        )
        .route(
            "/show/:show_id/:season/:episode/poster",
            get(public_api::episode_poster),
        )
        .route("/variants", get(public_api::get_all_variants))
        .route("/video/by_content", get(public_api::contents_video))
        .route("/video/:id", get(public_api::get_video_by_id))
        .route("/video/:id", delete(admin_api::remove_video))
        .route(
            "/video/:id/pull_subtitle",
            get(public_api::pull_video_subtitle),
        )
        .route("/video/:id/previews", get(public_api::previews))
        .route("/video/:id/previews", post(admin_api::generate_previews))
        .route("/video/:id/previews", delete(admin_api::delete_previews))
        .route(
            "/video/:id/history",
            delete(admin_api::remove_video_history),
        )
        .route("/video/:id/history", put(admin_api::update_video_history))
        .route("/video/:id/transcode", post(admin_api::transcode_video))
        .route(
            "/video/:id/stream_transcode",
            post(admin_api::create_transcode_stream),
        )
        .route("/video/:id/watch", get(public_api::watch))
        .route(
            "/video/:id/variant/:variant_id",
            delete(admin_api::remove_variant),
        )
        .route("/history", get(public_api::all_history))
        .route("/history", delete(admin_api::clear_history))
        .route("/history/suggest/movies", get(public_api::suggest_movies))
        .route("/history/suggest/shows", get(public_api::suggest_shows))
        .route("/history/:id", get(public_api::video_history))
        .route("/history/:id", delete(admin_api::remove_history_item))
        .route("/history/:id", put(admin_api::update_history))
        .route("/torrent/search", get(public_api::search_torrent))
        .route(
            "/torrent/resolve_magnet_link",
            get(admin_api::resolve_magnet_link),
        )
        .route(
            "/torrent/parse_torrent_file",
            post(admin_api::parse_torrent_file),
        )
        .route("/torrent/download", post(admin_api::download_torrent))
        .route("/search/content", get(public_api::search_content))
        .route("/configuration", get(admin_api::server_configuration))
        .route(
            "/configuration",
            put(admin_api::update_server_configuration),
        )
        .route(
            "/configuration/reset",
            post(admin_api::reset_server_configuration),
        )
        .route(
            "/configuration/schema",
            get(admin_api::server_configuration_schema),
        )
        .route("/configuration/providers", put(admin_api::order_providers))
        .route("/configuration/providers", get(admin_api::providers_order))
        .route("/log/latest", get(admin_api::latest_log))
        .route("/tasks", get(admin_api::get_tasks))
        .route("/tasks/:id", delete(admin_api::cancel_task))
        .route("/tasks/progress", get(admin_api::progress))
        .route("/mock_progress", post(admin_api::mock_progress))
        .route("/scan", post(admin_api::reconciliate_lib))
        .route("/fix_metadata/:content_id", post(admin_api::fix_metadata))
        .route(
            "/reset_metadata/:content_id",
            post(admin_api::reset_metadata),
        )
        .route(
            "/transcode/:id/segment/:segment",
            get(admin_api::transcoded_segment),
        )
        .route(
            "/transcode/:id/manifest",
            get(admin_api::transcode_stream_manifest),
        )
        .route("/clear_db", delete(admin_api::clear_db));

    let assets_service = ServeDir::new(program_files.join("dist"))
        .fallback(ServeFile::new(program_files.join("dist/index.html")));
    let app = Router::new()
        .route("/api/log", get(LogChannel::into_sse_stream))
        .layer(Extension(log_channel))
        .nest("/api", public_api)
        .nest_service("/", assets_service)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", OpenApiDoc::openapi()))
        .layer(cors)
        .with_state(app_state);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port);
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
    torrent_client.client.shutdown().await;
    tracker.close();
    tracker.wait().await;
    tracing::info!("Gracefully shutted down");
}
