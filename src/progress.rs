use std::{fmt::Display, sync::Mutex};

use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;

use crate::{
    app_state::AppError,
    ffmpeg::{PreviewsJob, TranscodeJob, VideoProgress},
    intro_detection::{IntroJob, IntroProgress},
    scan::scan_progress,
    torrent::{CompactTorrentProgress, PendingTorrent},
    watch::{WatchProgress, WatchTask},
};

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase", tag = "task_type")]
pub enum TaskProgress {
    WatchSession(ProgressStatus<WatchTask>),
    Transcode(ProgressStatus<TranscodeJob>),
    Previews(ProgressStatus<PreviewsJob>),
    Torrent(ProgressStatus<PendingTorrent>),
    LibraryScan(ProgressStatus<LibraryScanTask>),
    IntroDetection(ProgressStatus<IntroJob>),
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

#[derive(Debug, Clone, Serialize, Eq, PartialEq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct LibraryScanTask;

impl TaskTrait for LibraryScanTask {
    type Progress = scan_progress::ProgressChunk;

    fn into_progress(status: ProgressStatus<Self>) -> TaskProgress {
        TaskProgress::LibraryScan(status)
    }
}

/// Stores and manages task lifecycle
///
/// It ensures there are no duplicate tasks.
///
/// This is an abstraction to store and manage notifications about tasks automatically.
/// Any operation on task will be dispatched to the clients.
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
        self.send_progress(id, ProgressStatus::Finish);
        Some(task)
    }

    pub fn error_task(&self, id: Uuid, error: TaskError) -> Option<Task<T>> {
        let task = self.remove_task(id)?;
        self.send_progress(
            id,
            ProgressStatus::Error {
                message: Some(error.to_string()),
            },
        );
        Some(task)
    }

    pub fn cancel_task(&self, id: Uuid) -> Result<(), TaskError> {
        let mut task = self.remove_task(id).ok_or(TaskError::NotFound)?;
        let cancel = task.cancel.take().ok_or(TaskError::NotCancelable)?;
        cancel.cancel();
        self.send_progress(id, ProgressStatus::Cancel);
        Ok(())
    }

    pub fn send_progress(&self, task_id: Uuid, status: ProgressStatus<T>) {
        if let ProgressStatus::Pending { progress } = &status {
            if let Ok(mut tasks) = self.tasks.try_lock() {
                if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
                    task.latest_progress = Some(progress.clone());
                };
            } else {
                tracing::warn!(%task_id, "Failed to lock task without blocking");
            }
        }
        let task_progress = T::into_progress(status);
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
        if tasks.iter().any(|t| t.kind == task.kind) {
            return Err(TaskError::Duplicate);
        }
        let id = task.id;
        tasks.push(task);
        Ok(id)
    }
}

impl<T: TaskTrait<Progress: Clone> + PartialEq> TaskStorage<T> {
    pub fn start_task(
        &self,
        kind: T,
        cancellation_token: Option<CancellationToken>,
    ) -> Result<Uuid, TaskError> {
        let uuid = uuid::Uuid::new_v4();
        self.start_with_id(kind, uuid, cancellation_token)
    }

    pub fn start_with_id(
        &self,
        kind: T,
        uuid: uuid::Uuid,
        cancellation_token: Option<CancellationToken>,
    ) -> Result<Uuid, TaskError> {
        let task = Task::new(kind, uuid, cancellation_token);
        let json = serde_json::to_value(&task).unwrap();
        let id = self.add_task(task)?;
        let task_progress = T::into_progress(ProgressStatus::Start { task: json });
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

/// Dispatch progress to the task.
///
/// A dispatcher should end with a call to finish or error. If neither are called before the
/// dispatcher goes out-of-scope, error is called.
#[derive(Debug)]
pub struct ProgressDispatcher<T: TaskTrait + 'static> {
    task_id: uuid::Uuid,
    task_storage: &'static TaskStorage<T>,
    active: bool,
}

impl<T: TaskTrait> ProgressDispatcher<T> {
    pub fn new(task_storage: &'static TaskStorage<T>, task_id: uuid::Uuid) -> Self {
        Self {
            task_id,
            task_storage,
            active: true,
        }
    }

    pub fn progress(&self, progress: T::Progress) {
        self.task_storage
            .send_progress(self.task_id, ProgressStatus::Pending { progress });
    }

    pub fn error(mut self, err: TaskError) {
        self.active = false;
        self.task_storage.error_task(self.task_id, err);
    }

    /// Stop dispatcher from sending error on drop
    pub fn disarm(&mut self) {
        self.active = false;
    }

    pub fn finish(mut self) {
        self.active = false;
        self.task_storage.finish_task(self.task_id);
    }

    pub fn task_id(&self) -> uuid::Uuid {
        self.task_id
    }
}

impl<T: TaskTrait> Drop for ProgressDispatcher<T> {
    fn drop(&mut self) {
        if self.active {
            self.task_storage
                .error_task(self.task_id, TaskError::Failure);
        }
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
                                    self.send_progress(id, progress);
                                }
                                ProgressStatus::Cancel => {
                                    let _ = dispatch.on_cancel().await;
                                    self.cancel_task(id)?;
                                    return Err(TaskError::Canceled);
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

/// Trait implemented by all media server tasks.
///
/// The type this trait is implemented on will be sent when users fetch all tasks of a kind.
pub trait TaskTrait: Serialize + Clone + utoipa::ToSchema + std::fmt::Debug {
    /// This is the progress type of the task. Client will use it to show progress of the task.
    type Progress: Serialize + Clone + utoipa::ToSchema + std::fmt::Debug;

    fn into_progress(status: ProgressStatus<Self>) -> TaskProgress
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
    /// Its useful for client to know the current progress of the task without waiting for
    /// the next progress chunk
    pub latest_progress: Option<T::Progress>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(serialize_with = "ser_bool_option", rename = "cancelable")]
    #[schema(value_type = bool)]
    pub cancel: Option<CancellationToken>,
}

impl<T: TaskTrait> Task<T> {
    pub fn new(kind: T, id: uuid::Uuid, cancel_token: Option<CancellationToken>) -> Self {
        let now = time::OffsetDateTime::now_utc();
        Self {
            created: now,
            id,
            latest_progress: None,
            kind,
            cancel: cancel_token,
        }
    }

    pub fn is_cancelable(&self) -> bool {
        self.cancel.is_some()
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema, Eq, PartialEq)]
#[serde(rename_all = "lowercase", tag = "progress_type")]
pub enum ProgressStatus<T: TaskTrait> {
    Start {
        /// Use serde_json::Value since we just need the serialized payload
        #[schema(value_type = Task<T>)]
        task: serde_json::Value,
    },
    Finish,
    Pending {
        progress: T::Progress,
    },
    Cancel,
    Error {
        message: Option<String>,
    },
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
    pub watch_sessions: TaskStorage<WatchTask>,
}

/// State snapshot of all the running tasks
#[derive(Debug, Clone, utoipa::ToSchema, Serialize)]
pub struct TasksSnapshot {
    #[schema(value_type = Vec<Task<TranscodeJob>>)]
    pub transcode_tasks: serde_json::Value,
    #[schema(value_type = Vec<Task<PreviewsJob>>)]
    pub previews_tasks: serde_json::Value,
    #[schema(value_type = Vec<Task<LibraryScanTask>>)]
    pub library_scan_tasks: serde_json::Value,
    #[schema(value_type = Vec<Task<PendingTorrent>>)]
    pub torrent_tasks: serde_json::Value,
    #[schema(value_type = Vec<Task<IntroJob>>)]
    pub intro_detection_tasks: serde_json::Value,
    #[schema(value_type = Vec<Task<WatchTask>>)]
    pub watch_sessions: serde_json::Value,
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
    ) -> impl std::future::Future<Output = Result<ProgressStatus<T>, TaskError>> + Send;

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
            watch_sessions: TaskStorage::new(progress_channel.clone()),
            tracker: TaskTracker::new(),
            progress_channel,
        }
    }

    /// Get the copy of the current state of all tasks
    pub fn snapshot(&self) -> TasksSnapshot {
        TasksSnapshot {
            transcode_tasks: self.transcode_tasks.tasks(),
            previews_tasks: self.previews_tasks.tasks(),
            library_scan_tasks: self.library_scan_tasks.tasks(),
            torrent_tasks: self.torrent_tasks.tasks(),
            intro_detection_tasks: self.intro_detection_tasks.tasks(),
            watch_sessions: self.watch_sessions.tasks(),
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
        let (tx, _) = broadcast::channel(500);
        Self(tx)
    }
}
