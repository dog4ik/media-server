use std::path::PathBuf;

use crate::config::APP_RESOURCES;

pub mod command;
pub mod file_watcher;
pub mod job;
pub mod keyframe;
pub mod manifest;

#[derive(Debug, Clone)]
pub struct HlsTempPath(PathBuf);

impl HlsTempPath {
    pub fn hls_temp_dir() -> PathBuf {
        APP_RESOURCES.temp_path.join("hls")
    }

    pub fn new(task_id: uuid::Uuid) -> Self {
        Self(Self::hls_temp_dir().join(task_id.to_string()))
    }

    pub fn segment_path(&self, idx: usize) -> PathBuf {
        self.0.join(format!("{idx}.mp4"))
    }

    pub fn init_path(&self) -> PathBuf {
        self.0.join("init.mp4")
    }
}
