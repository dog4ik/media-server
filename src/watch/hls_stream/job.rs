use std::{path::PathBuf, sync::Arc};

use tokio::sync::{mpsc, oneshot};

use crate::{library::Video, progress::ProgressDispatcher, watch::WatchTask};

use super::{
    HlsTempPath,
    command::{self, AUDIO_CODEC, DEFAULT_SEGMENT_LENGTH, VIDEO_ENCODER},
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
}

impl HlsJobHandle {
    pub async fn request_segment(&self, idx: usize) -> PathBuf {
        let (tx, rx) = oneshot::channel();
        self.request
            .send(Request {
                kind: RequestKind::Segment(idx),
                ready: tx,
            })
            .await
            .unwrap();
        rx.await.unwrap();
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .join("0")
            .join(format!("{idx}.mp4"))
    }

    pub async fn request_init(&self) -> PathBuf {
        let (tx, rx) = oneshot::channel();
        self.request
            .send(Request {
                kind: RequestKind::Init,
                ready: tx,
            })
            .await
            .unwrap();
        rx.await.unwrap();
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tmp")
            .join("0")
            .join(format!("init.mp4"))
    }

    pub fn playlist(&self) -> &str {
        &self.manifest.inner
    }
}

pub async fn start(
    video: &Video,
    tmp_path: HlsTempPath,
    id: String,
    progress_dispatcher: ProgressDispatcher<WatchTask>,
) -> anyhow::Result<HlsJobHandle> {
    let target_path = video.path().to_path_buf();
    let duration = video.fetch_duration().await?;
    tracing::debug!(path = %target_path.display(), "Hls job input path");
    tracing::debug!(path = %tmp_path.0.display(), "Hls job temporary path");
    tokio::fs::create_dir_all(&tmp_path.0).await.unwrap();
    let (_watcher, mut file_change_rx) = spawn_watcher(&tmp_path.0).unwrap();

    let video_codec_copy = false;
    let child = command::run(
        &target_path,
        &tmp_path.0,
        0,
        0.,
        VIDEO_ENCODER,
        None,
        AUDIO_CODEC,
        video_codec_copy,
    )
    .unwrap();

    let (request_tx, mut request_rx) = mpsc::channel::<Request>(100);

    let playlist = if video_codec_copy {
        match keyframe::retrieve_keyframes(&target_path, 0, DEFAULT_SEGMENT_LENGTH as f64).await {
            Ok(k) => {
                println!("exracted {} keyframes", k.key_frames.len());
                M3U8Manifest::from_keyframes(k, &id)
            }
            Err(e) => {
                println!("failed to extract keyframes: {e}");
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
    tokio::spawn(async move {
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

                    // We have that segment
                    if segments_len > 0 && req.idx >= start_segment && req.idx < start_segment +  segments_len {
                        println!("requested exisiting segment {}", req.idx);
                        let _ = req.ready.send(());
                        continue;
                    } else if req.idx < start_segment || req.idx > start_segment + segments_len + JOB_RESET_SEGMENT_THRESHOLD {
                        println!("Segment {} is out of reach, resetting the job", req.idx);
                        child.kill().await.unwrap();
                        child = command::run(
                            &target_path,
                            &tmp_path.0,
                            req.idx,
                            manifiest.seek_time(req.idx),
                            VIDEO_ENCODER,
                            None,
                            AUDIO_CODEC,
                            video_codec_copy
                        )
                        .unwrap();

                        start_segment = req.idx;
                        segments_len = 0;
                        requests.clear();
                        requests.push(req);
                    } else {
                        // We client seeked outside the range
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
            }
        }
    });

    Ok(HlsJobHandle {
        request: request_tx,
        manifest: playlist,
    })
}
