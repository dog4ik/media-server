use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::ban_list::BanList;

#[derive(Debug)]
pub struct SessionConfiguration {
    pub tick_interval: Duration,
    pub ban_list: BanList,
    pub max_peer_connections: usize,
    pub cancellation_token: CancellationToken,
}

impl Default for SessionConfiguration {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_millis(500),
            max_peer_connections: 500,
            ban_list: BanList::default(),
            cancellation_token: CancellationToken::new(),
        }
    }
}
