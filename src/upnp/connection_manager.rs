use upnp::connection_manager::ConnectionManagerHandler;

#[derive(Debug, Clone)]
pub struct MediaServerConnectionManager;

impl ConnectionManagerHandler for MediaServerConnectionManager {
    async fn get_protocol_info(&self) -> Result<(String, String), upnp::action::ActionError> {
        todo!()
    }

    async fn get_current_connection_ids(&self) -> Result<String, upnp::action::ActionError> {
        todo!()
    }

    async fn get_current_connection_info(
        &self,
        _connection_id: String,
    ) -> Result<
        (
            String,
            String,
            String,
            String,
            upnp::connection_manager::ArgDirection,
            String,
        ),
        upnp::action::ActionError,
    > {
        todo!()
    }

    async fn get_feature_list(&self) -> Result<String, upnp::action::ActionError> {
        todo!()
    }
}
