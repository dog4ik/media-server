use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::sync::{mpsc, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{library::Video, progress::ProgressDispatcher, watch::WatchTask};

use super::{
    HlsStreamConfiguration, HlsTempPath,
    command::{self, DEFAULT_SEGMENT_LENGTH},
    file_watcher::spawn_watcher,
    keyframe,
};

use super::manifest::M3U8Manifest;

pub const JOB_RESET_SEGMENT_THRESHOLD: usize = 3;

#[derive(Debug)]
pub enum RequestKind {
    Init,
    Segment(usize),
}

#[derive(Debug)]
pub struct Request {
    kind: RequestKind,
    ready: oneshot::Sender<()>,
}

#[derive(Debug)]
pub struct SegmentRequest {
    idx: usize,
    ready: oneshot::Sender<()>,
}

#[derive(Debug, Clone)]
pub struct HlsJobHandle {
    request: mpsc::Sender<Request>,
    manifest: Arc<M3U8Manifest>,
    path: HlsTempPath,
}

impl HlsJobHandle {
    pub async fn request_segment(&self, idx: usize) -> anyhow::Result<PathBuf> {
        let (tx, rx) = oneshot::channel();
        self.request
            .send(Request {
                kind: RequestKind::Segment(idx),
                ready: tx,
            })
            .await?;
        rx.await?;
        Ok(self.path.segment_path(idx))
    }

    pub async fn request_init(&self) -> anyhow::Result<PathBuf> {
        let (tx, rx) = oneshot::channel();
        self.request
            .send(Request {
                kind: RequestKind::Init,
                ready: tx,
            })
            .await?;
        rx.await?;
        Ok(self.path.init_path())
    }

    pub fn playlist(&self) -> &str {
        &self.manifest.inner
    }
}

pub async fn clean_up_dir(path: &Path) -> std::io::Result<()> {
    tokio::fs::remove_dir_all(path).await?;
    tracing::debug!(path = %path.display(), "Cleaned up hls temp directory");
    Ok(())
}

pub async fn start(
    video: &Video,
    config: HlsStreamConfiguration,
    tmp_path: HlsTempPath,
    id: String,
    progress_dispatcher: ProgressDispatcher<WatchTask>,
    exit_token: CancellationToken,
    tracker: TaskTracker,
) -> anyhow::Result<HlsJobHandle> {
    let target_path = video.path().to_path_buf();
    let video_metadata = video.metadata().await?;
    let duration = video_metadata.duration();
    let avg_framerate = video_metadata
        .default_video()
        .map(|v| v.avg_frame_rate as usize);
    tracing::debug!(path = %target_path.display(), "Hls job input path");
    tracing::debug!(path = %tmp_path.0.display(), "Hls job temporary path");
    tracing::debug!("Hls job duration is {} mins", duration.as_secs_f32() / 60.);
    if let Some(framerate) = avg_framerate {
        tracing::debug!("Hls job avg framerate: {}/s", framerate);
    }
    tokio::fs::create_dir_all(&tmp_path.0).await.unwrap();
    let (_watcher, mut file_change_rx) = spawn_watcher(&tmp_path.0).unwrap();

    let video_codec_copy = false;
    let child = command::run(
        &target_path,
        config.video_track,
        config.audio_track,
        &tmp_path.0,
        &id,
        0,
        0.,
        config.video_encoder.as_ref().map_or("copy", String::as_str),
        avg_framerate,
        config.audio_encoder.as_ref().map_or("copy", String::as_str),
        video_codec_copy,
    )?;

    let (request_tx, mut request_rx) = mpsc::channel::<Request>(100);

    let playlist = if video_codec_copy {
        match keyframe::retrieve_keyframes(&target_path, 0, DEFAULT_SEGMENT_LENGTH as f64).await {
            Ok(k) => {
                tracing::debug!("Exracted {} keyframes", k.key_frames.len());
                M3U8Manifest::from_keyframes(k, &id)
            }
            Err(e) => {
                tracing::error!("Failed to extract keyframes: {e}");
                M3U8Manifest::from_interval(
                    DEFAULT_SEGMENT_LENGTH as f64,
                    duration.as_secs_f64(),
                    &id,
                )
            }
        }
    } else {
        M3U8Manifest::from_interval(DEFAULT_SEGMENT_LENGTH as f64, duration.as_secs_f64(), &id)
    };
    let playlist = Arc::new(playlist);

    let manifiest = playlist.clone();
    let root_path = tmp_path.0.clone();
    tracker.spawn(async move {
        let _watcher = _watcher;
        let mut child = child;
        let mut requests: Vec<SegmentRequest> = Vec::new();
        let mut init_waiters: Vec<oneshot::Sender<()>> = Vec::new();
        let mut start_segment = 0;
        let mut segments_len = 0;
        let mut have_init = false;
        loop {
            tokio::select! {
                Some(req) = request_rx.recv() => {
                    let req = match req.kind {
                        RequestKind::Init if have_init => {
                            let _ = req.ready.send(());
                            continue;
                        },
                        RequestKind::Init => {
                            init_waiters.push(req.ready);
                            continue;
                        },
                        RequestKind::Segment(s) => SegmentRequest { idx: s, ready: req.ready },
                    };

                    // progress_dispatcher.progress(
                    //     WatchProgress {
                    //         current_time: Duration::from_secs((req.idx * DEFAULT_SEGMENT_LENGTH) as u64)
                    //     }
                    // );

                    // We have that segment
                    if segments_len > 0 && req.idx >= start_segment && req.idx < start_segment +  segments_len {
                        tracing::trace!("Requested existing segment {}", req.idx);
                        let _ = req.ready.send(());
                        continue;
                    } else if req.idx < start_segment || req.idx > start_segment + segments_len + JOB_RESET_SEGMENT_THRESHOLD {
                        tracing::debug!("Segment {} is out of reach, resetting the job", req.idx);
                        child.kill().await.unwrap();
                        child = command::run(
                            &target_path,
                            config.video_track,
                            config.audio_track,
                            &root_path,
                            &id,
                            req.idx,
                            manifiest.seek_time(req.idx),
                            config.video_encoder.as_ref().map_or("copy", String::as_str),
                            avg_framerate,
                            config.audio_encoder.as_ref().map_or("copy", String::as_str),
                            video_codec_copy
                        )
                        .unwrap();

                        start_segment = req.idx;
                        segments_len = 0;
                        requests.clear();
                        requests.push(req);
                    } else {
                        // client sought outside the range
                        // reset is needed
                        requests.push(req);
                    }
                }
                Some(path) = file_change_rx.recv() => {
                    requests.retain(|r| !r.ready.is_closed());
                    let Ok(new_segment)= path
                        .file_stem()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .parse::<usize>()
                    else {
                        debug_assert_eq!(path.file_stem(), Some(std::ffi::OsStr::new("init")));
                        have_init = true;
                        for waiter in init_waiters.drain(..) {
                            let _ = waiter.send(());
                        }
                        continue;
                    };
                    segments_len = new_segment.saturating_sub(start_segment);

                    while let Some(ready_idx) = requests.iter().position(|r| r.idx < start_segment + segments_len) {
                        let ready = requests.swap_remove(ready_idx);
                        let _ = ready.ready.send(());
                    }
                }
                _ = exit_token.cancelled() => {
                    child.kill().await.unwrap();
                    progress_dispatcher.finish();
                    if let Err(e) = clean_up_dir(&root_path).await {
                        tracing::error!("Failed to clean up hls temp directory: {e}");
                    }
                    return;
                }
            }
        }
    });

    Ok(HlsJobHandle {
        request: request_tx,
        manifest: playlist,
        path: tmp_path,
    })
}
