use upnp::connection_manager::ConnectionManagerHandler;

#[derive(Debug, Clone)]
pub struct MediaServerConnectionManager;

impl ConnectionManagerHandler for MediaServerConnectionManager {
    fn get_protocol_info(
        &self,
    ) -> impl std::future::Future<
        Output = Result<(String, String), upnp::action::ActionError>,
    > + Send {
        async { todo!() }
    }

    fn get_current_connection_ids(
        &self,
    ) -> impl std::future::Future<Output = Result<String, upnp::action::ActionError>> + Send + Sync
    {
        async { todo!() }
    }

    fn get_current_connection_info(
        &self,
        _connection_id: String,
    ) -> impl std::future::Future<
        Output = Result<
            (
                String,
                String,
                String,
                String,
                upnp::connection_manager::Direction,
                String,
            ),
            upnp::action::ActionError,
        >,
    > + Send
           + Sync {
        async { todo!() }
    }

    fn get_feature_list(
        &self,
    ) -> impl std::future::Future<Output = Result<String, upnp::action::ActionError>> + Send + Sync
    {
        async { todo!() }
    }
}
