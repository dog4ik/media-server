use std::{ops::RangeBounds, path::Path, time::Duration};

use crate::{
    config::{self},
    ffmpeg,
};

const TAKE_TIME: Duration = Duration::from_secs(5 * 60);

/// each byte is ~ 10 ms
/// Higher value means less precision. Represents bytes
const WINDOW_SIZE: usize = 15;
/// How many byte errors are allowed in window
const ALLOWED_WINDOW_ERRORS: usize = 3;
/// Higher value means less precision. Represents percent
const ACCEPT_ERROR_RATE: usize = 15;
/// Higher value means less precision
/// One skip is `WINDOW_SIZE * 10` ms of audio
const ALLOWED_SKIPS: usize = 3;

/// Canculate duration of the single byte in fingerprint
const fn byte_duration(fingerprint_len: usize) -> Duration {
    Duration::from_millis(TAKE_TIME.as_millis() as u64 / fingerprint_len as u64)
}

fn chunk_duration(total_bytes: usize, chunk_len: usize) -> Duration {
    let byte_duration = byte_duration(total_bytes);
    byte_duration * chunk_len as u32
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

    pub fn len(&self) -> usize {
        self.data.len()
    }
}

/// 5 minutes audio fingerprint is 9608 bytes
#[derive(Debug)]
struct Chromaprint {
    fingerprint: Vec<u8>,
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
    pub fn fit_chunk(&self, chunk: &[u8]) -> Option<usize> {
        let mut min_errors_amount = u32::MAX;
        let mut start_idx = None;
        let window_size = chunk.len();
        assert!(window_size < self.len());
        for (window_start, window) in self.fingerprint.windows(window_size).enumerate() {
            if let Some(errors) = check_window_errors(window, chunk) {
                if errors < min_errors_amount {
                    start_idx = Some(window_start);
                    min_errors_amount = errors;
                }
            }
        }
        start_idx
    }

    /// Iterator over chunks of fingerprint
    pub fn chunks(&self) -> impl Iterator<Item = &[u8; WINDOW_SIZE]> + '_ {
        self.fingerprint.array_chunks()
    }

    /// Walks intro side by side. Returns the stop byte offset
    fn walk_intro<'a>(
        &'a self,
        byte_offset: usize,
        other_fp: &Chromaprint,
        other_byte_offset: usize,
    ) -> usize {
        let mut end = byte_offset;
        for (self_chunk, other_chunk) in self.fingerprint[byte_offset..]
            .array_chunks::<WINDOW_SIZE>()
            .zip(other_fp.fingerprint[other_byte_offset..].array_chunks::<WINDOW_SIZE>())
        {
            if check_window_errors(self_chunk, other_chunk).is_some() {
                end += WINDOW_SIZE;
            }
        }
        end
    }

    /// Get longest intersection with other fingerprint
    fn intersection_of(&self, other_fp: &Chromaprint, min_duration: Duration) -> Option<Intro<'_>> {
        let longest_intro = None;
        for (i, other_start) in self
            .chunks()
            .enumerate()
            .filter_map(|(i, c)| Some((i, other_fp.fit_chunk(c)?)))
        {
            let start = i * WINDOW_SIZE;
            // walk side by side until we find the end
            // we can skip current chunk because fit_chunk guarantees that chunk is similar
            let end = self.walk_intro(start, other_fp, other_start);
        }
        longest_intro
    }
}

#[derive(Debug)]
struct EpisodesIntersections<'a> {
    chromaprint: &'a Chromaprint,
    intersections: Vec<Option<Intro<'a>>>,
}

impl<'a> EpisodesIntersections<'a> {
    pub fn new(chromaprint: &'a Chromaprint) -> Self {
        Self {
            chromaprint,
            intersections: Vec::new(),
        }
    }

    pub fn add(&mut self, intersection: Option<Intro<'a>>) {
        self.intersections.push(intersection)
    }
}

/// early return with None if encountered critical amount of errors
/// Returns the amount of errors if not in clitical amount
fn check_window_errors(left: &[u8], right: &[u8]) -> Option<u32> {
    assert_eq!(left.len(), right.len());
    let mut errors = 0;
    for (left, right) in left.iter().zip(right) {
        let diff: u8 = left ^ right;
        if diff.count_ones() > 4 {
            errors += 1;
        }
        if errors > ALLOWED_WINDOW_ERRORS as u32 {
            return None;
        }
    }
    Some(errors)
}

fn detect_intros(
    fingerprints: Vec<Chromaprint>,
    min_duration: Duration,
) -> Vec<Option<IntroRange>> {
    if fingerprints.len() < 2 {
        tracing::error!("Need at least 2 fingerprints to detect common segment");
        return vec![None; fingerprints.len()];
    }

    let fingerprint_length = fingerprints[0].len();
    let mut ranges = vec![None; fingerprints.len()];
    let mut episodes = Vec::new();

    // idea is that we can finish fingerprinting in once loop cycle.
    for (i, current_fp) in fingerprints.iter().enumerate() {
        let mut intersections = EpisodesIntersections::new(current_fp);
        for (j, fp) in fingerprints.iter().enumerate() {
            // we don't analyze self with self
            if j == i {
                continue;
            }
            let intersection = current_fp.intersection_of(fp, min_duration);
            intersections.add(intersection);
        }
        episodes.push(intersections);
    }

    ranges
}

pub async fn intro_detection(
    episodes: Vec<impl AsRef<Path>>,
) -> anyhow::Result<Vec<Option<IntroRange>>> {
    let mut fingerprints = Vec::with_capacity(episodes.len());
    let min_duration: config::IntroMinDuration = config::CONFIG.get_value();
    let min_duration = Duration::from_secs(min_duration.0 as u64);
    tracing::debug!("Minimum intro duration: {:?}", min_duration);

    let mut jobs = Vec::with_capacity(episodes.len());
    for path in episodes.iter() {
        jobs.push(ffmpeg::spawn_chromaprint_command(path, TAKE_TIME));
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
                include_bytes!("../../tests_data/fp/friends.s10/1.chromaprint"),
                IntroRange::new(Duration::from_secs(130)..Duration::from_secs(174)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/2.chromaprint"),
                IntroRange::new(Duration::from_secs(89)..Duration::from_secs(125)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/3.chromaprint"),
                IntroRange::new(Duration::from_secs(76)..Duration::from_secs(120)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/4.chromaprint"),
                IntroRange::new(Duration::from_secs(70)..Duration::from_secs(104)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/5.chromaprint"),
                IntroRange::new(Duration::from_secs(125)..Duration::from_secs(169)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/6.chromaprint"),
                IntroRange::new(Duration::from_secs(108)..Duration::from_secs(144)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/7.chromaprint"),
                IntroRange::new(Duration::from_secs(76)..Duration::from_secs(110)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/8.chromaprint"),
                IntroRange::new(Duration::from_secs(99)..Duration::from_secs(144)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/9.chromaprint"),
                IntroRange::new(Duration::from_secs(80)..Duration::from_secs(115)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/10.chromaprint"),
                IntroRange::new(Duration::from_secs(78)..Duration::from_secs(112)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/11.chromaprint"),
                IntroRange::new(Duration::from_secs(75)..Duration::from_secs(120)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/12.chromaprint"),
                IntroRange::new(Duration::from_secs(79)..Duration::from_secs(124)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/13.chromaprint"),
                IntroRange::new(Duration::from_secs(63)..Duration::from_secs(98)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/14.chromaprint"),
                IntroRange::new(Duration::from_secs(91)..Duration::from_secs(136)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/15.chromaprint"),
                IntroRange::new(Duration::from_secs(164)..Duration::from_secs(198)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/16.chromaprint"),
                IntroRange::new(Duration::from_secs(95)..Duration::from_secs(139)),
            ),
            (
                include_bytes!("../../tests_data/fp/friends.s10/17.chromaprint"),
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

    #[test_log::test]
    fn test_edgerunners() {
        // First episode does not have intro
        // these intros have common netflix logo at the start.
        // also they have common silence at the end
        let expected_intros = [
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/1.chromaprint"),
                // No into in first episode
                IntroRange::new(Duration::from_secs(0)..Duration::from_secs(0)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/2.chromaprint"),
                IntroRange::new(Duration::from_secs(76)..Duration::from_secs(165)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/3.chromaprint"),
                IntroRange::new(Duration::from_secs(10)..Duration::from_secs(101)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/4.chromaprint"),
                IntroRange::new(Duration::from_secs(31)..Duration::from_secs(120)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/5.chromaprint"),
                IntroRange::new(Duration::from_secs(10)..Duration::from_secs(101)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/6.chromaprint"),
                IntroRange::new(Duration::from_secs(10)..Duration::from_secs(101)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/7.chromaprint"),
                IntroRange::new(Duration::from_secs(86)..Duration::from_secs(172)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/8.chromaprint"),
                IntroRange::new(Duration::from_secs(121)..Duration::from_secs(207)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/9.chromaprint"),
                IntroRange::new(Duration::from_secs(11)..Duration::from_secs(101)),
            ),
            (
                include_bytes!("../../tests_data/fp/edgerunners.s01/10.chromaprint"),
                IntroRange::new(Duration::from_secs(10)..Duration::from_secs(101)),
            ),
        ];

        let fingerprints = expected_intros
            .into_iter()
            .map(|f| Chromaprint::new(f.0.to_vec()))
            .collect();
        let intros = detect_intros(fingerprints, TEST_MIN_INTRO_DURATION);
        for (i, (range, (_, expected_range))) in intros.into_iter().zip(expected_intros).enumerate()
        {
            if i == 0 {
                continue;
            }
            let range = range.unwrap();
            test_range(range, expected_range, Duration::from_secs(1), i + 1);
        }
    }
}
