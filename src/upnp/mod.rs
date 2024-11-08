use connection_manager::MediaServerConnectionManager;
use content_directory::MediaServerContentDirectory;
use upnp::{connection_manager::ConnectionManagerService, content_directory::ContentDirectoryService, router::UpnpRouter};

use crate::{app_state::AppState, config, utils};

pub mod content_directory;
pub mod connection_manager;

#[derive(Debug)]
pub struct Upnp {
    pub router: UpnpRouter<AppState>,
}

impl Upnp {
    pub async fn init(app_state: AppState) -> Self {
        let cancellation_token = app_state.cancelation_token.child_token();
        let tracker = app_state.tasks.tracker.clone();
        let port: config::Port = config::CONFIG.get_value();

        tracker.spawn(async move {
            let config = upnp::ssdp::SsdpListenerConfig {
                location_port: port.0,
                ttl: None,
            };
            let mut listener = match upnp::ssdp::SsdpListener::bind(config).await {
                Ok(listener) => listener,
                Err(err) => {
                    tracing::error!("Failed to create ssdp listener: {err}");
                    return;
                }
            };
            if let Err(err) = listener.listen(cancellation_token).await {
                tracing::error!("Ssdp listener failed: {err}");
            }
        });

        let mut router = upnp::router::UpnpRouter::new("/upnp");
        match utils::local_addr().await {
            Ok(local_addr) => {
                let server_location = format!("http://{}:{}", local_addr.ip(), port.0);
                let content_directory =
                    MediaServerContentDirectory::new(app_state.db.clone(), server_location);
                let content_directory = ContentDirectoryService::new(content_directory);
                let connection_manager = MediaServerConnectionManager;
                let connection_manager = ConnectionManagerService::new(connection_manager);
                router = router.register_service(content_directory);
                router = router.register_service(connection_manager);
            }
            Err(e) => {
                tracing::error!("Failed to resolve server local address: {e}");
                tracing::warn!("Skipping initiation of content directory service");
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
