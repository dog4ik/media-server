use std::{
    fmt::Display,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde::Serialize;
use time::OffsetDateTime;
use tokio::sync::{broadcast, mpsc, oneshot};
use torrent::Torrent;
use tracing::error;
use uuid::Uuid;

use crate::{
    app_state::AppError,
    ffmpeg::{FFmpegRunningJob, FFmpegTask},
};

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TaskKind {
    Transcode { target: PathBuf },
    Scan { target: PathBuf },
    FullScan,
    Previews { target: PathBuf },
    Subtitles { target: PathBuf },
    Torrent { info_hash: [u8; 20] },
}

fn display_info_hash(hash: &[u8; 20]) -> String {
    hash.into_iter().map(|x| format!("{:x}", x)).collect()
}

impl Display for TaskKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TaskKind::Transcode { target } => write!(f, "transcode: {}", target.display()),
            TaskKind::Scan { target } => write!(f, "file scan: {}", target.display()),
            TaskKind::FullScan => write!(f, "library scan"),
            TaskKind::Previews { target } => write!(f, "previews: {}", target.display()),
            TaskKind::Subtitles { target } => write!(f, "subtitles: {}", target.display()),
            TaskKind::Torrent { info_hash } => write!(f, "{}", display_info_hash(info_hash)),
        }
    }
}

#[derive(Debug)]
pub struct Task {
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

        let mut task = serializer.serialize_struct("task", 3)?;
        task.serialize_field("id", &self.id)?;
        task.serialize_field("kind", &self.kind)?;
        task.serialize_field("cancelable", &self.cancel.is_some())?;
        task.end()
    }
}

impl Task {
    pub fn new(kind: TaskKind, cancel_channel: Option<oneshot::Sender<()>>) -> Self {
        let id = Uuid::new_v4();
        let now = time::OffsetDateTime::now_utc();
        Self {
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
        let id = self.start_task(kind, Some(tx))?;
        loop {
            tokio::select! {
                Some(percent) = progress.recv() => {
                    let _ = channel.send(ProgressChunk::pending(id,percent));
                },
                res = job.wait() => {
                    match res {
                        Err(_) => {
                            let _ = self.error_task(id);
                            return Err(TaskError::Failure);
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

    /// Wait until task is over while dispatching progress
    pub async fn observe_torrent_download(
        &self,
        client: &'static torrent::Client,
        torrent: Torrent,
        output_path: PathBuf,
    ) -> Result<(), TaskError> {
        let info_hash = torrent.info_hash();
        let ProgressChannel(channel) = self.progress_channel.clone();
        let (tx, mut rx) = oneshot::channel();
        let (progress_tx, mut progress_rx) = mpsc::channel(100);
        let mut job = client
            .download(output_path, torrent, progress_tx)
            .await
            .map_err(|_| TaskError::Failure)?;
        let kind = TaskKind::Torrent { info_hash };
        let id = self.start_task(kind, Some(tx))?;
        loop {
            tokio::select! {
                Some(progress) = progress_rx.recv() => {
                    let _ = channel.send(ProgressChunk::pending(id, progress.percent as usize));
                },
                res = &mut job.handle => {
                    match res {
                        Ok(Ok(_)) => {
                            let _ = self.finish_task(id);
                            return Ok(());
                        }
                        _ => {
                            let _ = self.error_task(id);
                            return Err(TaskError::Failure);
                        }
                    }
                }
                _ = &mut rx => {
                    let _ = job.abort();
                    return Err(TaskError::Canceled)
                }
            };
        }
    }

    fn add_task(&self, task: Task) -> Result<Uuid, TaskError> {
        let mut tasks = self.tasks.lock().unwrap();
        let duplicate = tasks.iter().find(|t| t.kind == task.kind);
        if let Some(duplicate) = duplicate {
            error!(
                "Failed to create task(): dublicate {} ({})",
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
        kind: TaskKind,
        cancel_channel: Option<oneshot::Sender<()>>,
    ) -> Result<Uuid, TaskError> {
        let task = Task::new(kind, cancel_channel);
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
