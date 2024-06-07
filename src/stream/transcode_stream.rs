use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::Context;
use tokio::{
    fs,
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    select,
    sync::{mpsc, oneshot},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::config::APP_RESOURCES;

#[derive(Debug, Clone)]
pub struct M3U8Manifest {
    inner: String,
}

impl M3U8Manifest {
    const MANIFEST_HEADER: &'static str = r#"#EXTM3U
#EXT-X-VERSION:3
#EXT-X-MEDIA-SEQUENCE:0
#EXT-X-ALLOW-CACHE:NO
#EXT-X-PLAYLIST-TYPE:VOD
"#;

    pub fn from_key_frames(frames: &KeyFrames, id: String) -> Self {
        use std::fmt::Write;
        let mut max_duration = 0.;
        let mut parts = String::new();
        for (i, key_frame) in frames.key_frames.iter().enumerate() {
            let next = match frames.key_frames.get(i + 1) {
                Some(f) => f.time,
                None => frames.last_frame.time,
            };
            let duration = next - key_frame.time;
            if duration > max_duration {
                max_duration = duration;
            }
            writeln!(&mut parts, "#EXTINF:{:.6},", duration).unwrap();
            writeln!(&mut parts, "/api/transcode/{id}/segment/{i}").unwrap();
        }

        let mut manifest: String = Self::MANIFEST_HEADER.into();
        writeln!(&mut manifest, "#EXT-X-TARGETDURATION:{:.6}", max_duration).unwrap();
        writeln!(&mut manifest, "{}", parts).unwrap();
        writeln!(&mut manifest, "#EXT-X-ENDLIST").unwrap();
        Self { inner: manifest }
    }

    pub async fn save(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        use tokio::fs;
        fs::create_dir_all(path.as_ref().parent().unwrap()).await?;
        let mut manifest_file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await
            .context("open manifest file")?;
        manifest_file
            .write_all(self.inner.as_bytes())
            .await
            .context("write manifest")?;
        Ok(())
    }

    pub fn from_interval(interval: f64, mut duration: f64, id: String) -> Self {
        use std::fmt::Write;
        let mut manifest: String = Self::MANIFEST_HEADER.into();
        writeln!(&mut manifest, "#EXT-X-TARGETDURATION:{:.6}", interval).unwrap();
        let mut i = 0;
        while duration > 0. {
            let time = if duration - interval >= 0. {
                interval
            } else {
                duration
            };
            writeln!(&mut manifest, "#EXTINF:{:.6},", time).unwrap();
            writeln!(&mut manifest, "/api/transcode/{id}/segment/{i}").unwrap();
            i += 1;
            duration -= interval;
        }
        writeln!(&mut manifest, "#EXT-X-ENDLIST").unwrap();
        Self { inner: manifest }
    }
}

impl AsRef<str> for M3U8Manifest {
    fn as_ref(&self) -> &str {
        self.inner.as_str()
    }
}

#[derive(Debug, Clone)]
pub struct KeyFrames {
    pub key_frames: Vec<Frame>,
    pub last_frame: Frame,
}

impl KeyFrames {
    pub fn closest_frame_with_offset(&self, position: u64) -> &Frame {
        let idx = match self
            .key_frames
            .binary_search_by(|k| k.position.cmp(&position))
        {
            Ok(idx) => idx,
            Err(insert_idx) => insert_idx.checked_sub(1).unwrap_or(0),
        };
        &self.key_frames[idx]
    }

    pub fn duration(&self, idx: usize) -> f64 {
        let target = self.key_frames[idx];
        if let Some(next) = self.key_frames.get(idx + 1) {
            next.time - target.time
        } else {
            self.last_frame.time - target.time
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Frame {
    pub time: f64,
    position: u64,
    is_key: bool,
}

impl Frame {
    pub fn from_ffprobe_csv_output_line(line: String) -> Option<Self> {
        let mut split = line.splitn(3, ',');
        let time = split.next()?.parse().ok()?;
        let position = split.next()?.parse().ok()?;
        let is_key = split.next()?.starts_with('K');
        Some(Self {
            time,
            position,
            is_key,
        })
    }
}

pub async fn retrieve_keyframes(
    input_file: impl AsRef<Path>,
    video_track: usize,
    min_delay: f64,
) -> anyhow::Result<KeyFrames> {
    let ffprobe = APP_RESOURCES
        .get()
        .unwrap()
        .ffprobe_path
        .as_ref()
        .context("ffprobe path")?;
    let mut child = tokio::process::Command::new(ffprobe)
        .args(&[
            "-loglevel",
            "error",
            "-select_streams",
            &format!("v:{}", video_track),
            "-show_entries",
            "packet=pts_time,flags,pos",
            "-of",
            "csv=print_section=0",
            input_file.as_ref().to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("spawn ffprobe command")?;
    let stdout = child.stdout.take().expect("std out is not taken");
    let mut lines = BufReader::new(stdout).lines();
    let mut key_frames: Vec<Frame> = Vec::new();
    let mut last_frame = None;
    while let Ok(Some(line)) = lines.next_line().await {
        if let Some(frame) = Frame::from_ffprobe_csv_output_line(line) {
            last_frame = Some(frame);
            if frame.is_key {
                if let Some(last) = key_frames.last() {
                    let delta = frame.time - last.time;
                    if delta < min_delay {
                        continue;
                    }
                };
                key_frames.push(frame);
            }
        };
    }
    Ok(KeyFrames {
        key_frames,
        last_frame: last_frame.unwrap(),
    })
}

#[derive(Debug, Clone)]
pub struct TranscodeStream {
    pub video_id: i64,
    pub target_path: PathBuf,
    pub uuid: uuid::Uuid,
    pub key_frames: KeyFrames,
    pub manifest: M3U8Manifest,
    pub sender: mpsc::Sender<(usize, oneshot::Sender<bytes::Bytes>)>,
    pub cancellation_token: CancellationToken,
}

impl TranscodeStream {
    pub async fn init(
        video_id: i64,
        target_path: PathBuf,
        tracker: TaskTracker,
        cancellation_token: CancellationToken,
    ) -> anyhow::Result<Self> {
        let uuid = uuid::uuid!("00000000-0000-0000-0000-ffff00000000");
        let temp_path = PathBuf::from("transcode").join(uuid.to_string());
        let key_frames = retrieve_keyframes(&target_path, 0, 2.0).await?;
        let manifest = M3U8Manifest::from_key_frames(&key_frames, uuid.to_string());
        let manifest_path = temp_path.join("manifest.m3u8");
        manifest.save(manifest_path).await.unwrap();
        let (tx, rx) = mpsc::channel(100);

        tracker.spawn(transcode_stream(
            target_path.clone(),
            temp_path,
            key_frames.clone(),
            cancellation_token.clone(),
            rx,
        ));

        Ok(Self {
            video_id,
            target_path,
            key_frames,
            uuid,
            manifest,
            sender: tx,
            cancellation_token,
        })
    }
}

async fn transcode_stream(
    target: PathBuf,
    temp_path: PathBuf,
    key_frames: KeyFrames,
    cancellation_token: CancellationToken,
    mut rx: mpsc::Receiver<(usize, oneshot::Sender<bytes::Bytes>)>,
) {
    let mut history = HashSet::new();
    let mut pending_requests: HashMap<usize, oneshot::Sender<bytes::Bytes>> = HashMap::new();
    pending_requests.retain(|_, v| !v.is_closed());
    let list_path = temp_path.join("list");
    let mut start_index = 0;
    let spawn_command = |current_index: usize| {
        let frame = key_frames.key_frames[current_index];
        let times: Vec<_> = key_frames
            .key_frames
            .iter()
            .map(|f| format!("{:.6}", f.time))
            .collect();
        let segment_times = times.join(",");
        let mut command = tokio::process::Command::new("ffmpeg")
            .args(&[
                "-hide_banner",
                "-ss",
                &format!("{:.6}", frame.time),
                "-copyts",
                "-i",
                &target.to_string_lossy(),
                "-c:v",
                "h264",
                "-c:a",
                "aac",
                "-f",
                "segment",
                "-segment_list",
                &list_path.to_string_lossy(),
                "-segment_list_type",
                "flat",
                "-segment_times",
                &segment_times,
                "-segment_format",
                "mpegts",
                "-segment_start_number",
                &current_index.to_string(),
                &temp_path.join("%d.ts").to_string_lossy(),
                "-y",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stderr = command.stderr.take().expect("stderr is not taken");
        let lines = BufReader::new(stderr).lines();
        (lines, command)
    };
    let (mut log, mut command) = spawn_command(start_index);
    loop {
        select! {
            Some((idx, res)) = rx.recv() => {
                tracing::trace!("REQUESTED segment idx: {}", idx);
                if history.contains(&idx) {
                    tracing::trace!("Sending ready segment: {}", idx);
                    let segment = retrieve_segment(&temp_path, idx).await.unwrap();
                    let _ = res.send(segment);
                    continue;
                }
                pending_requests.insert(idx, res);
                if idx < start_index || idx > start_index + history.len() + 2  {
                    tracing::warn!(
                        "Out of order request: got {idx}, expected: {} <= idx < {}",
                        start_index,
                        start_index + history.len() + 5
                    );
                    command.kill().await.unwrap();
                    let _ = tokio::fs::remove_file(&list_path).await;
                    history.clear();
                    pending_requests.retain(|_, v| !v.is_closed());
                    let behind = idx.checked_sub(0).unwrap_or(0);
                    (log, command) = spawn_command(behind);
                    start_index = behind;
                }
            },
            Ok(Some(line)) = log.next_line() => {
                match process_log_line(line, &mut history, &list_path).await {
                    Ok(have_new) => {
                        if have_new {
                            tracing::debug!("Log scanner detected a new segment");
                            pending_requests.retain(|_, v| !v.is_closed());
                            let ready_segments: Vec<_> = pending_requests
                                .keys()
                                .filter(|idx| history.contains(idx))
                                .copied()
                                .collect();
                            for segment_idx in ready_segments {
                                let sender = pending_requests.remove(&segment_idx).unwrap();
                                let segment = retrieve_segment(&temp_path, segment_idx).await.unwrap();
                                let _ = sender.send(segment);
                            }
                        }
                    },
                    Err(e) => tracing::error!("Log scanner error: {e}"),
                };
            },
            _ = cancellation_token.cancelled() => {
                if let Err(e) = command.kill().await {
                    tracing::error!("Failed to kill running transcode command: {e}")
                };
                if let Err(e) = fs::remove_dir_all(temp_path).await {
                    tracing::error!("Failed to clean up transcode stream temp files: {e}")
                };
                break;
            }
        }
    }
}

async fn retrieve_segment(
    segments_path: impl AsRef<Path>,
    idx: usize,
) -> anyhow::Result<bytes::Bytes> {
    use tokio::fs;
    let mut dir = fs::read_dir(&segments_path).await.unwrap();
    while let Ok(Some(entry)) = dir.next_entry().await {
        let path = entry.path();
        if let (Some(name), Some(ext)) = (
            path.file_stem().and_then(OsStr::to_str),
            path.extension().and_then(OsStr::to_str),
        ) {
            if ext == "ts" {
                let name: usize = name.parse().expect("all segments have numeric name");
                if name == idx {
                    let path = segments_path.as_ref().join(format!("{idx}.ts"));
                    let mut file = tokio::fs::File::open(&path).await?;
                    let len = file.metadata().await?.len();
                    let mut out = Vec::with_capacity(len as usize);
                    file.read_to_end(&mut out).await?;
                    return Ok(out.into());
                }
            }
        }
    }
    Err(anyhow::anyhow!("chunk with index {idx} is not found"))
}

async fn process_log_line(
    line: String,
    history: &mut HashSet<usize>,
    list_path: impl AsRef<Path>,
) -> anyhow::Result<bool> {
    let mut have_new = false;
    if line.contains("Opening") {
        let list = fs::read_to_string(list_path).await?;
        let segments = list.lines();
        for (stem, ext) in segments.filter_map(|s| s.split_once('.')) {
            if ext == "ts" {
                let idx = stem.parse().expect("all generated files have numeric name");
                if history.insert(idx) {
                    have_new = true;
                };
            }
        }
    }
    Ok(have_new)
}
