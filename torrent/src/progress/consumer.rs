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
    fn consume_progress(&mut self, progress: Progress);
}

impl<F> ProgressConsumer for F
where
    F: FnMut(Progress) + Send + 'static,
{
    fn consume_progress(&mut self, progress: Progress) {
        self(progress);
    }
}

impl ProgressConsumer for std::sync::mpsc::Sender<Progress> {
    fn consume_progress(&mut self, progress: Progress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for tokio::sync::mpsc::Sender<Progress> {
    fn consume_progress(&mut self, progress: Progress) {
        let _ = self.try_send(progress);
    }
}

impl ProgressConsumer for tokio::sync::broadcast::Sender<Progress> {
    fn consume_progress(&mut self, progress: Progress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for tokio::sync::watch::Sender<Progress> {
    fn consume_progress(&mut self, progress: Progress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for flume::Sender<Progress> {
    fn consume_progress(&mut self, progress: Progress) {
        let _ = self.send(progress);
    }
}

impl ProgressConsumer for () {
    fn consume_progress(&mut self, _progress: Progress) {}
}
