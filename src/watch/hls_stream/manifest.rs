use std::time::Duration;

use crate::watch::hls_stream::command::DEFAULT_SEGMENT_LENGTH;

use super::keyframe::KeyFrames;

#[derive(Debug)]
enum ManifestType {
    Keyframes(Vec<f64>),
    Interval(f64),
}

#[derive(Debug)]
pub struct M3U8Manifest {
    manifest_type: ManifestType,
    pub inner: String,
}

impl M3U8Manifest {
    const MANIFEST_HEADER: &'static str = r#"#EXTM3U
#EXT-X-PLAYLIST-TYPE:VOD
#EXT-X-VERSION:7
#EXT-X-MEDIA-SEQUENCE:0
"#;

    pub fn from_interval(segment_duration: f64, mut duration: f64, id: &str) -> Self {
        use std::fmt::Write;
        let mut manifest: String = Self::MANIFEST_HEADER.into();
        writeln!(
            &mut manifest,
            r#"#EXT-X-MAP:URI="/api/watch/hls/{id}/init""#
        )
        .unwrap();
        writeln!(
            &mut manifest,
            "#EXT-X-TARGETDURATION:{}",
            segment_duration.round() as u32
        )
        .unwrap();
        let mut i = 0;
        while duration > 0. {
            let time = if duration - segment_duration >= 0. {
                segment_duration
            } else {
                duration
            };
            writeln!(&mut manifest, "#EXTINF:{:.6},", time).unwrap();
            writeln!(&mut manifest, "/api/watch/hls/{id}/segment/{i}").unwrap();
            i += 1;
            duration -= segment_duration;
        }
        write!(&mut manifest, "#EXT-X-ENDLIST").unwrap();
        Self {
            inner: manifest,
            manifest_type: ManifestType::Interval(segment_duration),
        }
    }

    pub fn from_keyframes(mut frames: KeyFrames, id: &str, total_duration: Duration) -> Self {
        use std::fmt::Write;
        // max duration between keyframes
        let mut max_duration = 0.;
        let mut last_keyframe = 0.;
        let mut desired_cut_time = DEFAULT_SEGMENT_LENGTH as f64;
        let mut parts = String::new();
        let mut i = 0;
        let mut durations = Vec::new();
        frames.key_frames.retain(|&keyframe_time| {
            if keyframe_time >= desired_cut_time {
                let duration = keyframe_time - last_keyframe;
                durations.push(duration);
                writeln!(&mut parts, "#EXTINF:{:.6},", duration).unwrap();
                writeln!(
                    &mut parts,
                    "/api/watch/hls/{id}/segment/{i}?key_frame={}",
                    keyframe_time
                )
                .unwrap();
                if duration > max_duration {
                    max_duration = duration;
                }
                last_keyframe = keyframe_time;
                desired_cut_time += DEFAULT_SEGMENT_LENGTH as f64;
                i += 1;
                true
            } else {
                false
            }
        });

        let last_keyframe_duration = total_duration.as_secs_f64() - last_keyframe;
        durations.push(last_keyframe_duration);
        writeln!(&mut parts, "#EXTINF:{:.6},", last_keyframe_duration).unwrap();
        writeln!(
            &mut parts,
            "/api/watch/hls/{id}/segment/{i}?key_frame={}",
            total_duration.as_secs_f64() - last_keyframe_duration,
        )
        .unwrap();

        let mut manifest: String = Self::MANIFEST_HEADER.into();
        writeln!(
            &mut manifest,
            r#"#EXT-X-MAP:URI="/api/watch/hls/{id}/init""#
        )
        .unwrap();
        writeln!(
            &mut manifest,
            "#EXT-X-TARGETDURATION:{}",
            max_duration.round() as u32
        )
        .unwrap();
        write!(&mut manifest, "{}", parts).unwrap();
        write!(&mut manifest, "#EXT-X-ENDLIST").unwrap();

        Self {
            inner: manifest,
            manifest_type: ManifestType::Keyframes(durations),
        }
    }

    pub fn seek_time(&self, segment_idx: usize) -> f64 {
        match &self.manifest_type {
            ManifestType::Keyframes(durations) => {
                durations[..segment_idx].iter().sum::<f64>() + 0.001
            }
            ManifestType::Interval(i) => i * segment_idx as f64,
        }
    }
}

impl AsRef<str> for M3U8Manifest {
    fn as_ref(&self) -> &str {
        self.inner.as_str()
    }
}
