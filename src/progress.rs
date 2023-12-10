use std::{path::PathBuf, sync::Arc};

use serde::Serialize;
use tokio::sync::{broadcast, oneshot, Mutex};
use tracing::error;
use uuid::Uuid;

use crate::ffmpeg::FFmpegJob;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Transcode,
    Scan,
    Previews,
    Subtitles,
}

#[derive(Debug)]
pub struct Task {
    pub target: PathBuf,
    pub id: Uuid,
    pub kind: TaskKind,
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
        Self {
            target,
            id,
            kind,
            cancel: cancel_channel,
        }
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
    Duplicate,
    NotCancelable,
    Canceled,
    NotFound,
}

impl TaskResource {
    pub fn new() -> Self {
        TaskResource {
            progress_channel: ProgressChannel::new(),
            tasks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub async fn add_new_task(
        &self,
        target: PathBuf,
        kind: TaskKind,
        cancel_channel: Option<oneshot::Sender<()>>,
    ) -> Result<Uuid, TaskError> {
        let task = Task::new(target, kind, cancel_channel);
        self.add_task(task).await
    }

    pub async fn run_ffmpeg_task(
        &self,
        mut job: FFmpegJob,
        kind: TaskKind,
    ) -> Result<(), TaskError> {
        let ProgressChannel(channel) = self.progress_channel.clone();
        let (tx, rx) = oneshot::channel();
        let mut progress = job.progress();
        let id = self
            .add_new_task(job.target.clone(), kind, Some(tx))
            .await?;
        tokio::select! {
            _ = async {
                let _ = channel.send(ProgressChunk::start(id));
                while let Some(percent) = progress.recv().await {
                    let _ = channel.send(ProgressChunk::pending(id,percent));
                };
            } => {
                self.remove_task(id).await;
                if let Err(_) = job.wait().await{
                    let _ = channel.send(ProgressChunk::error(id));
                } else {
                    let _ = channel.send(ProgressChunk::finish(id));
                }
                Ok(())
            },
            _ = rx => {
                self.remove_task(id).await;
                job.kill().await;
                let _ = channel.send(ProgressChunk::cancel(id));
                Err(TaskError::Canceled)
            }
        }
    }

    async fn add_task(&self, task: Task) -> Result<Uuid, TaskError> {
        let mut tasks = self.tasks.lock().await;
        let duplicate = tasks
            .iter()
            .find(|t| t.target == task.target && t.kind == task.kind);
        if let Some(duplicate) = duplicate {
            error!(
                "Failed to create task: dublicate {:?} ({})",
                task.target, duplicate.id
            );
            return Err(TaskError::Duplicate);
        }
        let id = task.id;
        tasks.push(task);
        Ok(id)
    }

    pub async fn remove_task(&self, id: Uuid) -> Option<Task> {
        let mut tasks = self.tasks.lock().await;
        let idx = tasks.iter().position(|t| t.id == id)?;
        Some(tasks.remove(idx))
    }

    pub async fn cancel_task(&self, id: Uuid) -> Result<(), TaskError> {
        let mut tasks = self.tasks.lock().await;
        let task_to_cancel = tasks
            .iter_mut()
            .find(|t| t.id == id)
            .ok_or(TaskError::NotFound)?;
        let cancel = task_to_cancel
            .cancel
            .take()
            .ok_or(TaskError::NotCancelable)?;
        cancel.send(()).unwrap();
        drop(tasks);
        self.remove_task(id).await;
        Ok(())
    }
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
