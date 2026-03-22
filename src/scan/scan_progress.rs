use std::path::PathBuf;

use serde::Serialize;
use tokio::sync::broadcast;

use crate::library::assets::FileAsset;

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProgressType {
    Asset {
        #[schema(value_type = String)]
        path: PathBuf,
    },
}

impl ProgressType {
    fn asset(asset: impl FileAsset) -> Self {
        Self::Asset { path: asset.path() }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Event {
    Start,
    Finish,
    Error,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct ProgressChunk {
    pub event: Event,
    pub progress_type: ProgressType,
}

pub trait ScanProgressConsumer {
    fn on_event(&self, event: ProgressChunk);
}

impl ScanProgressConsumer for broadcast::Sender<ProgressChunk> {
    fn on_event(&self, event: ProgressChunk) {
        let _ = self.send(event);
    }
}

impl<F> ScanProgressConsumer for F
where
    F: Fn(ProgressChunk) + Send + 'static,
{
    fn on_event(&self, event: ProgressChunk) {
        self(event);
    }
}
