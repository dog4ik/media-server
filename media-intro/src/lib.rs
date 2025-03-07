use std::{cmp::Reverse, fmt::Display, time::Duration};

mod config;
mod gaussian;
mod gradient;

#[derive(Debug)]
pub enum MatchError {
    FingerprintTooLong { index: u8 },
}

impl Display for MatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchError::FingerprintTooLong { index } => {
                write!(f, "fingerprint #{index} is too long")
            }
        }
    }
}

impl std::error::Error for MatchError {}

const ALIGN_BITS: u32 = 12;
const HASH_SHIFT: u32 = 32 - ALIGN_BITS;
const HASH_MASK: u32 = ((1 << ALIGN_BITS) - 1) << HASH_SHIFT;
const OFFSET_MASK: u32 = (1 << (32 - ALIGN_BITS - 1)) - 1;
const SOURCE_MASK: u32 = 1 << (32 - ALIGN_BITS - 1);

const fn align_strip(x: u32) -> u32 {
    x >> (32 - ALIGN_BITS)
}

/// Returns similar segments of two audio streams using their fingerprints.
pub fn match_fingerprints(fp1: &[u32], fp2: &[u32]) -> Result<Vec<Segment>, MatchError> {
    if fp1.len() + 1 >= OFFSET_MASK as usize {
        return Err(MatchError::FingerprintTooLong { index: 0 });
    }

    if fp2.len() + 1 >= OFFSET_MASK as usize {
        return Err(MatchError::FingerprintTooLong { index: 1 });
    }

    let mut offsets = Vec::with_capacity(fp1.len() + fp2.len());
    for (i, &segment) in fp1.iter().enumerate() {
        offsets.push((align_strip(segment) << HASH_SHIFT) | (i as u32));
    }

    for (i, &segment) in fp2.iter().enumerate() {
        offsets.push((align_strip(segment) << HASH_SHIFT) | (i as u32) | SOURCE_MASK);
    }

    offsets.sort_unstable();

    let mut histogram = vec![0u32; fp1.len() + fp2.len()];
    for (offset_idx, item1) in offsets.iter().enumerate() {
        let hash1 = item1 & HASH_MASK;
        let offset1 = item1 & OFFSET_MASK;
        let source1 = item1 & SOURCE_MASK;
        if source1 != 0 {
            // if we got hash from fp2, it means there is no hash from fp1,
            // because if there was, it would be first
            continue;
        }

        for item2 in &offsets[offset_idx..] {
            let hash2 = item2 & HASH_MASK;
            if hash1 != hash2 {
                break;
            }

            let offset2 = item2 & OFFSET_MASK;
            let source2 = item2 & SOURCE_MASK;
            if source2 != 0 {
                let offset_diff = offset1 as usize + fp2.len() - offset2 as usize;
                histogram[offset_diff] += 1;
            }
        }
    }

    let mut best_alignments = Vec::new();
    let histogram_size = histogram.len();
    for i in 0..histogram_size {
        let count = histogram[i];
        if histogram[i] > 1 {
            let is_peak_left = if i > 0 {
                histogram[i - 1] <= count
            } else {
                true
            };
            let is_peak_right = if i < histogram_size - 1 {
                histogram[i + 1] <= count
            } else {
                true
            };
            if is_peak_left && is_peak_right {
                best_alignments.push((count, i));
            }
        }
    }

    best_alignments.sort_unstable_by_key(|it| Reverse(*it));

    let mut segments: Vec<Segment> = Vec::new();
    if let Some((_count, offset)) = best_alignments.into_iter().next() {
        let offset_diff = offset as isize - fp2.len() as isize;
        let offset1 = if offset_diff > 0 {
            offset_diff as usize
        } else {
            0
        };
        let offset2 = if offset_diff < 0 {
            -offset_diff as usize
        } else {
            0
        };

        let size = usize::min(fp1.len() - offset1, fp2.len() - offset2);
        let mut bit_counts = Vec::with_capacity(size);
        for i in 0..size {
            bit_counts.push((fp1[offset1 + i] ^ fp2[offset2 + i]).count_ones() as f64);
        }

        let orig_bit_counts = bit_counts.clone();
        let mut smoothed_bit_counts = vec![0.0; size];
        gaussian::gaussian_filter(&mut bit_counts, &mut smoothed_bit_counts, 8.0, 3);

        let mut grad = Vec::with_capacity(size);
        gradient::gradient(smoothed_bit_counts.iter().copied(), &mut grad);

        for item in &mut grad[..size] {
            *item = item.abs();
        }

        let mut gradient_peaks = Vec::new();
        for i in 0..size {
            let gi = grad[i];
            if i > 0
                && i < size - 1
                && gi > 0.15
                && gi >= grad[i - 1]
                && gi >= grad[i + 1]
                && (gradient_peaks.is_empty() || gradient_peaks.last().unwrap() + 1 < i)
            {
                gradient_peaks.push(i);
            }
        }
        gradient_peaks.push(size);

        let match_threshold = 10.0;
        let max_score_difference = 0.7;

        let mut begin = 0;
        for end in gradient_peaks {
            let duration = end - begin;
            let score: f64 = orig_bit_counts[begin..end].iter().sum::<f64>() / (duration as f64);
            if score < match_threshold {
                let new_segment = Segment {
                    offset1: offset1 + begin,
                    offset2: offset2 + begin,
                    items_count: duration,
                    score,
                };

                let mut added = false;
                if let Some(s1) = segments.last_mut() {
                    if (s1.score - score).abs() < max_score_difference {
                        if let Some(merged) = s1.try_merge(&new_segment) {
                            *s1 = merged;
                            added = true;
                        }
                    }
                }

                if !added {
                    segments.push(new_segment);
                }
            }
            begin = end;
        }
    }

    Ok(segments)
}

/// Segment of an audio that is similar between two fingerprints.
#[derive(Debug, Clone)]
pub struct Segment {
    /// Index of the item in the first fingerprint.
    pub offset1: usize,

    /// Index of an item in the second fingerprint.
    pub offset2: usize,

    /// Number of items from the fingerprint corresponding to this segment.
    pub items_count: usize,

    /// Score that corresponds to similarity of this segment.
    /// The smaller this value is, the stronger similarity.
    ///
    /// This value can be be 0 up to 32.
    pub score: f64,
}

impl Segment {
    /// A timestamp representing the start of the segment in the first fingerprint.
    pub fn start1(&self) -> Duration {
        config::CHROMA_CONFIG.item_duration_in_seconds() * self.offset1 as u32
    }

    /// A timestamp representing the end of the segment in the first fingerprint.
    pub fn end1(&self) -> Duration {
        self.start1() + self.duration()
    }

    /// A timestamp representing the start of the segment in the second fingerprint.
    pub fn start2(&self) -> Duration {
        config::CHROMA_CONFIG.item_duration_in_seconds() * self.offset2 as u32
    }

    /// A timestamp representing the end of the segment in the second fingerprint.
    pub fn end2(&self) -> Duration {
        self.start2() + self.duration()
    }

    /// Duration of the segment (in seconds).
    pub fn duration(&self) -> Duration {
        config::CHROMA_CONFIG.item_duration_in_seconds() * self.items_count as u32
    }
}

impl Segment {
    /// Try to merge two consecutive segments into one.
    fn try_merge(&self, other: &Self) -> Option<Self> {
        // Check if segments are consecutive
        if self.offset1 + self.items_count != other.offset1 {
            return None;
        }

        if self.offset2 + self.items_count != other.offset2 {
            return None;
        }

        let new_duration = self.items_count + other.items_count;
        let new_score = (self.score * self.items_count as f64
            + other.score * other.items_count as f64)
            / new_duration as f64;
        Some(Segment {
            offset1: self.offset1,
            offset2: self.offset2,
            items_count: new_duration,
            score: new_score,
        })
    }
}
