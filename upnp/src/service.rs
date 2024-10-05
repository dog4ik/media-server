use super::{
    action::{Action, ActionPayload, IntoArgumentList},
    templates::service_description::ServiceDescription,
    urn::URN,
};

pub trait Service {
    const NAME: &str;
    const URN: URN;

    fn service_description() -> ServiceDescription;
    fn actions() -> Vec<Action>;
    fn control_handler(
        &mut self,
        action: ActionPayload,
    ) -> impl std::future::Future<Output = anyhow::Result<impl IntoArgumentList>> + Send + Sync;
}
