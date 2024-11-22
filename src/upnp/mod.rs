use std::time::Duration;

use connection_manager::MediaServerConnectionManager;
use content_directory::MediaServerContentDirectory;
use tokio_util::sync::CancellationToken;
use upnp::{
    connection_manager::ConnectionManagerService,
    content_directory::ContentDirectoryService,
    router::UpnpRouter,
    ssdp::SsdpListenerConfig,
    templates::{SpecVersion, UpnpAgent},
};

use crate::{app_state::AppState, config, utils};

pub mod connection_manager;
pub mod content_directory;

#[derive(Debug)]
pub struct Upnp {
    pub router: UpnpRouter<AppState>,
}

const RETRY_TIME: Duration = Duration::from_secs(5);

async fn sleep_with_cancel(sleep_duration: Duration, cancellation_token: &CancellationToken) {
    tokio::select! {
        _ = tokio::time::sleep(sleep_duration) => {}
        _ = cancellation_token.cancelled() => {}
    }
}

async fn run_retry_ssdp(
    mut ssdp_config: SsdpListenerConfig,
    cancellation_token: CancellationToken,
) {
    let mut is_enabled_watcher = config::CONFIG.watch_value::<config::UpnpEnabled>();
    let mut ttl_watcher = config::CONFIG.watch_value::<config::UpnpTtl>();
    loop {
        if is_enabled_watcher.current_value().0 {
            let mut listener = match upnp::ssdp::SsdpListener::bind(ssdp_config.clone()).await {
                Ok(listener) => listener,
                Err(err) => {
                    tracing::error!(
                        "Failed to create ssdp listener: {err}, retrying in {:?}",
                        RETRY_TIME
                    );
                    sleep_with_cancel(RETRY_TIME, &cancellation_token).await;
                    continue;
                }
            };
            tokio::select! {
                res = listener.listen(cancellation_token.clone()) => {
                    match res {
                        Ok(_) => {
                            return
                        },
                        Err(err) => {
                            tracing::warn!("Ssdp listener failed: {err}, retrying in {:?}", RETRY_TIME);
                            sleep_with_cancel(RETRY_TIME, &cancellation_token).await;
                        }
                    }
                },
                new_ttl = ttl_watcher.watch_change() => {
                    tracing::warn!("Detected config change, recreating listener");
                    ssdp_config.ttl = Some(new_ttl.0);
                },
                new_enabled = is_enabled_watcher.watch_change() => {
                    tracing::debug!("SSDP enabled changed to: {}", new_enabled.0);
                },
            };
        } else {
            tokio::select! {
                _ = is_enabled_watcher.watch_change() => {}
                _ = cancellation_token.cancelled() => {
                    return;
                }
            }
        }
    }
}

impl Upnp {
    pub async fn init(app_state: AppState) -> Self {
        let os = &config::APP_RESOURCES.os;
        let os_version = &config::APP_RESOURCES.os_version;
        let product_version = config::APP_RESOURCES.app_version;
        let cancellation_token = app_state.cancelation_token.child_token();
        let tracker = app_state.tasks.tracker.clone();
        let port: config::Port = config::CONFIG.get_value();
        let ttl: config::UpnpTtl = config::CONFIG.get_value();

        let config = upnp::ssdp::SsdpListenerConfig {
            location_port: port.0,
            ttl: Some(ttl.0),
            user_agent: UpnpAgent {
                os,
                os_version,
                upnp_version: SpecVersion::upnp_v2(),
                product: config::AppResources::APP_NAME,
                product_version,
            },
        };

        tracker.spawn(run_retry_ssdp(config, cancellation_token));

        let mut router = upnp::router::UpnpRouter::new("/upnp");
        match utils::local_addr().await {
            Ok(local_addr) => {
                let server_location = format!("http://{}:{}", local_addr.ip(), port.0);
                let content_directory =
                    MediaServerContentDirectory::new(app_state, server_location);
                let content_directory = ContentDirectoryService::new(content_directory);
                let connection_manager = MediaServerConnectionManager;
                let connection_manager = ConnectionManagerService::new(connection_manager);
                router = router.register_service(content_directory);
                router = router.register_service(connection_manager);
            }
            Err(e) => {
                tracing::error!("Failed to resolve server local address: {e}");
                tracing::warn!("Skipping initiation of upnp services");
            }
        }

        Self { router }
    }
}

impl From<Upnp> for axum::Router<AppState> {
    fn from(upnp: Upnp) -> Self {
        upnp.router.into()
    }
}
