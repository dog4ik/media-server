use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct Configuration {
    frame_size: usize,
    frame_overlap: usize,
}

const DEFAULT_SAMPLE_RATE: u32 = 11025;
const DEFAULT_FRAME_SIZE: usize = 4096;
const DEFAULT_FRAME_OVERLAP: usize = DEFAULT_FRAME_SIZE - DEFAULT_FRAME_SIZE / 12;

pub const CHROMA_CONFIG: Configuration = Configuration::new();

impl Configuration {
    /// Creates a new default configuration.
    const fn new() -> Self {
        Self {
            frame_size: DEFAULT_FRAME_SIZE,
            frame_overlap: DEFAULT_FRAME_OVERLAP,
        }
    }

    /// Target sample rate for fingerprint calculation.
    pub fn sample_rate(&self) -> u32 {
        DEFAULT_SAMPLE_RATE
    }

    fn samples_in_item(&self) -> usize {
        self.frame_size - self.frame_overlap
    }

    /// A duration of a single item from the fingerprint.
    pub fn item_duration_in_seconds(&self) -> Duration {
        Duration::from_secs_f32(self.samples_in_item() as f32 / self.sample_rate() as f32)
    }
}

impl Default for Configuration {
    fn default() -> Self {
        Self::new()
    }
}
