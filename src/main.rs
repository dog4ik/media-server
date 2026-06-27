#![windows_subsystem = "windows"]
use axum::Router;
use axum::routing::{any, delete, get, patch, post, put};
use clap::Parser;
use dotenvy::dotenv;
use media_server::api::{self, OpenApiDoc};
use media_server::app_state::AppState;
use media_server::config::{self, APP_RESOURCES, AppResources, Args, ConfigFile};
use media_server::db::Db;
use media_server::library::Library;
use media_server::metadata::metadata_stack::MetadataProvidersStack;
use media_server::progress::TaskResource;
use media_server::torrent::TorrentClient;
use media_server::tracing::init_tracer;
use media_server::upnp::Upnp;
use media_server::{ffmpeg_abi, ws};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing::{Instrument, info_span};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

// Route every allocation in the process through jemalloc, including ffmpegs C allocations.
// glibc kept the freed probing buffers resident after a library scan thus leaving gigabytes claimed but unused memory.
// jemalloc returns them to the OS on its decay schedule.
#[cfg(all(feature = "jemalloc", target_os = "linux", target_env = "gnu"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(all(feature = "jemalloc", target_os = "linux", target_env = "gnu"))]
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "malloc_conf")]
pub static MALLOC_CONF: &[u8] = b"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:0\0";

#[tokio::main]
async fn main() {
    ffmpeg_next::init().expect("ffmpeg abi to initiate");
    ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Panic);
    Args::parse().apply_configuration();
    if let Err(err) = AppResources::initiate() {
        panic!("Could not initiate app resources: {err}");
    };
    // Load the dotfile and config file before initializing tracing: the otel
    // exporter is gated on the `otel_endpoint` config value, so the config must
    // be resolved first.
    let dotenv_path = dotenv().ok();
    let config_error = match ConfigFile::open_and_read().await {
        Ok(toml) => {
            config::CONFIG.apply_toml_settings(toml);
            None
        }
        Err(err) => Some(err),
    };

    let (
        config::OtelEndpoint(otel_endpoint),
        config::Port(port),
        config::ShowFolders(show_dirs),
        config::MovieFolders(movie_dirs),
        config::WebUiPath(web_ui_path),
    ) = config::CONFIG.get_values();
    let _guard = init_tracer(otel_endpoint.as_deref());

    match dotenv_path {
        Some(path) => tracing::info!("Loaded env variables from: {}", path.display()),
        None => tracing::warn!("Could not load env variables from dotfile"),
    }
    if let Some(err) = config_error {
        tracing::error!("Failed to read config file: {err}");
    }
    match &otel_endpoint {
        Some(endpoint) => tracing::info!("OpenTelemetry enabled, exporting to {endpoint}"),
        None => tracing::info!("OpenTelemetry disabled (set `otel_endpoint` to enable)"),
    }
    tracing::info!("Using log file location: {}", AppResources::log().display());

    // The whole boot sequence runs inside a single `startup` span
    let (cancellation_token, tracker, torrent_client) = async move {
        tokio::spawn(ffmpeg_abi::get_or_init_gpu_accelated_apis());

        let cancellation_token = CancellationToken::new();

        let db = Db::connect(&APP_RESOURCES.database_path)
            .await
            .expect("database to be found");

        let db = Box::leak(Box::new(db));

        let library = Library::init_from_folders(show_dirs, movie_dirs, db).await;
        let library = Box::leak(Box::new(Mutex::new(library)));

        let mut providers_stack = MetadataProvidersStack::new();
        providers_stack.setup_providers();
        let providers_stack = Box::leak(Box::new(providers_stack));

        let tasks = TaskResource::new(cancellation_token.clone());
        let tasks = Box::leak(Box::new(tasks));
        let tracker = tasks.tracker.clone();

        let torrent_client = TorrentClient::new(tasks, db.clone()).await.unwrap();
        torrent_client.load_torrents().await.unwrap();

        let torrent_client: &'static TorrentClient = Box::leak(Box::new(torrent_client));

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
                "/local_episode/{episode_id}/watch",
                get(api::server::watch_episode),
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
                post(api::intros::detect_intros),
            )
            .route(
                "/season/{season_id}/poster",
                get(api::server::season_poster),
            )
            .route(
                "/season/{season_id}/intros",
                delete(api::intros::delete_season_intros),
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
                delete(api::intros::delete_episode_intros),
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
            .route("/video/{id}/intro", put(api::intros::update_video_intro))
            .route("/video/{id}/intro", get(api::intros::video_intro))
            .route("/video/{id}/intro", delete(api::intros::delete_video_intro))
            .route(
                "/video/{id}/metadata",
                get(api::server::video_content_metadata),
            )
            .route(
                "/video/{id}/pull_subtitle",
                get(api::subtitles::pull_video_subtitle),
            )
            .route("/video/{id}/previews/{number}", get(api::server::previews))
            .route("/video/{id}/previews", post(api::server::generate_previews))
            .route("/video/{id}/previews", delete(api::server::delete_previews))
            .route(
                "/metadata/{id}/history",
                delete(api::history::remove_metadata_history),
            )
            .route(
                "/metadata/{id}/history",
                put(api::history::update_metadata_history),
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
                post(api::subtitles::upload_subtitles),
            )
            .route(
                "/video/{id}/reference_subtitles",
                post(api::subtitles::reference_external_subtitles),
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
            .route("/subtitles/{id}", delete(api::subtitles::delete_subtitles))
            .route("/subtitles/{id}", get(api::subtitles::get_subtitles))
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
            .route(
                "/tasks/watch_session/{id}",
                delete(api::server::stop_watch_session),
            )
            .route("/tasks/progress", get(api::server::progress))
            .route("/ws", any(ws::ws))
            .route("/scan", post(api::server::reconciliate_lib))
            .route(
                "/fix_metadata/{metadata_id}",
                post(api::server::fix_metadata),
            )
            .route(
                "/reset_metadata/{metadata_id}",
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

        let assets_service =
            ServeDir::new(&web_ui_path).fallback(ServeFile::new(web_ui_path.join("index.html")));

        let upnp = Upnp::init(app_state.clone()).await;

        let http_trace = tower_http::trace::TraceLayer::new_for_http();
        let app = Router::new()
            .nest("/api", server_api)
            .nest("/debug", debug_api)
            .merge(
                SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", OpenApiDoc::openapi()),
            )
            .merge(upnp)
            .layer(CorsLayer::permissive())
            .layer(http_trace)
            .fallback_service(assets_service)
            .with_state(app_state);

        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port);
        let listener = match tokio::net::TcpListener::bind(addr)
            .instrument(info_span!("bind_listener", %addr))
            .await
        {
            Ok(listener) => listener,
            Err(e) => {
                tracing::error!("Failed to start server on port {}: {e}", port);
                std::process::exit(1);
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

        tracing::info!("Server is ready");
        (cancellation_token, tracker, torrent_client)
    }
    .instrument(info_span!("startup"))
    .await;

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
