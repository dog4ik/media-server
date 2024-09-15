use axum::{http::HeaderMap, routing::get, Router};
use axum_extra::headers::{self, HeaderMapExt};

pub struct UpnpRouter(pub Router<AppState>);

use crate::app_state::AppState;

use super::device_description;

async fn handle_description() -> (HeaderMap, String) {
    tracing::debug!("Serving device description");
    let desc = device_description::DeviceDescription::new("Media server".into());
    let mut headers = HeaderMap::new();
    headers.typed_insert(headers::ContentType::xml());
    (
        headers,
        quick_xml::se::to_string_with_root("root", &desc).unwrap(),
    )
}

pub const DESC_PATH: &str = "/devicedesc.xml";

impl UpnpRouter {
    pub fn new() -> Self {
        let router = Router::new().route(DESC_PATH, get(handle_description));

        Self(router)
    }
}
