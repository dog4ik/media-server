use axum::routing::{delete, get, patch, post, put};
use axum::{Extension, Router};
use clap::Parser;
use dotenvy::dotenv;
use media_server::app_state::AppState;
use media_server::config::{self, AppResources, Args, ConfigFile, ConfigValue, APP_RESOURCES};
use media_server::db::Db;
use media_server::library::Library;
use media_server::metadata::tmdb_api::TmdbApi;
use media_server::metadata::MetadataProvidersStack;
use media_server::progress::TaskResource;
use media_server::server::{admin_api, public_api, torrent_api, OpenApiDoc};
use media_server::torrent::TorrentClient;
use media_server::torrent_index::tpb::TpbApi;
use media_server::tracing::{init_tracer, LogChannel};
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
    Args::parse().apply_configuration();
    if let Err(err) = AppResources::initiate() {
        panic!("Could not initiate app resources: {err}");
    };
    let log_channel = init_tracer();
    tracing::info!("Using log file location: {}", AppResources::log().display());

    if let Ok(path) = dotenv() {
        tracing::info!("Loaded env variables from: {}", path.display());
    } else {
        tracing::warn!("Could not load env variables from dotfile");
    }

    match ConfigFile::open_and_read().await {
        Ok(toml) => config::CONFIG.apply_toml_settings(toml),
        Err(err) => tracing::error!("Error reading config file: {err}"),
    };

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

    let db = Db::connect(&APP_RESOURCES.database_path)
        .await
        .expect("database to be found");

    let db = Box::leak(Box::new(db));
    let port: config::Port = config::CONFIG.get_value();
    let show_dirs: config::ShowFolders = config::CONFIG.get_value();
    let movie_dirs: config::MovieFolders = config::CONFIG.get_value();

    let shows_dirs: Vec<PathBuf> = show_dirs
        .0
        .into_iter()
        .filter(|d| d.try_exists().unwrap_or(false))
        .collect();
    let movies_dirs: Vec<PathBuf> = movie_dirs
        .0
        .into_iter()
        .filter(|d| d.try_exists().unwrap_or(false))
        .collect();

    let library = Library::init_from_folders(&shows_dirs, &movies_dirs, db).await;
    let library = Box::leak(Box::new(Mutex::new(library)));
    let Some(tmdb_key) = config::CONFIG.get_value::<config::TmdbKey>().0 else {
        panic!("Missing tmdb api token, consider passing it in cli, configuration file or {} environment variable", config::TmdbKey::ENV_KEY.unwrap());
    };
    let tmdb_api = TmdbApi::new(tmdb_key);
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
    // tokio::spawn(watch::monitor_config(app_state.configuration, config_path));

    let public_api = Router::new()
        .route("/local_shows", get(public_api::all_local_shows))
        .route("/local_episode/:id", get(public_api::local_episode))
        .route(
            "/local_episode/by_video",
            get(public_api::local_episode_by_video_id),
        )
        .route(
            "/local_episode/:episode_id/watch",
            get(public_api::watch_episode),
        )
        .route(
            "/local_movie/by_video",
            get(public_api::local_movie_by_video_id),
        )
        .route("/local_movie/:movie_id/watch", get(public_api::watch_movie))
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
            "/show/:show_id/:season/detect_intros",
            post(admin_api::detect_intros),
        )
        .route("/season/:season_id/poster", get(public_api::season_poster))
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
            "/video/:id/metadata",
            get(public_api::video_content_metadata),
        )
        .route(
            "/video/:id/pull_subtitle",
            get(public_api::pull_video_subtitle),
        )
        .route("/video/:id/previews/:number", get(public_api::previews))
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
        .route("/torrent/all", get(torrent_api::all_torrents))
        .route("/search/content", get(public_api::search_content))
        .route("/configuration", get(admin_api::server_configuration))
        .route(
            "/configuration/capabilities",
            get(admin_api::server_capabilities),
        )
        .route(
            "/configuration",
            patch(admin_api::update_server_configuration),
        )
        .route(
            "/configuration/reset",
            post(admin_api::reset_server_configuration),
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
        .route("/file_browser/root_dirs", get(admin_api::root_dirs))
        .route(
            "/file_browser/browse/:key",
            get(admin_api::browse_directory),
        )
        .route(
            "/file_browser/parent/:key",
            get(admin_api::parent_directory),
        )
        .route("/clear_db", delete(admin_api::clear_db));

    let web_ui_path: config::WebUiPath = config::CONFIG.get_value();

    let assets_service =
        ServeDir::new(&web_ui_path.0).fallback(ServeFile::new(web_ui_path.0.join("index.html")));
    let app = Router::new()
        .route("/api/log", get(LogChannel::into_sse_stream))
        .layer(Extension(log_channel))
        .nest("/api", public_api)
        .nest_service("/", assets_service)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", OpenApiDoc::openapi()))
        .layer(cors)
        .with_state(app_state);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), port.0);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!("Failed to start server on port {}: {e}", port.0);
            return;
        }
    };
    tracing::info!("Starting server on port {}", port.0);

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
    tracing::info!("Gracefully shut down");
}
