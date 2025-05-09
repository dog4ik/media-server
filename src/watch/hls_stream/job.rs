use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use tokio::sync::{mpsc, oneshot};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    config,
    library::Video,
    progress::ProgressDispatcher,
    watch::{WatchTask, hls_stream::command::CommandArgumentsParams},
};

use super::{
    HlsStreamConfiguration, HlsTempPath,
    command::{self, DEFAULT_SEGMENT_LENGTH},
    file_watcher::spawn_watcher,
    keyframe,
};

use super::manifest::M3U8Manifest;

/// If requested segment > current segment + this value, we reset transcoding job.
pub const JOB_RESET_SEGMENT_THRESHOLD: usize = 6;

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

async fn cleanup_temp_dir(path: &Path) -> std::io::Result<()> {
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
    let ffmpeg_path: config::FFmpegPath = config::CONFIG.get_value();
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
    tokio::fs::create_dir_all(&tmp_path.0).await?;
    let (_watcher, file_change_rx) = spawn_watcher(&tmp_path.0)?;

    let video_codec_copy = config.video_encoder.is_none();
    let args = CommandArgumentsParams {
        ffmpeg_path: ffmpeg_path.0,
        video_path: target_path,
        video_track_idx: config.video_track,
        audio_track_idx: config.audio_track,
        temp_path: tmp_path.0.to_path_buf(),
        task_id: id,
        start: 0,
        seek_to: 0.,
        video_encoder: config.video_encoder.unwrap_or("copy".to_string()),
        framerate: avg_framerate,
        audio_codec: config.audio_encoder.unwrap_or("copy".to_string()),
        copy_video: video_codec_copy,
    };
    let child = command::run(&args)?;

    let (request_tx, request_rx) = mpsc::channel::<Request>(100);

    let playlist = if video_codec_copy {
        match keyframe::retrieve_keyframes(&args.video_path, args.video_track_idx).await {
            Ok(k) => {
                tracing::debug!("Exracted {} keyframes", k.key_frames.len());
                M3U8Manifest::from_keyframes(k, &args.task_id, duration)
            }
            Err(e) => {
                tracing::error!("Failed to extract keyframes: {e}");
                M3U8Manifest::from_interval(
                    DEFAULT_SEGMENT_LENGTH as f64,
                    duration.as_secs_f64(),
                    &args.task_id,
                )
            }
        }
    } else {
        M3U8Manifest::from_interval(
            DEFAULT_SEGMENT_LENGTH as f64,
            duration.as_secs_f64(),
            &args.task_id,
        )
    };

    let playlist = Arc::new(playlist);

    let manifest = playlist.clone();
    tracker.spawn(async move {
        let _watcher = _watcher;
        match run_hls_handler(
            args,
            child,
            manifest,
            progress_dispatcher,
            request_rx,
            file_change_rx,
            exit_token,
        )
        .await
        {
            Ok(_) => {}
            Err(e) => {
                tracing::error!("Hls task runner errored: {e}");
            }
        }
    });

    Ok(HlsJobHandle {
        request: request_tx,
        manifest: playlist,
        path: tmp_path,
    })
}

async fn run_hls_handler(
    mut args: CommandArgumentsParams,
    mut child: tokio::process::Child,
    manifest: Arc<M3U8Manifest>,
    progress_dispatcher: ProgressDispatcher<WatchTask>,
    mut request_rx: mpsc::Receiver<Request>,
    mut file_change_rx: mpsc::Receiver<PathBuf>,
    exit_token: CancellationToken,
) -> anyhow::Result<()> {
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

                // Incorrect:
                // progress_dispatcher.progress(
                //     WatchProgress {
                //         current_time: Duration::from_secs((req.idx * DEFAULT_SEGMENT_LENGTH) as u64)
                //     }
                // );
                // Requested segment is always ahead of the current watch time
                // we can calculate default hls.js buffer size though.

                // We have that segment
                if segments_len > 0 && req.idx >= start_segment && req.idx < start_segment +  segments_len {
                    tracing::trace!("Requested existing segment {}", req.idx);
                    let _ = req.ready.send(());
                    continue;
                } else if req.idx < start_segment || req.idx > start_segment + segments_len + JOB_RESET_SEGMENT_THRESHOLD {
                    tracing::debug!("Segment {} is out of reach, resetting the job", req.idx);
                    child.kill().await?;
                    while file_change_rx.try_recv().is_ok() {}
                    args.start = req.idx;
                    args.seek_to = manifest.seek_time(req.idx);
                    child = command::run(&args)?;

                    start_segment = req.idx;
                    segments_len = 0;
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
                    .expect("segment must have a filename")
                    .to_str()
                    .expect("utf-8 filename")
                    .parse::<usize>()
                else {
                    debug_assert_eq!(path.file_stem(), Some(std::ffi::OsStr::new("init")));
                    have_init = true;
                    for waiter in init_waiters.drain(..) {
                        let _ = waiter.send(());
                    }
                    continue;
                };
                segments_len = new_segment - start_segment;

                while let Some(ready_idx) = requests.iter().position(|r| r.idx < start_segment + segments_len) {
                    let ready = requests.swap_remove(ready_idx);
                    let _ = ready.ready.send(());
                }
            }
            _ = exit_token.cancelled() => {
                child.kill().await?;
                progress_dispatcher.finish();
                if let Err(e) = cleanup_temp_dir(&args.temp_path).await {
                    tracing::error!("Failed to clean up hls temp directory: {e}");
                }
                return Ok(());
            }
        }
    }
}
