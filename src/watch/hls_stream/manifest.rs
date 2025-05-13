use super::keyframe::KeyFrames;

#[derive(Debug)]
enum ManifestType {
    Keyframes(KeyFrames),
    Interval(f64),
}

#[derive(Debug)]
pub struct M3U8Manifest {
    manifest_type: ManifestType,
    pub inner: String,
}

impl M3U8Manifest {
    const MANIFEST_HEADER: &'static str = r#"#EXTM3U
#EXT-X-VERSION:7
#EXT-X-MEDIA-SEQUENCE:0
#EXT-X-ALLOW-CACHE:NO
#EXT-X-PLAYLIST-TYPE:VOD
"#;

    pub fn from_interval(segment_duration: f64, mut duration: f64, id: &str) -> Self {
        use std::fmt::Write;
        let mut manifest: String = Self::MANIFEST_HEADER.into();
        writeln!(&mut manifest, r#"#EXT-X-MAP:URI="/api/watch/hls/{id}/init""#).unwrap();
        writeln!(
            &mut manifest,
            "#EXT-X-TARGETDURATION:{:.6}",
            segment_duration
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
        writeln!(&mut manifest, "#EXT-X-ENDLIST").unwrap();
        Self {
            inner: manifest,
            manifest_type: ManifestType::Interval(segment_duration),
        }
    }

    pub fn from_keyframes(frames: KeyFrames, id: &str) -> Self {
        use std::fmt::Write;
        // max duration between 2 keyframes
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
            writeln!(&mut parts, "/api/watch/hls/{id}/segment/{i}").unwrap();
        }

        let mut manifest: String = Self::MANIFEST_HEADER.into();
        writeln!(&mut manifest, r#"#EXT-X-MAP:URI="/api/watch/hls/{id}/init""#).unwrap();
        writeln!(&mut manifest, "#EXT-X-TARGETDURATION:{:.6}", max_duration).unwrap();
        writeln!(&mut manifest, "{}", parts).unwrap();
        writeln!(&mut manifest, "#EXT-X-ENDLIST").unwrap();
        Self {
            inner: manifest,
            manifest_type: ManifestType::Keyframes(frames),
        }
    }

    pub fn seek_time(&self, segment_idx: usize) -> f64 {
        match &self.manifest_type {
            ManifestType::Keyframes(frames) => frames.key_frames[segment_idx].time,
            ManifestType::Interval(i) => i * segment_idx as f64,
        }
    }
}

impl AsRef<str> for M3U8Manifest {
    fn as_ref(&self) -> &str {
        self.inner.as_str()
    }
}
