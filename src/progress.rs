use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::{broadcast, mpsc, oneshot};
use tracing::error;
use uuid::Uuid;

use crate::{
    app_state::AppError,
    ffmpeg::{FFmpegRunningJob, FFmpegTask},
};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Transcode,
    Scan,
    FullScan,
    Previews,
    Subtitles,
}

#[derive(Debug)]
pub struct Task {
    pub target: PathBuf,
    pub id: Uuid,
    pub kind: TaskKind,
    pub created: OffsetDateTime,
    pub cancel: Option<oneshot::Sender<()>>,
}

impl Serialize for Task {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut task = serializer.serialize_struct("task", 4)?;
        task.serialize_field("target", &self.target)?;
        task.serialize_field("id", &self.id)?;
        task.serialize_field("kind", &self.kind)?;
        task.serialize_field("cancelable", &self.cancel.is_some())?;
        task.end()
    }
}

impl Task {
    pub fn new(
        target: PathBuf,
        kind: TaskKind,
        cancel_channel: Option<oneshot::Sender<()>>,
    ) -> Self {
        let id = Uuid::new_v4();
        let now = time::OffsetDateTime::now_utc();
        Self {
            target,
            created: now,
            id,
            kind,
            cancel: cancel_channel,
        }
    }

    pub fn is_cancelable(&self) -> bool {
        self.cancel.is_some()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProgressStatus {
    Start,
    Finish,
    Pending,
    Cancel,
    Error,
    Pause,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProgressChunk {
    pub task_id: Uuid,
    pub progress: usize,
    pub status: ProgressStatus,
}

impl ProgressChunk {
    pub fn start(task_id: Uuid) -> Self {
        Self {
            task_id,
            progress: 0,
            status: ProgressStatus::Start,
        }
    }

    pub fn pending(task_id: Uuid, progress: usize) -> Self {
        Self {
            task_id,
            progress,
            status: ProgressStatus::Pending,
        }
    }

    pub fn finish(task_id: Uuid) -> Self {
        Self {
            task_id,
            progress: 100,
            status: ProgressStatus::Finish,
        }
    }

    pub fn cancel(task_id: Uuid) -> Self {
        Self {
            task_id,
            progress: 0,
            status: ProgressStatus::Cancel,
        }
    }

    pub fn error(task_id: Uuid) -> Self {
        Self {
            task_id,
            progress: 0,
            status: ProgressStatus::Error,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskResource {
    pub progress_channel: ProgressChannel,
    pub tasks: Arc<Mutex<Vec<Task>>>,
}

#[derive(Debug, Clone)]
pub enum TaskError {
    Failure,
    Duplicate,
    NotCancelable,
    Canceled,
    NotFound,
}

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

impl TaskResource {
    pub fn new() -> Self {
        TaskResource {
            progress_channel: ProgressChannel::new(),
            tasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Wait until task is over while dispatching progress
    pub async fn observe_ffmpeg_task(
        &self,
        mut job: FFmpegRunningJob<impl FFmpegTask>,
        kind: TaskKind,
    ) -> Result<(), TaskError> {
        let ProgressChannel(channel) = self.progress_channel.clone();
        let (tx, mut rx) = oneshot::channel();
        let mut progress = job.progress();
        let id = self.start_task(job.target.clone(), kind, Some(tx))?;
        loop {
            tokio::select! {
                Some(percent) = progress.recv() => {
                    let _ = channel.send(ProgressChunk::pending(id,percent));
                },
                res = job.wait() => {
                    match res {
                        Err(_) => {
                            let _ = self.error_task(id);
                            return Err(TaskError::Failure)
                        }
                        Ok(status) => {
                            if status.success() {
                                let _ = self.finish_task(id);
                                return Ok(());
                            } else {
                                let _ = self.error_task(id);
                                return Err(TaskError::Failure);
                            }
                        }
                    }
                }
                _ = &mut rx => {
                    let _ = job.cancel().await;
                    return Err(TaskError::Canceled)
                }
            };
        }
    }

    fn add_task(&self, task: Task) -> Result<Uuid, TaskError> {
        let mut tasks = self.tasks.lock().unwrap();
        let duplicate = tasks
            .iter()
            .find(|t| t.target == task.target && t.kind == task.kind);
        if let Some(duplicate) = duplicate {
            error!(
                "Failed to create task: dublicate {} ({})",
                task.target.display(),
                duplicate.id
            );
            return Err(TaskError::Duplicate);
        }
        let id = task.id;
        tasks.push(task);
        Ok(id)
    }

    pub fn start_task(
        &self,
        target: PathBuf,
        kind: TaskKind,
        cancel_channel: Option<oneshot::Sender<()>>,
    ) -> Result<Uuid, TaskError> {
        let task = Task::new(target, kind, cancel_channel);
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
        cancel.send(()).unwrap();
        let _ = self.progress_channel.0.send(ProgressChunk::cancel(id));
        Ok(())
    }
}

pub trait ProgressJob {
    fn progress(&mut self) -> Result<mpsc::Sender<usize>, anyhow::Error>;
}

pub trait CancelJob {
    fn cancel(self) -> Result<(), anyhow::Error>;
}

impl ProgressChunk {
    pub fn is_done(&self) -> bool {
        self.progress == 100
    }
}

#[derive(Debug, Clone)]
pub struct ProgressChannel(pub broadcast::Sender<ProgressChunk>);

impl ProgressChannel {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(10);

        Self(tx)
    }
}
