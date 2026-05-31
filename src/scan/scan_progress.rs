use std::sync::{Arc, atomic};

use serde::Serialize;

use crate::progress::{LibraryScanTask, ProgressDispatcher};

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ProgressChunk {
    /// Files are being tokenized, grouped, metadata fetch happens.
    MetadataFetch {
        total_video_files: usize,
        success_count: usize,
        fail_count: usize,
    },
    /// At this stage fetched metadata is being saved to database
    MetadataSave,
    /// Assets are being saved to the disk.
    AssetsSave {
        total_assets_count: usize,
        success_count: usize,
        fail_count: usize,
    },
}

#[derive(Debug, Clone)]
pub struct ScanProgressEmitter {
    dispatch: Arc<ProgressDispatcher<LibraryScanTask>>,
}

impl ScanProgressEmitter {
    pub fn new(dispatcher: ProgressDispatcher<LibraryScanTask>) -> Self {
        Self {
            dispatch: Arc::new(dispatcher),
        }
    }

    fn emit(&self, progress: ProgressChunk) {
        self.dispatch.progress(progress);
    }

    pub fn finish_scan(self) {
        let dispatcher = Arc::try_unwrap(self.dispatch)
            .expect("when finish is called all other progress emitters are dropped");
        dispatcher.finish();
    }

    pub fn assets_progress_emitter(&self, asset_count: usize) -> AssetProgressEmitter {
        AssetProgressEmitter {
            total_count: asset_count,
            done_count: Default::default(),
            fail_count: Default::default(),
            emitter: self.clone(),
        }
    }

    pub fn metadata_progress_emitter(&self, file_count: usize) -> MetadataProgressEmitter {
        MetadataProgressEmitter {
            total_file_count: file_count,
            done_count: Default::default(),
            fail_count: Default::default(),
            emitter: self.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetadataProgressEmitter {
    pub total_file_count: usize,
    pub done_count: Arc<atomic::AtomicUsize>,
    pub fail_count: Arc<atomic::AtomicUsize>,
    pub emitter: ScanProgressEmitter,
}

impl MetadataProgressEmitter {
    pub fn dispatch_success(&self, count: usize) {
        self.emitter.emit(ProgressChunk::MetadataFetch {
            total_video_files: self.total_file_count,
            success_count: self.done_count.fetch_add(count, atomic::Ordering::Relaxed) + count,
            fail_count: self.fail_count.load(atomic::Ordering::Relaxed),
        });
    }

    pub fn dispatch_fail(&self, count: usize) {
        self.emitter.emit(ProgressChunk::MetadataFetch {
            total_video_files: self.total_file_count,
            success_count: self.done_count.load(atomic::Ordering::Relaxed),
            fail_count: self.fail_count.fetch_add(count, atomic::Ordering::Relaxed) + count,
        });
    }
}

#[derive(Debug, Clone)]
pub struct AssetProgressEmitter {
    pub total_count: usize,
    pub done_count: Arc<atomic::AtomicUsize>,
    pub fail_count: Arc<atomic::AtomicUsize>,
    pub emitter: ScanProgressEmitter,
}

impl AssetProgressEmitter {
    pub fn dispatch_success(&self) {
        self.emitter.emit(ProgressChunk::AssetsSave {
            total_assets_count: self.total_count,
            success_count: self.done_count.fetch_add(1, atomic::Ordering::Relaxed) + 1,
            fail_count: self.fail_count.load(atomic::Ordering::Relaxed),
        });
    }

    pub fn dispatch_fail(&self) {
        self.emitter.emit(ProgressChunk::AssetsSave {
            total_assets_count: self.total_count,
            success_count: self.done_count.load(atomic::Ordering::Relaxed),
            fail_count: self.fail_count.fetch_add(1, atomic::Ordering::Relaxed) + 1,
        });
    }
}
