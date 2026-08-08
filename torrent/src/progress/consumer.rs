use std::time::Duration;

use crate::progress::Progress;

#[derive(Debug, serde::Serialize, Default)]
pub struct TrackerStats {
    pub url: String,
    pub announce_interval: Duration,
    pub peers: Option<usize>,
    pub leechers: Option<usize>,
}

pub trait ProgressConsumer: Send + 'static {
    fn consume_progress(
        &mut self,
        progress: Progress,
    ) -> impl std::future::Future<Output = ()> + Send;
}

impl<T, F> ProgressConsumer for T
where
    F: std::future::Future + Send + 'static,
    T: Fn(Progress) -> F + Send + 'static,
{
    async fn consume_progress(&mut self, progress: Progress) {
        self(progress).await;
    }
}

impl ProgressConsumer for std::sync::mpsc::Sender<Progress> {
    async fn consume_progress(&mut self, progress: Progress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for tokio::sync::mpsc::Sender<Progress> {
    async fn consume_progress(&mut self, progress: Progress) {
        let _ = self.try_send(progress);
    }
}

impl ProgressConsumer for tokio::sync::broadcast::Sender<Progress> {
    async fn consume_progress(&mut self, progress: Progress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for tokio::sync::watch::Sender<Progress> {
    async fn consume_progress(&mut self, progress: Progress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for flume::Sender<Progress> {
    async fn consume_progress(&mut self, progress: Progress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for () {
    async fn consume_progress(&mut self, _progress: Progress) {}
}
