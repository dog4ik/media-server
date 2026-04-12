#![windows_subsystem = "windows"]
use axum::routing::{any, delete, get, patch, post, put};
use axum::{Extension, Router};
use clap::Parser;
use dotenvy::dotenv;
use media_server::api::{self, OpenApiDoc};
use media_server::app_state::AppState;
use media_server::config::{self, APP_RESOURCES, AppResources, Args, ConfigFile};
use media_server::db::Db;
use media_server::library::Library;
use media_server::metadata::metadata_stack::MetadataProvidersStack;
use media_server::metadata::tmdb_api::TmdbApi;
use media_server::metadata::tvdb_api::TvdbApi;
use media_server::progress::TaskResource;
use media_server::torrent::TorrentClient;
use media_server::torrent_index::rutracker::ProvodRuTrackerAdapter;
use media_server::torrent_index::tpb::TpbApi;
use media_server::tracing::{LogChannel, init_tracer};
use media_server::upnp::Upnp;
use media_server::{ffmpeg_abi, ws};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[tokio::main]
async fn main() {
    let server_start_time = std::time::Instant::now();
    ffmpeg_next::init().expect("ffmpeg abi to initiate");
    ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Panic);
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
        Err(err) => tracing::error!("Failed to read config file: {err}"),
    };

    tokio::spawn(ffmpeg_abi::get_or_init_gpu_accelated_apis());

    let cancellation_token = CancellationToken::new();

    let db = Db::connect(&APP_RESOURCES.database_path)
        .await
        .expect("database to be found");

    let db = Box::leak(Box::new(db));
    let config::Port(port) = config::CONFIG.get_value();
    let show_dirs: config::ShowFolders = config::CONFIG.get_value();
    let movie_dirs: config::MovieFolders = config::CONFIG.get_value();

    let library = Library::init_from_folders(show_dirs.0, movie_dirs.0, db).await;
    let library = Box::leak(Box::new(Mutex::new(library)));

    let mut providers_stack = MetadataProvidersStack::new();

    match TmdbApi::new(config::CONFIG.get_value::<config::TmdbKey>().0) {
        Ok(tmdb_api) => {
            let tmdb_api: &'static _ = Box::leak(Box::new(tmdb_api));
            providers_stack.tmdb = Some(tmdb_api);
        }
        Err(e) => tracing::warn!("Failed to initialize TMDB api: {e}"),
    };

    let tpb_api = TpbApi::new();
    let tpb_api = Box::leak(Box::new(tpb_api));
    providers_stack.tpb = Some(tpb_api);

    match ProvodRuTrackerAdapter::new() {
        Ok(rutracker_api) => {
            let rutracker_api: &'static _ = Box::leak(Box::new(rutracker_api));
            providers_stack.rutracker = Some(&rutracker_api);
        }
        Err(e) => tracing::warn!("Failed to initialize RuTracker api: {e}"),
    }

    match TvdbApi::new(config::CONFIG.get_value::<config::TvdbKey>().0.as_deref()) {
        Ok(tvdb_api) => {
            let tvdb_api: &'static _ = Box::leak(Box::new(tvdb_api));
            providers_stack.tvdb = Some(tvdb_api);
        }
        Err(e) => tracing::warn!("Failed to initialize TVDB api: {e}"),
    }
    providers_stack.apply_config_order();
    let providers_stack = Box::leak(Box::new(providers_stack));

    let tasks = TaskResource::new(cancellation_token.clone());
    let tasks = Box::leak(Box::new(tasks));
    let tracker = tasks.tracker.clone();

    let torrent_client = TorrentClient::new(tasks, db.clone()).await.unwrap();
    torrent_client.load_torrents().await.unwrap();

    let torrent_client = Box::leak(Box::new(torrent_client));

    let app_state = AppState {
        library,
        db,
        tasks,
        providers_stack,
        torrent_client,
        cancelation_token: cancellation_token.clone(),
    };

    #[cfg(feature = "windows-tray")]
    tokio::spawn(media_server::tray::spawn_tray_icon(app_state.clone()));
    // tokio::spawn(watch::monitor_library(app_state.clone(), media_folders));
    // tokio::spawn(watch::monitor_config(app_state.configuration, config_path));

    let server_api = Router::new()
        .route("/local_shows", get(api::server::all_local_shows))
        .route("/local_episode/{id}", get(api::server::local_episode))
        .route("/local_episode/{id}", delete(api::server::delete_episode))
        .route(
            "/local_episode/by_video",
            get(api::server::local_episode_by_video_id),
        )
        .route(
            "/local_episode/{episode_id}/watch",
            get(api::server::watch_episode),
        )
        .route(
            "/local_movie/by_video",
            get(api::server::local_movie_by_video_id),
        )
        .route("/local_movie/{id}", delete(api::server::delete_movie))
        .route(
            "/local_movie/{movie_id}/watch",
            get(api::server::watch_movie),
        )
        .route("/local_movies", get(api::server::all_local_movies))
        .route("/local_season/{id}", delete(api::server::delete_season))
        .route("/local_show/{id}", delete(api::server::delete_show))
        .route("/external_ids/{id}", get(api::server::external_ids))
        .route("/movie/{movie_id}", get(api::server::get_movie))
        .route("/movie/{movie_id}", put(api::server::alter_movie_metadata))
        .route(
            "/movie/{movie_id}/fix_metadata",
            post(api::server::fix_movie_metadata),
        )
        .route(
            "/movie/{movie_id}/reset_metadata",
            post(api::server::reset_movie_metadata),
        )
        .route("/movie/{movie_id}/poster", get(api::server::movie_poster))
        .route(
            "/movie/{movie_id}/backdrop",
            get(api::server::movie_backdrop),
        )
        .route("/show/{show_id}", get(api::server::get_show))
        .route("/show/{show_id}", put(api::server::alter_show_metadata))
        .route(
            "/show/{show_id}/fix_metadata",
            post(api::server::fix_show_metadata),
        )
        .route(
            "/show/{show_id}/reset_metadata",
            post(api::server::reset_show_metadata),
        )
        .route("/show/{show_id}/poster", get(api::server::show_poster))
        .route("/show/{show_id}/backdrop", get(api::server::show_backdrop))
        .route("/show/{show_id}/{season}", get(api::server::get_season))
        .route(
            "/show/{show_id}/{season}/detect_intros",
            post(api::server::detect_intros),
        )
        .route(
            "/season/{season_id}/poster",
            get(api::server::season_poster),
        )
        .route(
            "/season/{season_id}/intros",
            delete(api::server::delete_season_intros),
        )
        .route(
            "/show/{show_id}/{season}",
            put(api::server::alter_season_metadata),
        )
        .route(
            "/episode/{episode_id}/poster",
            get(api::server::episode_poster),
        )
        .route(
            "/episode/{episode_id}/intros",
            delete(api::server::delete_episode_intros),
        )
        .route(
            "/show/{show_id}/{season}/{episode}",
            get(api::server::get_episode),
        )
        .route(
            "/show/{show_id}/{season}/{episode}",
            put(api::server::alter_episode_metadata),
        )
        .route(
            "/show/{show_id}/{season}/{episode}/poster",
            get(api::server::episode_poster),
        )
        .route("/variants", get(api::server::get_all_variants))
        .route("/video/by_content", get(api::server::contents_video))
        .route("/video/{id}", get(api::server::get_video_by_id))
        .route("/video/{id}", delete(api::server::remove_video))
        .route("/video/{id}/intro", put(api::server::update_video_intro))
        .route("/video/{id}/intro", get(api::server::video_intro))
        .route("/video/{id}/intro", delete(api::server::delete_video_intro))
        .route(
            "/video/{id}/metadata",
            get(api::server::video_content_metadata),
        )
        .route(
            "/video/{id}/pull_subtitle",
            get(api::server::pull_video_subtitle),
        )
        .route("/video/{id}/previews/{number}", get(api::server::previews))
        .route("/video/{id}/previews", post(api::server::generate_previews))
        .route("/video/{id}/previews", delete(api::server::delete_previews))
        .route(
            "/video/{id}/history",
            delete(api::history::remove_video_history),
        )
        .route(
            "/video/{id}/history",
            put(api::history::update_video_history),
        )
        .route("/video/{id}/transcode", post(api::server::transcode_video))
        .route(
            "/watch/direct/start/{id}",
            post(api::server::start_direct_stream),
        )
        .route("/watch/hls/start/{id}", post(api::server::start_hls_stream))
        .route("/video/{id}/watch", get(api::server::watch))
        .route(
            "/video/{id}/upload_subtitles",
            post(api::server::upload_subtitles),
        )
        .route(
            "/video/{id}/reference_subtitles",
            post(api::server::reference_external_subtitles),
        )
        .route(
            "/video/{id}/variant/{variant_id}",
            delete(api::server::remove_variant),
        )
        .route("/history", get(api::history::all_history))
        .route("/history", delete(api::history::clear_history))
        .route("/history/suggest/movies", get(api::history::suggest_movies))
        .route("/history/suggest/shows", get(api::history::suggest_shows))
        .route("/history/{id}", delete(api::history::remove_history_item))
        .route("/history/{id}", put(api::history::update_history))
        .route("/actor/{id}/poster", get(api::server::actor_poster))
        .route("/actor/list", get(api::server::actor_list))
        .route("/subtitles/{id}", delete(api::server::delete_subtitles))
        .route("/subtitles/{id}", get(api::server::get_subtitles))
        .route("/torrent/search", get(api::server::search_torrent))
        .route(
            "/torrent/resolve_magnet_link",
            get(api::torrent::resolve_magnet_link),
        )
        .route(
            "/torrent/parse_torrent_file",
            post(api::torrent::parse_torrent_file),
        )
        .route("/torrent/open", post(api::torrent::open_torrent))
        .route("/torrent/all", get(api::torrent::all_torrents))
        .route("/torrent/session_state", get(api::torrent::session_state))
        .route(
            "/torrent/open_torrent_file",
            post(api::torrent::open_torrent_file),
        )
        .route(
            "/torrent/{info_hash}/state",
            get(api::torrent::torrent_state),
        )
        .route(
            "/torrent/{info_hash}/files_priority",
            post(api::torrent::set_files_priority),
        )
        .route("/torrent/updates", get(api::torrent::updates))
        .route("/torrent/{info_hash}", delete(api::torrent::delete_torrent))
        .route(
            "/torrent/{info_hash}/validate",
            post(api::torrent::validate_torrent),
        )
        .route(
            "/torrent/output_location",
            get(api::torrent::output_location),
        )
        .route(
            "/torrent/index_magnet_link",
            get(api::torrent::index_magnet_link),
        )
        .route("/torrent/batch_action", post(api::torrent::batch_action))
        .route("/search/content", get(api::server::search_content))
        .route(
            "/search/trending_shows",
            get(api::server::get_trending_shows),
        )
        .route(
            "/search/trending_movies",
            get(api::server::get_trending_movies),
        )
        .route("/configuration", get(api::server::server_configuration))
        .route("/version", get(api::server::server_version))
        .route(
            "/configuration/capabilities",
            get(api::server::server_capabilities),
        )
        .route(
            "/configuration",
            patch(api::server::update_server_configuration),
        )
        .route(
            "/configuration/reset",
            post(api::server::reset_server_configuration),
        )
        .route(
            "/configuration/providers",
            put(api::server::order_providers),
        )
        .route(
            "/configuration/providers",
            get(api::server::get_providers_order),
        )
        .route("/log/latest", get(api::server::latest_log))
        .route("/tasks/transcode", get(api::server::transcode_tasks))
        .route(
            "/tasks/transcode/{id}",
            delete(api::server::cancel_transcode_task),
        )
        .route("/tasks/previews", get(api::server::previews_tasks))
        .route(
            "/tasks/previews/{id}",
            delete(api::server::cancel_previews_task),
        )
        .route("/tasks/watch_sessions", get(api::server::watch_sessions))
        .route(
            "/tasks/watch_session/{id}",
            delete(api::server::stop_watch_session),
        )
        .route(
            "/tasks/intro_detection",
            get(api::server::intro_detection_tasks),
        )
        .route("/tasks/progress", get(api::server::progress))
        .route("/ws", any(ws::ws))
        .route("/scan", post(api::server::reconciliate_lib))
        .route(
            "/fix_metadata/{content_id}",
            post(api::server::fix_metadata),
        )
        .route(
            "/reset_metadata/{content_id}",
            post(api::server::reset_metadata),
        )
        .route(
            "/watch/hls/{id}/segment/{segment}",
            get(api::server::hls_segment),
        )
        .route("/watch/hls/{id}/manifest", get(api::server::hls_manifest))
        .route("/watch/hls/{id}/init", get(api::server::hls_init))
        .route("/file_browser/root_dirs", get(api::file_browser::root_dirs))
        .route(
            "/file_browser/browse/{key}",
            get(api::file_browser::browse_directory),
        )
        .route(
            "/file_browser/parent/{key}",
            get(api::file_browser::parent_directory),
        )
        .route("/clear_db", delete(api::server::clear_db));

    let debug_api = Router::new().route("/library", get(api::server::library_state));

    let web_ui_path: config::WebUiPath = config::CONFIG.get_value();

    let assets_service =
        ServeDir::new(&web_ui_path.0).fallback(ServeFile::new(web_ui_path.0.join("index.html")));

    let upnp = Upnp::init(app_state.clone()).await;

    let http_trace = tower_http::trace::TraceLayer::new_for_http();
    let app = Router::new()
        .route("/api/log", get(LogChannel::into_sse_stream))
        .layer(Extension(log_channel))
        .nest("/api", server_api)
        .nest("/debug", debug_api)
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", OpenApiDoc::openapi()))
        .merge(upnp)
        .layer(CorsLayer::permissive())
        .layer(http_trace)
        .fallback_service(assets_service)
        .with_state(app_state);

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(e) => {
            tracing::error!("Failed to start server on port {}: {e}", port);
            return;
        }
    };
    tracing::info!("Starting server on port {}", port);

    {
        let cancellation_token = cancellation_token.clone();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(cancellation_token.cancelled_owned())
            .await
            .unwrap();
        });
    }

    tracing::debug!(took = ?server_start_time.elapsed(), "Server is ready");
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
