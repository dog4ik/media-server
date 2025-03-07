use std::{collections::HashMap, ops::RangeBounds, path::Path, time::Duration};

use media_intro::Segment;

use crate::{
    config::{self},
    ffmpeg,
    progress::TaskTrait,
};

#[derive(Debug, Clone, Copy, serde::Serialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct IntroJob {
    pub show_id: i64,
    pub season: usize,
}

impl TaskTrait for IntroJob {
    type Identifier = Self;

    type Progress = ();

    fn identifier(&self) -> Self::Identifier {
        *self
    }

    fn into_progress(chunk: crate::progress::ProgressChunk<Self>) -> crate::progress::TaskProgress
    where
        Self: Sized,
    {
        crate::progress::TaskProgress::IntroDetection(chunk)
    }
}

const TAKE_TIME: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, Default)]
pub struct IntroPair(IntroRange, IntroRange);

impl IntroPair {
    pub fn from_segments(segments: &[Segment], min_duration: Duration) -> Option<Self> {
        let mut shortest_segment: Option<&Segment> = None;
        let mut prev: Option<Segment> = None;
        for seg in segments {
            if let Some(prev) = prev {
                if prev.offset1 + prev.items_count == seg.offset1
                    && prev.offset2 + prev.items_count == seg.offset2
                {
                    println!("both offsets are contingious")
                }
            }
            prev = Some(seg.clone());
            let duration = seg.duration();
            if duration < min_duration {
                continue;
            }
            match shortest_segment {
                Some(shortest) if shortest.duration() > duration => {
                    shortest_segment = Some(seg);
                }
                Some(_) => {}
                None => shortest_segment = Some(seg),
            };
        }
        shortest_segment.map(|s| {
            IntroPair(
                IntroRange {
                    start: s.start1(),
                    end: s.end1(),
                },
                IntroRange {
                    start: s.start2(),
                    end: s.end2(),
                },
            )
        })
    }

    pub fn from_segments_merged(segments: &[Segment], min_duration: Duration) -> Option<Self> {
        let mut merged_segments = Vec::new();
        let mut current_segment: Option<Segment> = None;

        for seg in segments {
            if let Some(prev) = &mut current_segment {
                if prev.offset1 + prev.items_count == seg.offset1
                    && prev.offset2 + prev.items_count == seg.offset2
                {
                    prev.items_count += seg.items_count;
                    continue;
                } else {
                    merged_segments.push(prev.clone());
                }
            }
            current_segment = Some(seg.clone());
        }

        if let Some(seg) = current_segment {
            merged_segments.push(seg);
        }

        let mut shortest_segment: Option<&Segment> = None;
        for seg in &merged_segments {
            let duration = seg.duration();
            if duration < min_duration {
                continue;
            }
            match shortest_segment {
                Some(shortest) if shortest.duration() > duration => {
                    shortest_segment = Some(seg);
                }
                Some(_) => {}
                None => shortest_segment = Some(seg),
            };
        }

        shortest_segment.map(|s| {
            IntroPair(
                IntroRange {
                    start: s.start1(),
                    end: s.end1(),
                },
                IntroRange {
                    start: s.start2(),
                    end: s.end2(),
                },
            )
        })
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IntroRange {
    pub start: Duration,
    pub end: Duration,
}

impl From<&Segment> for IntroRange {
    fn from(seg: &Segment) -> Self {
        IntroRange {
            start: seg.start1(),
            end: seg.end1(),
        }
    }
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

    pub fn from_segments(segments: &[Segment], min_duration: Duration) -> Option<Self> {
        let mut best_section: Option<IntroRange> = None;
        for seg in segments {
            let duration = seg.duration();
            if duration < min_duration {
                continue;
            }
            match best_section {
                Some(best) if best.end - best.start > duration => {
                    best_section = Some(IntroRange::from(seg))
                }
                Some(_) => {}
                None => best_section = Some(IntroRange::from(seg)),
            };
        }
        best_section
    }

    pub fn into_db_intro(self, video_id: i64) -> crate::db::DbEpisodeIntro {
        crate::db::DbEpisodeIntro {
            id: None,
            video_id,
            start_sec: self.start.as_secs() as i64,
            end_sec: self.end.as_secs() as i64,
        }
    }
}

#[derive(Debug)]
struct Chromaprint {
    fingerprint: Vec<u32>,
}

impl Chromaprint {
    pub fn new(fingerprint: Vec<u8>) -> Self {
        assert!(
            fingerprint.len() % 4 == 0,
            "vector length must be a multiple of 4"
        );

        let fingerprint = fingerprint
            .windows(4)
            .map(|w| w.try_into().expect("window size is 4"))
            .map(u32::from_be_bytes)
            .collect();
        Self { fingerprint }
    }
}

#[derive(Debug)]
struct EpisodesIntersections {
    intersections: Vec<Option<IntroRange>>,
}

impl<'a> EpisodesIntersections {
    pub fn new() -> Self {
        Self {
            intersections: Vec::new(),
        }
    }

    pub fn add(&mut self, intersection: Option<IntroRange>) {
        self.intersections.push(intersection)
    }

    pub fn finalize(self) -> Option<IntroRange> {
        let mut iter = self.intersections.into_iter().flatten();
        let mut current_intro = iter.next()?;
        for intro in iter {
            if intro.start > current_intro.start {
                current_intro.start = intro.start;
            }
            if intro.end > current_intro.end {
                current_intro.end = intro.end;
            }
        }
        Some(current_intro)
    }
}

fn detect_intros(
    fingerprints: Vec<Chromaprint>,
    min_duration: Duration,
) -> Vec<Option<IntroRange>> {
    if fingerprints.len() < 2 {
        tracing::error!("Need at least 2 fingerprints to detect common segment");
        return vec![None; fingerprints.len()];
    }

    let mut ranges = vec![None; fingerprints.len()];
    let mut intro_cache = HashMap::new();

    // Optimization: if intro = ranges[compared intro] then we can reuse compared intro calculations instead of calculating them again
    for (i, current_fp) in fingerprints.iter().enumerate() {
        let mut intersections = EpisodesIntersections::new();
        for (j, fp) in fingerprints.iter().enumerate() {
            // don't analyze self
            if j == i {
                intersections.add(None);
                continue;
            }
            let (ni, nj) = (i.min(j), i.max(j));
            let range = intro_cache.entry((ni, nj)).or_insert_with(|| {
                let intersection =
                    media_intro::match_fingerprints(&current_fp.fingerprint, &fp.fingerprint)
                        .unwrap();
                IntroPair::from_segments_merged(&intersection, min_duration)
            });
            if i == ni {
                intersections.add(range.map(|v| v.0));
            } else {
                intersections.add(range.map(|v| v.1));
            }
        }
        ranges[i] = intersections.finalize();
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
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::error!(
                error = %stderr,
                "Fingerprint collector failed, make sure ffmpeg supports chromaprint"
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
    const ALLOWED_THRESHOLD: Duration = Duration::from_secs(5);

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
    fn test_friends_intro_detection() {
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
            test_range(range, expected_range, ALLOWED_THRESHOLD, i + 1);
        }
    }

    #[test]
    fn test_dexter_original_sin_intro_detection() {
        let duration = 88;
        let expected_intros = [
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/1.chromaprint"),
                IntroRange::new(Duration::from_secs(170)..Duration::from_secs(170 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/2.chromaprint"),
                IntroRange::new(Duration::from_secs(205)..Duration::from_secs(205 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/3.chromaprint"),
                IntroRange::new(Duration::from_secs(87)..Duration::from_secs(87 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/4.chromaprint"),
                IntroRange::new(Duration::from_secs(74)..Duration::from_secs(74 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/5.chromaprint"),
                IntroRange::new(Duration::from_secs(60)..Duration::from_secs(60 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/6.chromaprint"),
                IntroRange::new(Duration::from_secs(107)..Duration::from_secs(107 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/7.chromaprint"),
                IntroRange::new(Duration::from_secs(141)..Duration::from_secs(141 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/8.chromaprint"),
                IntroRange::new(Duration::from_secs(80)..Duration::from_secs(80 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/9.chromaprint"),
                IntroRange::new(Duration::from_secs(111)..Duration::from_secs(111 + duration)),
            ),
            (
                include_bytes!("../../tests_data/fp/dexter.original.sin.s01/10.chromaprint"),
                IntroRange::new(Duration::from_secs(172)..Duration::from_secs(172 + duration)),
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
            test_range(range, expected_range, ALLOWED_THRESHOLD, i + 1);
        }
    }

    #[test_log::test]
    fn test_edgerunners_intro_detection() {
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
            test_range(range, expected_range, ALLOWED_THRESHOLD, i + 1);
        }
    }
}
