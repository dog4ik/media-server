use std::{path::Path, process::Stdio};

use tokio::process::{self, Command};

pub const DEFAULT_SEGMENT_LENGTH: usize = 3;
/// Segment if segment gap is higher than this we start new transcoding job.
pub const ALLOWED_SEGMENT_GAP: usize = 5;
pub const VIDEO_ENCODER: &str = "libx264";
pub const AUDIO_CODEC: &str = "aac";

fn apply_video_arguments(c: &mut Command, codec: &str) {
    c.arg("-c:v:0");
    c.arg(codec);

    c.arg("-pix_fmt");
    c.arg("yuv420p");

    c.arg("-flags");
    c.arg("+cgop");

    c.arg("-g");
    c.arg("30");
}

fn apply_audio_arguments(c: &mut Command, codec: &str) {
    c.arg("-c:a");
    c.arg(codec);

    if codec == "aac" {
        c.arg("-bsf:a");
        c.arg("aac_adtstoasc");
    }
}

fn apply_keyframes_arguments(c: &mut Command, codec: &str, framerate: Option<usize>) {
    let add_keyframe_args = |c: &mut Command| {
        c.arg("-force_key_frames:0");
        c.arg(format!("expr:gte(t,n_forced*{})", DEFAULT_SEGMENT_LENGTH));
    };

    let add_gop_args = |c: &mut Command| {
        if let Some(framerate) = framerate {
            c.arg("-g:v:0");
            // Math.ceil it
            let frame_amount = DEFAULT_SEGMENT_LENGTH * framerate;
            c.arg(frame_amount.to_string());
            c.arg("-keyint_min:v:0");
            c.arg(frame_amount.to_string());
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

pub fn run(
    video_path: &Path,
    video_track_idx: usize,
    audio_track_idx: usize,
    temp_path: &Path,
    task_id: &str,
    start: usize,
    seek_to: f64,
    video_encoder: &str,
    framerate: Option<usize>,
    audio_codec: &str,
    copy_video: bool,
) -> anyhow::Result<process::Child> {
    let mut c = tokio::process::Command::new("ffmpeg");
    let segment_file_name = format!("{}/%d.mp4", temp_path.display());
    c.arg("-hide_banner");
    c.arg("-progress");
    c.arg("-");

    c.arg("-ss");
    let seek_time = format!("{:.6}", seek_to);
    c.arg(&seek_time);

    c.arg("-i");
    c.arg(video_path);

    c.arg("-map");
    c.arg(format!("0:{video_track_idx}"));

    c.arg("-map");
    c.arg(format!("0:{audio_track_idx}"));

    apply_video_arguments(&mut c, if copy_video { "copy" } else { video_encoder });
    if copy_video {
        c.arg("-start_at_zero");
    } else {
        apply_keyframes_arguments(&mut c, video_encoder, framerate);
    }
    apply_audio_arguments(&mut c, audio_codec);
    c.arg("-sn");

    c.arg("-copyts");

    c.arg("-avoid_negative_ts");
    c.arg("disabled");
    c.arg("-max_muxing_queue_size");
    c.arg("1");

    c.arg("-f");
    c.arg("hls");

    c.arg("-max_delay");
    c.arg("5000000");

    c.arg("-hls_time");
    c.arg(DEFAULT_SEGMENT_LENGTH.to_string());

    c.arg("-hls_segment_type");
    c.arg("fmp4");
    c.arg("-hls_fmp4_init_filename");
    let init_filename = format!("{}/init.mp4", task_id);
    c.arg(&init_filename);

    c.arg("-start_number");
    let start_number = start.to_string();
    c.arg(&start_number);

    c.arg("-hls_segment_filename");
    c.arg(&segment_file_name);

    c.arg("-hls_playlist_type");
    c.arg("vod");

    c.arg("-hls_list_size");
    c.arg("0");

    c.arg("-progress");
    c.arg("pipe:1");

    c.arg("-y");

    c.arg(temp_path);

    tracing::debug!(
        audio_codec,
        video_encoder,
        seek_offset = seek_time,
        start_segment = start_number,
        "Started hls ffmpeg command"
    );

    let child = c
        .stderr(Stdio::null())
        .stdout(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    Ok(child)
}
