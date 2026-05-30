use std::time::Duration;

use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub struct SessionConfiguration {
    pub tick_interval: Duration,
    pub cancellation_token: CancellationToken,
}

impl Default for SessionConfiguration {
    fn default() -> Self {
        Self {
            tick_interval: Duration::from_millis(500),
            cancellation_token: CancellationToken::new(),
        }
    }
}
