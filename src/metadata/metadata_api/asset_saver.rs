use std::sync::Arc;

use tokio::{sync::Semaphore, task::JoinSet};

use crate::scan::AssetSaveTask;

#[derive(Debug)]
pub struct AssetTasks {
    tasks: Vec<AssetSaveTask>,
    http_client: reqwest::Client,
}

pub trait AssetsProgressSink {
    fn dispatch_success(&self);
    fn dispatch_fail(&self);
}

impl AssetsProgressSink for () {
    fn dispatch_success(&self) {}

    fn dispatch_fail(&self) {}
}

impl AssetTasks {
    pub fn new(http_client: reqwest::Client) -> Self {
        Self {
            tasks: Vec::new(),
            http_client,
        }
    }

    pub fn push(&mut self, task: AssetSaveTask) {
        self.tasks.push(task);
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub async fn save<T>(self, max_concurrency: usize, progress_handler: T)
    where
        T: AssetsProgressSink,
    {
        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        let mut join_set = JoinSet::new();
        for task in self.tasks {
            let semaphore = semaphore.clone();
            let http_client = self.http_client.clone();
            join_set.spawn(async move {
                let _permit = semaphore.acquire_owned().await.unwrap();
                task.execute(&http_client).await
            });
        }
        while let Some(Ok(val)) = join_set.join_next().await {
            match val {
                Ok(_) => {
                    progress_handler.dispatch_success();
                }
                Err(e) => {
                    progress_handler.dispatch_fail();
                    tracing::warn!("Asset save task failed: {e}");
                }
            }
        }
    }
}
