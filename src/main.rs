#![windows_subsystem = "windows"]
use axum::Router;
use axum::routing::get;
use clap::Parser;
use dotenvy::dotenv;
use media_server::{
    APP_RESOURCES, AppResources, AppState, Args, CONFIG, ConfigFile, Db, Library,
    MetadataProvidersStack, MovieFolders, OtelEndpoint, Port, ShowFolders, TaskResource,
    TorrentClient, Upnp, WebUiPath, api_router, get_or_init_gpu_accelated_apis, init_tracer,
    library_state,
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tracing::{Instrument, info_span};
use utoipa_axum::router::OpenApiRouter;
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
            CONFIG.apply_toml_settings(toml);
            None
        }
        Err(err) => Some(err),
    };

    let (
        OtelEndpoint(otel_endpoint),
        Port(port),
        ShowFolders(show_dirs),
        MovieFolders(movie_dirs),
        WebUiPath(web_ui_path),
    ) = CONFIG.get_values();
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
        tokio::spawn(get_or_init_gpu_accelated_apis());

        let cancellation_token = CancellationToken::new();

        let http_client = reqwest::Client::new();

        let db = Db::connect(&APP_RESOURCES.database_path)
            .await
            .expect("database to be found");

        let db = Box::leak(Box::new(db));

        let library = Library::init_from_folders(show_dirs, movie_dirs, db).await;
        let library = Box::leak(Box::new(Mutex::new(library)));

        let mut providers_stack = MetadataProvidersStack::new();
        providers_stack.setup_providers(&http_client);
        let providers_stack = Box::leak(Box::new(providers_stack));

        let tasks = TaskResource::new(cancellation_token.clone());
        let tasks = Box::leak(Box::new(tasks));
        let tracker = tasks.tracker.clone();

        let torrent_client = TorrentClient::new(tasks, db.clone(), http_client.clone())
            .await
            .unwrap();
        torrent_client.load_torrents().await.unwrap();

        let torrent_client: &'static TorrentClient = Box::leak(Box::new(torrent_client));

        let app_state = AppState {
            library,
            db,
            tasks,
            providers_stack,
            torrent_client,
            http_client,
            cancelation_token: cancellation_token.clone(),
        };

        #[cfg(all(feature = "windows-tray", target_os = "windows"))]
        tokio::spawn(media_server::spawn_tray_icon(app_state.clone()));
        // tokio::spawn(watch::monitor_library(app_state.clone(), media_folders));
        // tokio::spawn(watch::monitor_config(app_state.configuration, config_path));

        let (server_api, openapi) = OpenApiRouter::new()
            .nest("/api", api_router())
            .split_for_parts();

        let debug_api = Router::new().route("/library", get(library_state));

        let assets_service =
            ServeDir::new(&web_ui_path).fallback(ServeFile::new(web_ui_path.join("index.html")));

        let upnp = Upnp::init(app_state.clone()).await;

        let http_trace = tower_http::trace::TraceLayer::new_for_http();
        let app = server_api
            .nest("/debug", debug_api)
            .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", openapi))
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
