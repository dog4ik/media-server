use std::{fmt::Display, sync::Mutex};

use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::error;
use uuid::Uuid;

use crate::{
    app_state::AppError, stream::transcode_stream::TranscodeStream, torrent::TorrentContent,
};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum VideoTaskKind {
    Transcode,
    LiveTranscode,
    Previews,
    Subtitles,
}

impl Display for VideoTaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            VideoTaskKind::Transcode => "Transcoding",
            VideoTaskKind::LiveTranscode => "Live transcoding",
            VideoTaskKind::Previews => "Previews generation",
            VideoTaskKind::Subtitles => "Subtitles extraction",
        };
        write!(f, "{msg}")
    }
}
#[derive(Debug, Clone, Serialize, Eq, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct VideoTask {
    pub video_id: i64,
    pub kind: VideoTaskKind,
}

impl Display for VideoTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} for video: {}", self.kind, self.video_id)
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct TorrentTask {
    pub info_hash: [u8; 20],
    pub content: Option<TorrentContent>,
}

impl From<TorrentTask> for TaskKind {
    fn from(value: TorrentTask) -> Self {
        Self::Torrent(value)
    }
}

impl Display for TorrentTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fn display_info_hash(hash: &[u8; 20]) -> String {
            hash.iter().fold(String::with_capacity(40), |mut acc, x| {
                let hex = format!("{:x}", x);
                acc.push_str(&hex);
                acc
            })
        }
        write!(
            f,
            "Torrent with info_hash: {}",
            display_info_hash(&self.info_hash)
        )
    }
}

impl Eq for TorrentTask {}
impl PartialEq for TorrentTask {
    fn eq(&self, other: &Self) -> bool {
        self.info_hash == other.info_hash
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", tag = "task_kind")]
pub enum TaskKind {
    Video(VideoTask),
    Torrent(TorrentTask),
    Scan,
}

impl From<VideoTask> for TaskKind {
    fn from(value: VideoTask) -> Self {
        Self::Video(value)
    }
}

impl Display for TaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskKind::Scan => write!(f, "Library scan"),
            TaskKind::Video(video_task) => video_task.fmt(f),
            TaskKind::Torrent(torrent_task) => torrent_task.fmt(f),
        }
    }
}

fn ser_bool_option<S>(option: &Option<CancellationToken>, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    ser.serialize_bool(option.is_some())
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct Task {
    pub id: Uuid,
    pub kind: TaskKind,
    pub created: OffsetDateTime,
    #[serde(serialize_with = "ser_bool_option", rename = "cancelable")]
    #[schema(value_type = bool)]
    pub cancel: Option<CancellationToken>,
}

impl Task {
    pub fn new(kind: TaskKind, cancel_token: Option<CancellationToken>) -> Self {
        let id = Uuid::new_v4();
        let now = time::OffsetDateTime::now_utc();
        Self {
            created: now,
            id,
            kind,
            cancel: cancel_token,
        }
    }

    pub fn is_cancelable(&self) -> bool {
        self.cancel.is_some()
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", tag = "progress_type")]
pub enum ProgressStatus {
    Start,
    Finish,
    Pending {
        speed: Option<ProgressSpeed>,
        percent: Option<f32>,
    },
    Cancel,
    Error,
    Pause,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ProgressChunk {
    pub task_id: Uuid,
    pub status: ProgressStatus,
}

impl ProgressChunk {
    pub fn start(task_id: Uuid) -> Self {
        Self {
            task_id,
            status: ProgressStatus::Start,
        }
    }

    pub fn pending(task_id: Uuid, percent: Option<f32>, speed: Option<ProgressSpeed>) -> Self {
        Self {
            task_id,
            status: ProgressStatus::Pending { speed, percent },
        }
    }

    pub fn finish(task_id: Uuid) -> Self {
        Self {
            task_id,
            status: ProgressStatus::Finish,
        }
    }

    pub fn cancel(task_id: Uuid) -> Self {
        Self {
            task_id,
            status: ProgressStatus::Cancel,
        }
    }

    pub fn error(task_id: Uuid) -> Self {
        Self {
            task_id,
            status: ProgressStatus::Error,
        }
    }
}

#[derive(Debug)]
pub struct TaskResource {
    pub progress_channel: ProgressChannel,
    pub parent_cancellation_token: CancellationToken,
    pub tracker: TaskTracker,
    pub tasks: Mutex<Vec<Task>>,
    pub active_streams: Mutex<Vec<TranscodeStream>>,
}

#[derive(Debug, Clone)]
pub enum TaskError {
    Failure,
    Duplicate,
    NotCancelable,
    Canceled,
    NotFound,
}

impl std::error::Error for TaskError {}

impl From<TaskError> for AppError {
    fn from(value: TaskError) -> Self {
        match value {
            TaskError::Duplicate => Self::bad_request("Duplicate task encountered"),
            TaskError::NotCancelable => Self::bad_request("Task can not be canceled"),
            TaskError::Canceled => Self::bad_request("Task was canceled"),
            TaskError::NotFound => Self::not_found("Task was not found"),
            TaskError::Failure => Self::internal_error("Task failed"),
        }
    }
}

impl Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            TaskError::Failure => "Failure",
            TaskError::Duplicate => "Duplicate",
            TaskError::NotCancelable => "Not cancelable",
            TaskError::Canceled => "Canceled",
            TaskError::NotFound => "Not found",
        };
        write!(f, "{msg}")
    }
}

#[derive(Debug, Clone, Serialize, Copy, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", tag = "speed_type")]
pub enum ProgressSpeed {
    BytesPerSec { bytes: usize },
    RelativeSpeed { speed: f32 },
}

impl From<usize> for ProgressSpeed {
    fn from(value: usize) -> Self {
        Self::BytesPerSec { bytes: value }
    }
}

impl From<f32> for ProgressSpeed {
    fn from(value: f32) -> Self {
        Self::RelativeSpeed { speed: value }
    }
}

#[derive(Debug)]
pub struct Progress {
    pub is_finished: bool,
    pub percent: Option<f32>,
    pub speed: Option<ProgressSpeed>,
}

impl Progress {
    pub fn finished() -> Self {
        Self {
            is_finished: true,
            percent: None,
            speed: None,
        }
    }
}

pub trait ResourceTask {
    /// Required method. Must be cancellation safe
    fn progress(&mut self)
        -> impl std::future::Future<Output = Result<Progress, TaskError>> + Send;
    fn on_cancel(&mut self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        std::future::ready(Ok(()))
    }
}

impl TaskResource {
    pub fn new(cancellation_token: CancellationToken) -> Self {
        TaskResource {
            progress_channel: ProgressChannel::new(),
            parent_cancellation_token: cancellation_token,
            tasks: Mutex::new(Vec::new()),
            active_streams: Mutex::new(Vec::new()),
            tracker: TaskTracker::new(),
        }
    }

    pub async fn observe_task<T: ResourceTask>(
        &self,
        mut task: T,
        kind: impl Into<TaskKind>,
    ) -> Result<(), TaskError> {
        let ProgressChannel(channel) = self.progress_channel.clone();
        let child_token = self.parent_cancellation_token.child_token();
        let id = self.start_task(kind, Some(child_token.clone()))?;

        loop {
            tokio::select! {
                progress = task.progress() => {
                    match progress {
                        Ok(progress) => {
                            if progress.is_finished {
                                let _ = self.finish_task(id);
                                return Ok(());
                            }
                            let _ = channel.send(ProgressChunk::pending(
                                id,
                                progress.percent,
                                progress.speed,
                            ));
                        },
                        Err(e) => {
                            let _ = self.error_task(id);
                            return Err(e);
                        },
                    }
                }
                _ = child_token.cancelled() => {
                    if let Err(err) = task.on_cancel().await {
                        tracing::error!("Task cleanup failed: {err}")
                    };
                    let _ = self.cancel_task(id);
                    return Ok(())
                }
            }
        }
    }

    pub async fn run_future<F: std::future::Future>(
        &self,
        fut: F,
        kind: TaskKind,
    ) -> Result<F::Output, TaskError> {
        let child_token = self.parent_cancellation_token.child_token();
        let id = self.start_task(kind, Some(child_token.clone()))?;
        tokio::select! {
            result = self.tracker.track_future(fut) => {
                let _ = self.finish_task(id);
                Ok(result)
            },
            _ = child_token.cancelled() => {
                let _ = self.cancel_task(id);
                Err(TaskError::Canceled)
            },
        }
    }

    pub async fn run_result_future<R, E, F: std::future::Future<Output = Result<R, E>>>(
        &self,
        fut: F,
        kind: TaskKind,
    ) -> Result<R, TaskError> {
        let child_token = self.parent_cancellation_token.child_token();
        let id = self.start_task(kind, Some(child_token.clone()))?;
        tokio::select! {
            result = self.tracker.track_future(fut) => {
                self.finish_task(id);
                match result {
                    Ok(r) => {
                        self.finish_task(id);
                        Ok(r)
                    },
                    Err(_) => {
                        self.error_task(id);
                        Err(TaskError::Failure)
                    },
                }
            },
            _ = child_token.cancelled() => {
                let _ = self.cancel_task(id);
                Err(TaskError::Canceled)
            },
        }
    }

    fn add_task(&self, task: Task) -> Result<Uuid, TaskError> {
        let mut tasks = self.tasks.lock().unwrap();
        let duplicate = tasks.iter().find(|t| t.kind == task.kind);
        if let Some(duplicate) = duplicate {
            error!(
                "Failed to create task(): duplicate {} ({})",
                task.kind, duplicate.id
            );
            return Err(TaskError::Duplicate);
        }
        let id = task.id;
        tasks.push(task);
        Ok(id)
    }

    pub fn start_task(
        &self,
        kind: impl Into<TaskKind>,
        cancellation_token: Option<CancellationToken>,
    ) -> Result<Uuid, TaskError> {
        let task = Task::new(kind.into(), cancellation_token);
        let id = self.add_task(task)?;
        let _ = self.progress_channel.0.send(ProgressChunk::start(id));
        Ok(id)
    }

    fn remove_task(&self, id: Uuid) -> Option<Task> {
        let mut tasks = self.tasks.lock().unwrap();
        let idx = tasks.iter().position(|t| t.id == id)?;
        Some(tasks.remove(idx))
    }

    pub fn finish_task(&self, id: Uuid) -> Option<Task> {
        let task = self.remove_task(id)?;
        let _ = self.progress_channel.0.send(ProgressChunk::finish(id));
        Some(task)
    }

    pub fn error_task(&self, id: Uuid) -> Option<Task> {
        let task = self.remove_task(id)?;
        let _ = self.progress_channel.0.send(ProgressChunk::error(id));
        Some(task)
    }

    pub fn cancel_task(&self, id: Uuid) -> Result<(), TaskError> {
        let mut task = self.remove_task(id).ok_or(TaskError::NotFound)?;
        let cancel = task.cancel.take().ok_or(TaskError::NotCancelable)?;
        cancel.cancel();
        let _ = self.progress_channel.0.send(ProgressChunk::cancel(id));
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ProgressChannel(pub broadcast::Sender<ProgressChunk>);

impl Default for ProgressChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl ProgressChannel {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(10);

        Self(tx)
    }
}
