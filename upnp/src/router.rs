use anyhow::Context;
use axum::{
    http::HeaderMap,
    routing::{get, post},
    Router,
};
use axum_extra::headers::{self, HeaderMapExt};

#[derive(Debug)]
pub struct UpnpRouter<S> {
    path: String,
    router: Router<S>,
}

impl<S> From<UpnpRouter<S>> for Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn from(upnp_router: UpnpRouter<S>) -> Self {
        Router::new().nest(&upnp_router.path, upnp_router.router)
    }
}

use crate::{
    action::{ActionError, ActionPayload, IntoValueList},
    service::UpnpService,
};

use super::{
    action::{ActionResponse, SoapMessage},
    device_description,
    service::Service,
};

pub const DESC_PATH: &str = "/devicedesc.xml";

impl<T: Clone + Send + Sync + 'static> UpnpRouter<T> {
    pub fn new(path: &str, name: &'static str, uuid: uuid::Uuid) -> Self {
        let desc = device_description::DeviceDescription::new(name.to_owned(), uuid);
        let desc = std::sync::Arc::new(desc);
        let serve_description = move || async move {
            tracing::debug!("Serving device description");
            let mut headers = HeaderMap::new();
            headers.typed_insert(headers::ContentType::xml());
            (headers, desc.into_xml().unwrap())
        };
        let router = Router::new().route(DESC_PATH, get(serve_description));
        Self {
            path: path.to_string(),
            router,
        }
    }

    pub fn register_service<S: Service + Send + Sync + Clone + 'static>(
        mut self,
        service: S,
    ) -> Self {
        let base_path = format!("/{}", S::NAME);
        let control_path = format!("{base_path}/control.xml");
        let event_path = format!("{base_path}/event.xml");
        let scpd_path = format!("{base_path}/scpd.xml");
        let service = UpnpService::new(service);

        let action_handler = |headers: HeaderMap, body: String| async move {
            let mut header = headers
                .get("soapaction")
                .context("get soapaction header")?
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
            let expected_action = service.find_action(action_name)?;
            let scanner = expected_action.input_scanner(action.arguments);

            let out_arguments = service
                .s
                .control_handler(action_name, scanner)
                .await?
                .into_value_list();

            let args = expected_action.map_out_variables(out_arguments);

            let action_response = ActionResponse {
                service_urn: S::URN,
                action_name: action_name.to_string(),
                args,
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
        let event_handler = || async move {
            println!("Eventing is not yet implemented!");
            println!("Eventing is not yet implemented!");
            println!("Eventing is not yet implemented!");
            println!("Eventing is not yet implemented!");
            println!("Eventing is not yet implemented!");
            println!("Eventing is not yet implemented!");
            println!("Eventing is not yet implemented!");
            println!("Eventing is not yet implemented!");
        };
        self.router = self.router.route(&scpd_path, get(scpd_handler));
        self.router = self.router.route(&control_path, post(action_handler));
        self.router = self.router.route(&event_path, post(event_handler));
        self
    }
}
