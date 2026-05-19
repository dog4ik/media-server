use hls_stream::{HlsStreamConfiguration, HlsTempPath, job::HlsJobHandle};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    library::media::Video,
    progress::{ProgressDispatcher, TaskTrait},
};

pub mod direct_play;
pub mod hls_stream;
pub mod torrent_stream;

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, utoipa::ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StreamMethod {
    DirectPlay,
    Hls,
}

#[derive(Debug, Clone, Copy, serde::Serialize, utoipa::ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ClientType {
    WebClient,
    Upnp,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "stream_type")]
pub enum Stream {
    DirectPlay,
    Hls {
        #[serde(skip)]
        handle: HlsJobHandle,
        configuration: HlsStreamConfiguration,
    },
}

#[derive(Debug, Clone, utoipa::ToSchema, serde::Serialize, PartialEq)]
pub struct WatchProgress {
    pub current_time: crate::MediaDuration,
}

/// Task for watch tracking.
///
/// Be aware that currently watch tracking can be bypassed.
/// Therefore, these tasks should not be considered fully reliable.
#[derive(Debug, Clone, utoipa::ToSchema, serde::Serialize)]
pub struct WatchTask {
    pub video_id: i64,
    pub total_duration: crate::MediaDuration,
    pub variant_id: Option<uuid::Uuid>,
    pub method: StreamMethod,
    pub client_agent: String,
    pub client_type: ClientType,
    #[serde(skip)]
    pub exit_token: CancellationToken,
    pub stream: crate::watch::Stream,
}

impl PartialEq for WatchTask {
    fn eq(&self, _other: &Self) -> bool {
        // watch tasks are can't be duplicates
        false
    }
}

impl TaskTrait for WatchTask {
    type Progress = WatchProgress;

    fn into_progress(
        status: crate::progress::ProgressStatus<Self>,
    ) -> crate::progress::TaskProgress {
        crate::progress::TaskProgress::WatchSession(status)
    }
}

impl WatchTask {
    pub async fn spawn_hls(
        video: &Video,
        configuration: HlsStreamConfiguration,
        progress_dispatcher: ProgressDispatcher<WatchTask>,
        exit_token: CancellationToken,
        tracker: TaskTracker,
    ) -> HlsJobHandle {
        let task_id = progress_dispatcher.task_id();
        let hls_path = HlsTempPath::new(task_id);
        hls_stream::job::start(
            video,
            configuration,
            hls_path,
            task_id.to_string(),
            progress_dispatcher,
            exit_token,
            tracker,
        )
        .await
        .unwrap()
    }

    pub async fn spawn_direct_play(
        _progress_dispatcher: ProgressDispatcher<WatchTask>,
        _tracker: TaskTracker,
    ) {
        todo!();
    }
}
