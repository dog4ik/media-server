use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::Stdio,
};

use std::process::Command;

use anyhow::Context;
use tokio::io::{AsyncBufReadExt, BufReader};

pub const DEFAULT_SEGMENT_LENGTH: usize = 6;

fn apply_video_arguments(c: &mut Command, codec: &str) {
    c.arg("-c:v:0");
    c.arg(codec);

    c.arg("-pix_fmt");
    c.arg("yuv420p");

    // if codec != "copy" {
    //     c.arg("-flags");
    //     c.arg("+cgop");
    //
    //     c.arg("-g");
    //     c.arg("30");
    // }
}

fn apply_audio_arguments(c: &mut Command, codec: &str) {
    c.arg("-c:a");
    c.arg(codec);

    if codec == "aac" {
        c.arg("-ac");
        c.arg("2");
    }
}

fn apply_keyframes_arguments(
    c: &mut Command,
    codec: &str,
    gop_frames: Option<usize>,
    segment_duration: f64,
) {
    let add_keyframe_args = |c: &mut Command| {
        c.arg("-force_key_frames:0");
        // `t` is relative to the output start, so a keyframe is forced every segment_duration
        // aligns the boundaries with the manifest grid.
        c.arg(format!("expr:gte(t,n_forced*{})", segment_duration));
    };

    let add_gop_args = |c: &mut Command| {
        if let Some(gop_frames) = gop_frames {
            c.arg("-g:v:0");
            c.arg(gop_frames.to_string());
            c.arg("-keyint_min:v:0");
            c.arg(gop_frames.to_string());
        }
    };

    match codec {
        // Unable to force key frames using these encoders, set key frames by GOP.
        "h264_qsv" | "h264_nvenc" | "h264_amf" | "h264_rkmpp" | "hevc_qsv" | "hevc_nvenc"
        | "hevc_rkmpp" | "av1_qsv" | "av1_nvenc" | "av1_amf" | "libsvtav1" => add_gop_args(c),

        "libx264" | "libx265" | "h264_vaapi" | "hevc_vaapi" | "av1_vaapi" => {
            add_keyframe_args(c);
            // prevent the libx264 from post processing to break the set keyframe.
            if codec == "libx264" {
                c.arg("-sc_threshold:v:0");
                c.arg("0");
            }
        }
        _ => {
            add_keyframe_args(c);
            add_gop_args(c);
        }
    }

    // // global_header produced by AMD HEVC VA-API encoder causes non-playable fMP4 on iOS
    // if (string.Equals(codec, "hevc_vaapi", StringComparison.OrdinalIgnoreCase)
    // && _mediaEncoder.IsVaapiDeviceAmd)
    // {
    //     args += " -flags:v -global_header";
    // }
}

#[derive(Debug)]
pub(super) struct CommandArgumentsParams {
    pub ffmpeg_path: PathBuf,
    pub video_path: PathBuf,
    pub video_track_idx: usize,
    pub audio_track_idx: usize,
    pub temp_path: PathBuf,
    pub task_id: String,
    pub start: usize,
    pub seek_to: f64,
    pub video_encoder: String,
    /// Number of frames per GOP. `None` when the frame rate is
    /// unknown or the video is copied.
    pub gop_frames: Option<usize>,
    /// Frame-aligned segment duration in seconds (matches the manifest grid).
    pub segment_duration: f64,
    pub audio_codec: String,
    pub copy_video: bool,
}

#[allow(unused)]
#[derive(Debug, Default)]
struct A(pub Vec<OsString>);
impl A {
    #[allow(unused)]
    pub fn arg(&mut self, a: impl AsRef<OsStr>) {
        let s = OsString::from(a.as_ref());
        self.0.push(s);
    }
}

pub(super) fn run(
    CommandArgumentsParams {
        ffmpeg_path,
        video_path,
        video_track_idx,
        audio_track_idx,
        temp_path,
        task_id: _,
        start,
        seek_to,
        video_encoder,
        gop_frames,
        segment_duration,
        audio_codec,
        copy_video,
    }: &CommandArgumentsParams,
) -> anyhow::Result<tokio::process::Child> {
    let mut c = Command::new(ffmpeg_path);
    c.current_dir(temp_path);

    c.arg("-hide_banner");
    c.arg("-loglevel");
    c.arg("error");

    c.arg("-ss");
    let seek_time = format!("{:.6}", seek_to);
    c.arg(&seek_time);
    if *copy_video {
        // Copy can't decode-and-discard to a frame boundary; the seek lands on the source
        // keyframe, which is exactly the keyframe-based manifest boundary. Re-encoding uses
        // accurate seek so the first output frame is exactly on the segment boundary.
        c.arg("-noaccurate_seek");
    }

    c.arg("-fflags");
    c.arg("+genpts");

    c.arg("-i");
    c.arg(video_path);

    c.arg("-map");
    c.arg(format!("0:{video_track_idx}"));

    c.arg("-map");
    c.arg(format!("0:{audio_track_idx}"));

    apply_video_arguments(&mut c, if *copy_video { "copy" } else { video_encoder });
    if !*copy_video {
        apply_keyframes_arguments(&mut c, video_encoder, *gop_frames, *segment_duration);
    }
    apply_audio_arguments(&mut c, audio_codec);

    c.arg("-copyts");

    c.arg("-avoid_negative_ts");
    c.arg("disabled");

    c.arg("-max_muxing_queue_size");
    c.arg("2084");

    c.arg("-f");
    c.arg("hls");

    c.arg("-max_delay");
    c.arg("5000000");

    c.arg("-hls_time");
    c.arg(DEFAULT_SEGMENT_LENGTH.to_string());

    c.arg("-hls_segment_type");
    c.arg("fmp4");
    c.arg("-hls_fmp4_init_filename");
    c.arg("init.mp4");

    // Segments are written to `{idx}.mp4.tmp` and renamed once complete, which is the only
    // completion signal available from file watchers on every platform.
    c.arg("-hls_flags");
    c.arg("temp_file");

    c.arg("-start_number");
    c.arg(start.to_string());

    c.arg("-hls_segment_filename");
    c.arg("%d.mp4");

    c.arg("-hls_playlist_type");
    c.arg("vod");

    c.arg("-hls_list_size");
    c.arg("0");

    c.arg("-y");

    c.arg("playlist.m3u8");

    let dbg_args: Vec<_> = c.get_args().map(|v| v.to_string_lossy()).collect();

    tracing::debug!(
        full_args = %dbg_args.join(" "),
        audio_codec,
        video_encoder,
        seek_offset = seek_time,
        start_segment = start,
        "Started hls ffmpeg command"
    );

    let mut c = tokio::process::Command::from(c);
    #[cfg(windows)]
    {
        c.creation_flags(crate::utils::CREATE_NO_WINDOW);
    }

    let mut child = c
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("spawn hls ffmpeg command")?;

    let stderr = child.stderr.take().expect("stderr is not taken");
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::warn!("Hls ffmpeg: {line}");
        }
    });
    Ok(child)
}
