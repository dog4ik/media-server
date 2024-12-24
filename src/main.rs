#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use axum::routing::{delete, get, patch, post, put};
use axum::{Extension, Router};
use clap::Parser;
use dotenvy::dotenv;
use media_server::app_state::AppState;
use media_server::config::{self, AppResources, Args, ConfigFile, ConfigValue, APP_RESOURCES};
use media_server::db::Db;
use media_server::library::Library;
use media_server::metadata::metadata_stack::MetadataProvidersStack;
use media_server::metadata::tmdb_api::TmdbApi;
use media_server::metadata::tvdb_api::TvdbApi;
use media_server::progress::TaskResource;
use media_server::server::{server_api, torrent_api, OpenApiDoc};
use media_server::torrent::TorrentClient;
use media_server::torrent_index::tpb::TpbApi;
use media_server::tracing::{init_tracer, LogChannel};
use media_server::upnp::Upnp;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
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

    let db = Db::connect(&APP_RESOURCES.database_path)
        .await
        .expect("database to be found");

    let db = Box::leak(Box::new(db));
    let port: config::Port = config::CONFIG.get_value();
    let show_dirs: config::ShowFolders = config::CONFIG.get_value();
    let movie_dirs: config::MovieFolders = config::CONFIG.get_value();

    let library = Library::init_from_folders(show_dirs.0, movie_dirs.0, db).await;
    let library = Box::leak(Box::new(Mutex::new(library)));

    let mut providers_stack = MetadataProvidersStack::default();

    providers_stack.add_show_provider(db);
    providers_stack.add_movie_provider(db);
    providers_stack.add_discover_provider(db);

    let Some(tmdb_key) = config::CONFIG.get_value::<config::TmdbKey>().0 else {
        panic!("Missing tmdb api token, consider passing it in cli, configuration file or {} environment variable", config::TmdbKey::ENV_KEY.unwrap());
    };
    let tmdb_api = TmdbApi::new(tmdb_key);
    let tmdb_api = Box::leak(Box::new(tmdb_api));
    providers_stack.add_show_provider(tmdb_api);
    providers_stack.add_movie_provider(tmdb_api);
    providers_stack.add_discover_provider(tmdb_api);

    let tpb_api = TpbApi::new();
    let tpb_api = Box::leak(Box::new(tpb_api));

    providers_stack.add_torrent_provider(tpb_api);

    if let Some(tvdb_key) = config::CONFIG.get_value::<config::TvdbKey>().0 {
        let tvdb_api = TvdbApi::new(&tvdb_key);
        let tvdb_api = Box::leak(Box::new(tvdb_api));
        providers_stack.add_show_provider(tvdb_api);
        providers_stack.add_movie_provider(tvdb_api);
        providers_stack.add_discover_provider(tvdb_api);
    }

    let tasks = TaskResource::new(cancellation_token.clone());
    let tasks = Box::leak(Box::new(tasks));
    let tracker = tasks.tracker.clone();

    let torrent_client = TorrentClient::new(tasks, db.clone()).await.unwrap();
    torrent_client.load_torrents().await.unwrap();

    let torrent_client = Box::leak(Box::new(torrent_client));

    let providers_stack = Box::leak(Box::new(providers_stack));

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

    let server_api = Router::new()
        .route("/local_shows", get(server_api::all_local_shows))
        .route("/local_episode/:id", get(server_api::local_episode))
        .route(
            "/local_episode/by_video",
            get(server_api::local_episode_by_video_id),
        )
        .route(
            "/local_episode/:episode_id/watch",
            get(server_api::watch_episode),
        )
        .route(
            "/local_movie/by_video",
            get(server_api::local_movie_by_video_id),
        )
        .route("/local_movie/:movie_id/watch", get(server_api::watch_movie))
        .route("/local_movies", get(server_api::all_local_movies))
        .route(
            "/external_to_local/:id",
            get(server_api::external_to_local_id),
        )
        .route("/external_ids/:id", get(server_api::external_ids))
        .route("/movie/:movie_id", get(server_api::get_movie))
        .route("/movie/:movie_id", put(server_api::alter_movie_metadata))
        .route(
            "/movie/:movie_id/fix_metadata",
            post(server_api::fix_movie_metadata),
        )
        .route(
            "/movie/:movie_id/reset_metadata",
            post(server_api::reset_movie_metadata),
        )
        .route("/movie/:movie_id/poster", get(server_api::movie_poster))
        .route("/movie/:movie_id/backdrop", get(server_api::movie_backdrop))
        .route("/show/:show_id", get(server_api::get_show))
        .route("/show/:show_id", put(server_api::alter_show_metadata))
        .route(
            "/show/:show_id/fix_metadata",
            post(server_api::fix_show_metadata),
        )
        .route(
            "/show/:show_id/reset_metadata",
            post(server_api::reset_show_metadata),
        )
        .route("/show/:show_id/poster", get(server_api::show_poster))
        .route("/show/:show_id/backdrop", get(server_api::show_backdrop))
        .route("/show/:show_id/:season", get(server_api::get_season))
        .route(
            "/show/:show_id/:season/detect_intros",
            post(server_api::detect_intros),
        )
        .route("/season/:season_id/poster", get(server_api::season_poster))
        .route(
            "/show/:show_id/:season",
            put(server_api::alter_season_metadata),
        )
        .route(
            "/episode/:episode_id/poster",
            get(server_api::episode_poster),
        )
        .route(
            "/show/:show_id/:season/:episode",
            get(server_api::get_episode),
        )
        .route(
            "/show/:show_id/:season/:episode",
            put(server_api::alter_episode_metadata),
        )
        .route(
            "/show/:show_id/:season/:episode/poster",
            get(server_api::episode_poster),
        )
        .route("/variants", get(server_api::get_all_variants))
        .route("/video/by_content", get(server_api::contents_video))
        .route("/video/:id", get(server_api::get_video_by_id))
        .route("/video/:id", delete(server_api::remove_video))
        .route(
            "/video/:id/metadata",
            get(server_api::video_content_metadata),
        )
        .route(
            "/video/:id/pull_subtitle",
            get(server_api::pull_video_subtitle),
        )
        .route("/video/:id/previews/:number", get(server_api::previews))
        .route("/video/:id/previews", post(server_api::generate_previews))
        .route("/video/:id/previews", delete(server_api::delete_previews))
        .route(
            "/video/:id/history",
            delete(server_api::remove_video_history),
        )
        .route("/video/:id/history", put(server_api::update_video_history))
        .route("/video/:id/transcode", post(server_api::transcode_video))
        .route(
            "/video/:id/stream_transcode",
            post(server_api::create_transcode_stream),
        )
        .route("/video/:id/watch", get(server_api::watch))
        .route(
            "/video/:id/variant/:variant_id",
            delete(server_api::remove_variant),
        )
        .route("/history", get(server_api::all_history))
        .route("/history", delete(server_api::clear_history))
        .route("/history/suggest/movies", get(server_api::suggest_movies))
        .route("/history/suggest/shows", get(server_api::suggest_shows))
        .route("/history/:id", get(server_api::video_history))
        .route("/history/:id", delete(server_api::remove_history_item))
        .route("/history/:id", put(server_api::update_history))
        .route("/torrent/search", get(server_api::search_torrent))
        .route(
            "/torrent/resolve_magnet_link",
            get(torrent_api::resolve_magnet_link),
        )
        .route(
            "/torrent/parse_torrent_file",
            post(torrent_api::parse_torrent_file),
        )
        .route("/torrent/open", post(torrent_api::open_torrent))
        .route("/torrent/all", get(torrent_api::all_torrents))
        .route(
            "/torrent/open_torrent_file",
            post(torrent_api::open_torrent_file),
        )
        .route("/torrent/:info_hash/state", get(torrent_api::torrent_state))
        .route(
            "/torrent/:info_hash/file_priority",
            post(torrent_api::set_file_priority),
        )
        .route("/torrent/updates", get(torrent_api::updates))
        .route("/torrent/:info_hash", delete(torrent_api::delete_torrent))
        .route("/search/content", get(server_api::search_content))
        .route(
            "/search/trending_shows",
            get(server_api::get_trending_shows),
        )
        .route(
            "/search/trending_movies",
            get(server_api::get_trending_movies),
        )
        .route("/configuration", get(server_api::server_configuration))
        .route(
            "/configuration/capabilities",
            get(server_api::server_capabilities),
        )
        .route(
            "/configuration",
            patch(server_api::update_server_configuration),
        )
        .route(
            "/configuration/reset",
            post(server_api::reset_server_configuration),
        )
        .route("/configuration/providers", put(server_api::order_providers))
        .route(
            "/configuration/providers",
            get(server_api::get_providers_order),
        )
        .route("/log/latest", get(server_api::latest_log))
        .route("/tasks/transcode", get(server_api::transcode_tasks))
        .route(
            "/tasks/transcode/:id",
            delete(server_api::cancel_transcode_task),
        )
        .route("/tasks/previews", get(server_api::previews_tasks))
        .route(
            "/tasks/previews/:id",
            delete(server_api::cancel_previews_task),
        )
        .route("/tasks/progress", get(server_api::progress))
        .route("/scan", post(server_api::reconciliate_lib))
        .route("/fix_metadata/:content_id", post(server_api::fix_metadata))
        .route(
            "/reset_metadata/:content_id",
            post(server_api::reset_metadata),
        )
        .route(
            "/transcode/:id/segment/:segment",
            get(server_api::transcoded_segment),
        )
        .route(
            "/transcode/:id/manifest",
            get(server_api::transcode_stream_manifest),
        )
        .route("/file_browser/root_dirs", get(server_api::root_dirs))
        .route(
            "/file_browser/browse/:key",
            get(server_api::browse_directory),
        )
        .route(
            "/file_browser/parent/:key",
            get(server_api::parent_directory),
        )
        .route("/clear_db", delete(server_api::clear_db));

    let debug_api = Router::new().route("/library", get(server_api::library_state));

    let web_ui_path: config::WebUiPath = config::CONFIG.get_value();

    let assets_service =
        ServeDir::new(&web_ui_path.0).fallback(ServeFile::new(web_ui_path.0.join("index.html")));

    let upnp = Upnp::init(app_state.clone()).await;

    let app = Router::new()
        .route("/api/log", get(LogChannel::into_sse_stream))
        .layer(Extension(log_channel))
        .nest("/api", server_api)
        .nest("/debug", debug_api)
        .nest_service("/", assets_service)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", OpenApiDoc::openapi()))
        .merge(upnp)
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
