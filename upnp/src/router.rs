use anyhow::Context;
use axum::{
    http::HeaderMap,
    routing::{get, post},
    Router,
};
use axum_extra::headers::{self, HeaderMapExt};

pub struct UpnpRouter(pub Router<AppState>);

use crate::{
    app_state::AppState,
    upnp::action::{ActionError, ActionPayload},
};

use super::{
    action::{ActionResponse, IntoArgumentList, SoapMessage},
    device_description,
    service::Service,
};

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

    pub fn register_service<S: Service + Send + Clone + 'static>(&mut self, mut service: S) {
        let base_path = format!("/{}", S::NAME);
        let control_path = format!("{base_path}/control.xml");
        let scpd_path = format!("{base_path}/scpd.xml");

        let action_handler = |headers: HeaderMap, body: String| async move {
            let mut header = headers
                .get("soapaction")
                .context("soap_action header")?
                .to_str()
                .context("convert header to string")?;
            if let Some(stripped) = header.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                header = stripped;
            }
            let (_urn, action_name) = header.split_once('#').context("split soapaction header")?;
            tracing::info!("Action {action_name} invoked");
            let action: SoapMessage<ActionPayload> = SoapMessage::from_xml(body.as_bytes())?;
            let action = action.into_inner();

            if action.name() != action_name {
                tracing::warn!(
                    "Inconsintence in soapaction header and action_payload: {} vs {}",
                    action_name,
                    action.name(),
                );
            }

            let out_arguments = service
                .control_handler(action)
                .await?
                .into_action_response();
            let action_response = ActionResponse {
                service_urn: S::URN,
                action_name: action_name.to_string(),
                args: out_arguments,
            };
            Ok::<_, ActionError>(action_response)
        };
        let scpd = S::service_description()
            .into_xml()
            .expect("services serialize without errors");
        let scpd_handler = || async move {
            let mut headers = HeaderMap::new();
            headers.typed_insert(headers::ContentType::xml());
            let response = String::from_utf8(scpd).unwrap();
            Ok::<_, ActionError>((headers, response))
        };
        self.0 = self.0.clone().route(&scpd_path, get(scpd_handler));
        self.0 = self.0.clone().route(&control_path, post(action_handler));
    }
}
