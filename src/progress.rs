use std::{fmt::Display, sync::Mutex};

use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{
    app_state::AppError,
    ffmpeg::{PreviewsJob, TranscodeJob},
    intro_detection::IntroJob,
    stream::transcode_stream::TranscodeStream,
    torrent::PendingTorrent,
};

#[derive(Debug, Clone, Serialize, utoipa::ToSchema, PartialEq)]
#[serde(rename_all = "lowercase", tag = "task_type")]
pub enum TaskProgress {
    WatchSession(ProgressChunk<WatchTask>),
    Transcode(ProgressChunk<TranscodeJob>),
    Previews(ProgressChunk<PreviewsJob>),
    Torrent(ProgressChunk<PendingTorrent>),
    LibraryScan(ProgressChunk<LibraryScanTask>),
    IntroDetection(ProgressChunk<IntroJob>),
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Notification {
    #[serde(flatten)]
    task_progress: TaskProgress,
    /// This is used to cancel activity
    activity_id: Uuid,
}

impl Notification {
    pub fn new(id: Uuid, progress: impl Into<TaskProgress>) -> Self {
        Self {
            task_progress: progress.into(),
            activity_id: id,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, utoipa::ToSchema)]
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

#[derive(Debug, Clone, Serialize, Eq, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct LibraryScanTask;

impl TaskTrait for LibraryScanTask {
    type Identifier = ();

    type Progress = Vec<String>;

    fn identifier(&self) -> Self::Identifier {
        ()
    }

    fn into_progress(chunk: ProgressChunk<Self>) -> TaskProgress {
        TaskProgress::LibraryScan(chunk)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct WatchTask {}

impl TaskTrait for WatchTask {
    type Identifier = ();

    type Progress = ();

    fn identifier(&self) -> Self::Identifier {
        ()
    }

    fn into_progress(chunk: ProgressChunk<Self>) -> TaskProgress {
        TaskProgress::WatchSession(chunk)
    }
}

#[derive(Debug)]
pub struct TaskStorage<T: TaskTrait> {
    pub tasks: Mutex<Vec<Task<T>>>,
    progress_channel: ProgressChannel,
}

impl<T: TaskTrait> TaskStorage<T> {
    fn new(progress_channel: ProgressChannel) -> Self {
        Self {
            tasks: Default::default(),
            progress_channel,
        }
    }

    fn remove_task(&self, id: Uuid) -> Option<Task<T>> {
        let mut tasks = self.tasks.lock().unwrap();
        let idx = tasks.iter().position(|t| t.id == id)?;
        Some(tasks.remove(idx))
    }

    pub fn finish_task(&self, id: Uuid) -> Option<Task<T>> {
        let task = self.remove_task(id)?;
        let ident = task.kind.identifier();
        let chunk = ProgressChunk {
            identifier: ident,
            status: ProgressStatus::Finish,
        };
        self.send_progress(id, chunk);
        Some(task)
    }

    pub fn error_task(&self, id: Uuid, error: TaskError) -> Option<Task<T>> {
        let task = self.remove_task(id)?;
        let ident = task.kind.identifier();
        let chunk = ProgressChunk {
            identifier: ident,
            status: ProgressStatus::Error {
                message: Some(error.to_string()),
            },
        };
        self.send_progress(id, chunk);
        Some(task)
    }

    pub fn cancel_task(&self, id: Uuid) -> Result<(), TaskError> {
        let mut task = self.remove_task(id).ok_or(TaskError::NotFound)?;
        let cancel = task.cancel.take().ok_or(TaskError::NotCancelable)?;
        cancel.cancel();
        let chunk = ProgressChunk {
            identifier: task.kind.identifier(),
            status: ProgressStatus::Cancel,
        };
        self.send_progress(id, chunk);
        Ok(())
    }

    pub fn send_progress(&self, task_id: Uuid, chunk: ProgressChunk<T>) {
        if let Ok(mut tasks) = self.tasks.try_lock() {
            if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
                task.latest_progress = chunk.clone();
            };
        } else {
            tracing::warn!(%task_id, "Failed to lock task without blocking");
        }
        let task_progress = T::into_progress(chunk);
        let notification = Notification {
            task_progress,
            activity_id: task_id,
        };
        let _ = self.progress_channel.0.send(notification);
    }
}

impl<T: TaskTrait + PartialEq> TaskStorage<T> {
    fn add_task(&self, task: Task<T>) -> Result<Uuid, TaskError> {
        let mut tasks = self.tasks.lock().unwrap();
        if tasks.iter().find(|t| t.kind == task.kind).is_some() {
            return Err(TaskError::Duplicate);
        }
        let id = task.id;
        tasks.push(task);
        Ok(id)
    }
}

impl<T: TaskTrait<Progress: Clone, Identifier: Clone> + PartialEq> TaskStorage<T> {
    pub fn start_task(
        &self,
        kind: T,
        cancellation_token: Option<CancellationToken>,
    ) -> Result<Uuid, TaskError> {
        let task = Task::new(kind, cancellation_token);
        let latest_progress = task.latest_progress.clone();
        let id = self.add_task(task)?;
        let task_progress = T::into_progress(latest_progress);
        let notification = Notification {
            task_progress,
            activity_id: id,
        };
        let _ = self.progress_channel.0.send(notification);
        Ok(id)
    }
}

impl<T: TaskTrait + Serialize> TaskStorage<T> {
    pub fn tasks(&self) -> serde_json::Value {
        serde_json::to_value(&*self.tasks.lock().unwrap()).unwrap()
    }
}

impl<T> TaskStorage<T>
where
    T: TaskTrait + PartialEq,
{
    pub async fn observe_task<P: ProgressDispatch<T>>(
        &self,
        task: T,
        mut dispatch: P,
    ) -> Result<(), TaskError> {
        let identifier = task.identifier();
        let child_token = CancellationToken::new();
        let id = self.start_task(task, Some(child_token.clone()))?;

        loop {
            tokio::select! {
                progress = dispatch.progress() => {
                    match progress {
                        Ok(progress) => {
                            match progress {
                                ProgressStatus::Finish => {
                                    self.finish_task(id).unwrap();
                                    return Ok(());
                                }
                                ProgressStatus::Pending { .. } => {
                                    let task_progress = ProgressChunk {
                                        identifier: identifier.clone(),
                                        status: progress,
                                    };
                                    self.send_progress(id, task_progress);
                                }
                                ProgressStatus::Cancel => {
                                    let _ = dispatch.on_cancel().await;
                                }
                                _ => {}
                            }
                        }
                        Err(e) => {
                            self.error_task(id, e).unwrap();
                            return Err(e);
                        }
                    }
                }
                _ = child_token.cancelled() => {
                    if let Err(err) = dispatch.on_cancel().await {
                        tracing::error!("Task cleanup failed: {err}")
                    };
                    return Err(TaskError::Canceled)
                }
            }
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema, PartialEq)]
pub struct ProgressChunk<T: TaskTrait> {
    #[serde(flatten)]
    pub identifier: T::Identifier,
    pub status: ProgressStatus<T::Progress>,
}

impl<T: TaskTrait> Clone for ProgressChunk<T> {
    fn clone(&self) -> Self {
        Self {
            identifier: self.identifier.clone(),
            status: self.status.clone(),
        }
    }
}

pub trait TaskTrait {
    type Identifier: Serialize + Clone + utoipa::ToSchema + std::fmt::Debug;
    type Progress: Serialize + Clone + utoipa::ToSchema + std::fmt::Debug;

    fn identifier(&self) -> Self::Identifier;
    fn into_progress(chunk: ProgressChunk<Self>) -> TaskProgress
    where
        Self: Sized;
}

fn ser_bool_option<S>(option: &Option<CancellationToken>, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    ser.serialize_bool(option.is_some())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct Task<T: TaskTrait> {
    pub id: Uuid,
    pub kind: T,
    pub latest_progress: ProgressChunk<T>,
    pub created: OffsetDateTime,
    #[serde(serialize_with = "ser_bool_option", rename = "cancelable")]
    #[schema(value_type = bool)]
    pub cancel: Option<CancellationToken>,
}

impl<T: TaskTrait> Task<T> {
    pub fn new(kind: T, cancel_token: Option<CancellationToken>) -> Self {
        let id = Uuid::new_v4();
        let now = time::OffsetDateTime::now_utc();
        Self {
            created: now,
            id,
            latest_progress: ProgressChunk {
                identifier: kind.identifier(),
                status: ProgressStatus::Start,
            },
            kind,
            cancel: cancel_token,
        }
    }

    pub fn latest_progress(&self) -> ProgressChunk<T> {
        self.latest_progress.clone()
    }

    pub fn is_cancelable(&self) -> bool {
        self.cancel.is_some()
    }
}

#[derive(Debug, Clone, Default, Serialize, utoipa::ToSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase", tag = "progress_type")]
pub enum ProgressStatus<T> {
    #[default]
    Start,
    Finish,
    Pending {
        progress: T,
    },
    Cancel,
    Error {
        message: Option<String>,
    },
    Pause,
}

#[derive(Debug)]
pub struct TaskResource {
    pub progress_channel: ProgressChannel,
    pub parent_cancellation_token: CancellationToken,
    pub tracker: TaskTracker,
    pub transcode_tasks: TaskStorage<TranscodeJob>,
    pub previews_tasks: TaskStorage<PreviewsJob>,
    pub library_scan_tasks: TaskStorage<LibraryScanTask>,
    pub torrent_tasks: TaskStorage<PendingTorrent>,
    pub intro_detection_tasks: TaskStorage<IntroJob>,
    pub active_streams: Mutex<Vec<TranscodeStream>>,
}

#[derive(Debug, Clone, Copy)]
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

pub trait ProgressDispatch<T: TaskTrait> {
    /// Required method. Must be cancellation safe
    fn progress(
        &mut self,
    ) -> impl std::future::Future<Output = Result<ProgressStatus<T::Progress>, TaskError>> + Send;

    fn on_cancel(&mut self) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        std::future::ready(Ok(()))
    }
}

impl TaskResource {
    pub fn new(cancellation_token: CancellationToken) -> Self {
        let progress_channel = ProgressChannel::new();
        TaskResource {
            parent_cancellation_token: cancellation_token,
            transcode_tasks: TaskStorage::new(progress_channel.clone()),
            library_scan_tasks: TaskStorage::new(progress_channel.clone()),
            torrent_tasks: TaskStorage::new(progress_channel.clone()),
            previews_tasks: TaskStorage::new(progress_channel.clone()),
            intro_detection_tasks: TaskStorage::new(progress_channel.clone()),
            active_streams: Mutex::new(Vec::new()),
            tracker: TaskTracker::new(),
            progress_channel,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProgressChannel(pub broadcast::Sender<Notification>);

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
