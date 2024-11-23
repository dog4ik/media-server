use std::{ops::RangeBounds, path::Path, process::Stdio, time::Duration};

use tokio::process::{Child, Command};

use crate::config::{self};

const TAKE_TIME: Duration = Duration::from_secs(300);

// Higher value means less precision. Represents bytes
const WINDOW_SIZE: usize = 15;
// Higher value means less precision. Represents percent
const ACCEPT_ERROR_RATE: usize = 15;
// Higher value means less precision
const ALLOWED_SKIPS: usize = 3;

/// Canculate duration of the single byte in fingerprint
const fn byte_duration(fingerprint_len: usize) -> Duration {
    Duration::from_millis(TAKE_TIME.as_millis() as u64 / fingerprint_len as u64)
}

fn chunk_duration(total_bytes: usize, chunk_len: usize) -> Duration {
    let byte_duration = byte_duration(total_bytes);
    byte_duration * chunk_len as u32
}

#[derive(Debug, Clone, Copy)]
struct Position {
    errors_amount: usize,
    start_byte: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct Intro<'a> {
    start: usize,
    data: &'a [u8],
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IntroRange {
    pub start: Duration,
    pub end: Duration,
}

impl IntroRange {
    pub fn new(range: impl RangeBounds<Duration>) -> Self {
        let start = match range.start_bound() {
            std::ops::Bound::Included(d) => *d,
            std::ops::Bound::Excluded(d) => *d,
            std::ops::Bound::Unbounded => panic!("intro range must be bounded"),
        };
        let end = match range.end_bound() {
            std::ops::Bound::Included(d) => *d,
            std::ops::Bound::Excluded(d) => *d,
            std::ops::Bound::Unbounded => panic!("intro range must be bounded"),
        };
        Self { start, end }
    }
}

impl Intro<'_> {
    pub fn range(&self, total_bytes: usize) -> IntroRange {
        let total_bytes = byte_duration(total_bytes);
        let range =
            total_bytes * self.start as u32..=(self.start + self.data.len()) as u32 * total_bytes;
        IntroRange {
            start: *range.start(),
            end: *range.end(),
        }
    }
}

#[derive(Debug)]
struct Chromaprint {
    fingerprint: Vec<u8>,
}

fn spawn_chromaprint_command(path: impl AsRef<Path>) -> std::io::Result<Child> {
    let path = path.as_ref().to_path_buf();
    let str_path = path.to_string_lossy();
    let ffmpeg: config::IntroDetectionFfmpegBuild = config::CONFIG.get_value();
    Command::new(ffmpeg.0)
        .args([
            "-hide_banner",
            "-i",
            &str_path,
            "-to",
            &format_ffmpeg_time(&TAKE_TIME),
            "-ac",
            "2",
            "-map",
            "0:a:0",
            "-f",
            "chromaprint",
            "-fp_format",
            "raw",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
}

impl Chromaprint {
    pub fn new(fingerprint: Vec<u8>) -> Self {
        Self { fingerprint }
    }

    /// Length of fingerprint in bytes
    pub fn len(&self) -> usize {
        self.fingerprint.len()
    }

    /// Get best position for the given chunk.
    pub fn fit_chunk(&self, chunk: &[u8]) -> Position {
        let mut min_errors_amount = usize::MAX;
        let mut start_idx = 0;
        let window_size = chunk.len();
        assert!(window_size < self.len());
        for (window_start, window) in self.fingerprint.windows(window_size).enumerate() {
            let errors = count_bit_error_rates(window, chunk);
            if errors < min_errors_amount {
                min_errors_amount = errors;
                start_idx = window_start;
            }
        }

        Position {
            errors_amount: min_errors_amount,
            start_byte: start_idx,
        }
    }

    pub fn find_intro(&self, intro: &Intro) -> Option<Intro> {
        let window_size = intro.data.len();
        let allowed_errors = window_size * 8 / 100 * ACCEPT_ERROR_RATE;
        for (window_start, window) in self.fingerprint.windows(window_size).enumerate() {
            if check_bit_errors(allowed_errors -10, window, intro.data) {
                return Some(Intro {
                    start: window_start,
                    data: &self.fingerprint[window_start..window_start + window_size],
                });
            }
        }
        None
    }

    /// Iterator over chunks of fingerprint
    pub fn chunks(&self) -> impl Iterator<Item = &[u8; WINDOW_SIZE]> + '_ {
        self.fingerprint.array_chunks::<WINDOW_SIZE>()
    }

    pub fn get_intersection_of<'a>(
        &self,
        other: &'a Chromaprint,
        min_duration: Duration,
    ) -> Option<Intro<'a>> {
        let mut start_position: Option<usize> = None;
        let mut end_position: Option<usize> = None;
        let other_len = other.fingerprint.len();
        let allowed_errors = WINDOW_SIZE * 8 / 100 * ACCEPT_ERROR_RATE;
        let mut track_offset = 1;
        // Amount of chunks that are allowed to be different before we consider end of intro
        let mut skips_allowed = ALLOWED_SKIPS;
        for chunk in self.chunks() {
            // walk side by side using same chunk window after start position is found
            if let Some(start_pos) = start_position {
                let start = start_pos + (track_offset * WINDOW_SIZE);
                let end = start + WINDOW_SIZE;
                track_offset += 1;
                if end > other_len {
                    continue;
                }

                let other_chunk = &other.fingerprint[start..end];
                let errors = count_bit_error_rates(chunk, &other_chunk);
                if errors <= allowed_errors {
                    skips_allowed = ALLOWED_SKIPS;
                    end_position = Some(end);
                } else {
                    skips_allowed -= 1;
                }
                if skips_allowed == 0 {
                    if chunk_duration(self.len(), end_position.unwrap() - start_position.unwrap())
                        < min_duration
                    {
                        skips_allowed = ALLOWED_SKIPS;
                        track_offset = 1;
                        start_position = None;
                        end_position = None;
                        continue;
                    }
                    break;
                }

            // find the position that fits current chunk
            } else {
                let position = other.fit_chunk(chunk);
                if position.errors_amount <= allowed_errors {
                    start_position = Some(position.start_byte);
                    end_position = Some(position.start_byte);
                }
            }
        }
        start_position.zip(end_position).map(|(start, end)| Intro {
            start,
            data: &other.fingerprint[start..=end],
        })
    }
}

fn format_ffmpeg_time(duration: &Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let hours = minutes / 60;
    format!("{:0>2}:{:0>2}:{:0>2}", hours, minutes % 60, seconds % 60)
}

fn count_bit_error_rates(left: &[u8], right: &[u8]) -> usize {
    assert_eq!(left.len(), right.len());
    let mut errors = 0;
    for (left, right) in left.iter().zip(right) {
        let diff: u8 = left ^ right;
        errors += diff.count_ones();
    }
    errors as usize
}

fn check_bit_errors(max_allowed: usize, left: &[u8], right: &[u8]) -> bool {
    assert_eq!(left.len(), right.len());
    let mut errors = 0;
    for (left, right) in left.iter().zip(right) {
        let diff: u8 = left ^ right;
        errors += diff.count_ones();
        if errors > max_allowed as u32 {
            return false;
        }
    }
    true
}

fn detect_intros(
    fingerprints: Vec<Chromaprint>,
    min_duration: Duration,
) -> Vec<Option<IntroRange>> {
    let mut output = vec![None; fingerprints.len()];
    let Some(fingerprint_length) = fingerprints.first().map(|f| f.len()) else {
        return vec![None; fingerprints.len()];
    };

    for (i, fingerprint) in fingerprints.iter().enumerate() {
        if output[i].is_some() {
            continue;
        }
        let mut current_intro: Option<Intro> = None;
        for (remaining_fingerprint, remaining_fingerprint_intro) in
            fingerprints[i + 1..].iter().zip(output[i + 1..].iter_mut())
        {
            if remaining_fingerprint_intro.is_some() {
                continue;
            }

            if let Some(current_intro) = &current_intro {
                if let Some(intro) = remaining_fingerprint.find_intro(current_intro) {
                    *remaining_fingerprint_intro = Some(intro);
                }
            } else {
                if let Some(intersection) =
                    fingerprint.get_intersection_of(remaining_fingerprint, min_duration)
                {
                    *remaining_fingerprint_intro = Some(intersection);
                    current_intro = Some(intersection);
                }
            }
        }
        if let Some(current_intro) = current_intro {
            output[i] = Some(current_intro);
        }
    }

    output
        .iter()
        .map(|intro| intro.map(|p| p.range(fingerprint_length)))
        .collect()
}

pub async fn intro_detection(
    episodes: Vec<impl AsRef<Path>>,
) -> anyhow::Result<Vec<Option<IntroRange>>> {
    let mut fingerprints = Vec::with_capacity(episodes.len());
    let min_duration: config::IntroMinDuration = config::CONFIG.get_value();
    let min_duration = Duration::from_secs(min_duration.0 as u64);

    let mut jobs = Vec::with_capacity(episodes.len());
    for path in episodes.iter() {
        jobs.push(spawn_chromaprint_command(path));
    }
    for job in jobs {
        let output = job?.wait_with_output().await?;
        if output.status.success() {
            fingerprints.push(Chromaprint::new(output.stdout))
        } else {
            let stderr = String::from_utf8(output.stderr).unwrap();
            eprintln!("Fingerprint collector failed: {stderr}");
            tracing::warn!(
                error = stderr,
                "Fingerprint collector failed, make sure ffmpeg have chromaprint muxer setup"
            );
        }
    }
    tracing::debug!("Collected {} fingerprints", fingerprints.len());

    let positions = tokio::task::spawn_blocking(move || detect_intros(fingerprints, min_duration));
    let positions = positions.await?;
    Ok(positions)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::intro_detection::detect_intros;

    use super::{Chromaprint, IntroRange};

    const TEST_MIN_INTRO_DURATION: Duration = Duration::from_secs(20);

    fn test_range(range: IntroRange, expected: IntroRange, threshold: Duration, ep: usize) {
        assert!(
            range.start.abs_diff(expected.start) < threshold,
            "episode {ep} got wrong start range {:?}/{:?}, expected {:?}/{:?}",
            range.start,
            range.end,
            expected.start,
            expected.end,
        );
        assert!(
            range.end.abs_diff(expected.end) < threshold,
            "episode {ep} got end range {:?}, expected {:?}",
            range.end,
            expected.end,
        );
    }

    #[test]
    fn test_intros() {
        let expected_intros = [
            (
                include_bytes!("../tests_data/fp/friends.s10/1.chromaprint"),
                IntroRange::new(Duration::from_secs(130)..Duration::from_secs(174)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/2.chromaprint"),
                IntroRange::new(Duration::from_secs(89)..Duration::from_secs(125)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/3.chromaprint"),
                IntroRange::new(Duration::from_secs(76)..Duration::from_secs(120)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/4.chromaprint"),
                IntroRange::new(Duration::from_secs(70)..Duration::from_secs(104)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/5.chromaprint"),
                IntroRange::new(Duration::from_secs(125)..Duration::from_secs(169)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/6.chromaprint"),
                IntroRange::new(Duration::from_secs(108)..Duration::from_secs(144)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/7.chromaprint"),
                IntroRange::new(Duration::from_secs(76)..Duration::from_secs(110)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/8.chromaprint"),
                IntroRange::new(Duration::from_secs(99)..Duration::from_secs(144)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/9.chromaprint"),
                IntroRange::new(Duration::from_secs(80)..Duration::from_secs(115)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/10.chromaprint"),
                IntroRange::new(Duration::from_secs(78)..Duration::from_secs(112)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/11.chromaprint"),
                IntroRange::new(Duration::from_secs(75)..Duration::from_secs(120)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/12.chromaprint"),
                IntroRange::new(Duration::from_secs(79)..Duration::from_secs(124)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/13.chromaprint"),
                IntroRange::new(Duration::from_secs(63)..Duration::from_secs(98)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/14.chromaprint"),
                IntroRange::new(Duration::from_secs(91)..Duration::from_secs(136)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/15.chromaprint"),
                IntroRange::new(Duration::from_secs(164)..Duration::from_secs(198)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/16.chromaprint"),
                IntroRange::new(Duration::from_secs(95)..Duration::from_secs(139)),
            ),
            (
                include_bytes!("../tests_data/fp/friends.s10/17.chromaprint"),
                IntroRange::new(Duration::from_secs(80)..Duration::from_secs(125)),
            ),
        ];

        let fingerprints = expected_intros
            .into_iter()
            .map(|f| Chromaprint::new(f.0.to_vec()))
            .collect();
        let intros = detect_intros(fingerprints, TEST_MIN_INTRO_DURATION);
        for (i, (range, (_, expected_range))) in intros.into_iter().zip(expected_intros).enumerate()
        {
            let range = range.unwrap();
            test_range(range, expected_range, Duration::from_secs(1), i + 1);
        }
    }
}
