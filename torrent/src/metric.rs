use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use crate::download::peer::Performance;

/// Time windowed rolling speed meter.
#[derive(Debug)]
pub struct RollingSpeedMeter {
    history: VecDeque<(Instant, Performance)>,
}

impl RollingSpeedMeter {
    const WINDOW: Duration = Duration::from_secs(5);
    pub fn new() -> Self {
        let mut history = VecDeque::new();
        let now = Instant::now();
        history.push_back((now, Performance::default()));

        Self { history }
    }

    // Update with current total downloaded bytes and get the average speed over the window.
    // Call this periodically (e.g., every second).
    // Automatically prunes old entries.
    pub fn update(&mut self, now: Instant, current: Performance) -> (f64, f64) {
        // Prune history older than window
        while let Some(&(time, _)) = self.history.front() {
            if now.duration_since(time) > Self::WINDOW {
                self.history.pop_front();
            } else {
                break;
            }
        }

        // Add new entry (assuming current_bytes is cumulative and non-decreasing)
        if let Some(&(_, last)) = self.history.back() {
            debug_assert!(current.downloaded >= last.downloaded);
            debug_assert!(current.uploaded >= last.uploaded);
            self.history.push_back((now, current));
        } else {
            // Shouldn't happen, but reset if empty
            self.history.push_back((now, current));
        }

        // Compute speed: (latest_bytes - oldest_bytes) / window_duration
        if self.history.len() >= 2 {
            if let (Some(&(oldest_time, oldest_bytes)), Some(&(latest_time, latest_bytes))) =
                (self.history.front(), self.history.back())
            {
                let delta_time = latest_time.duration_since(oldest_time).as_secs_f64();
                let delta_uploaded = latest_bytes.uploaded.saturating_sub(oldest_bytes.uploaded);
                let delta_downloaded = latest_bytes
                    .downloaded
                    .saturating_sub(oldest_bytes.downloaded);

                if delta_time > 0.0 {
                    return (
                        delta_downloaded as f64 / delta_time,
                        delta_uploaded as f64 / delta_time,
                    );
                }
            }
        }

        (0., 0.)
    }

    // Get the current speed without updating the history.
    // Returns the average speed over the window based on existing data.
    pub fn speed(&self) -> (f64, f64) {
        // Compute speed: (latest_bytes - oldest_bytes) / window_duration
        if self.history.len() >= 2 {
            if let (Some(&(oldest_time, oldest_bytes)), Some(&(latest_time, latest_bytes))) =
                (self.history.front(), self.history.back())
            {
                let delta_time = latest_time.duration_since(oldest_time).as_secs_f64();
                let delta_downloaded = latest_bytes
                    .downloaded
                    .saturating_sub(oldest_bytes.downloaded);
                let delta_uploaded = latest_bytes.uploaded.saturating_sub(oldest_bytes.uploaded);

                if delta_time > 0.0 {
                    return (
                        delta_downloaded as f64 / delta_time,
                        delta_uploaded as f64 / delta_time,
                    );
                }
            }
        }

        (0., 0.)
    }

    pub fn total_downloaded(&self) -> Performance {
        self.history
            .back()
            .map_or_else(Performance::default, |&(_, performance)| performance)
    }
}

#[allow(unused)]
mod capacity_speed_meter {
    use std::{
        collections::VecDeque,
        time::{Duration, Instant},
    };

    #[derive(Debug)]
    pub struct CapacityRollingSpeedMeter {
        history: VecDeque<(Instant, u64)>,
    }

    impl CapacityRollingSpeedMeter {
        const CAPACITY: usize = 20;
        pub fn new() -> Self {
            let mut history = VecDeque::with_capacity(Self::CAPACITY);
            let now = Instant::now();
            history.push_back((now, 0));

            Self { history }
        }

        // Update with current total downloaded bytes and get the average speed over the last N ticks.
        pub fn update(&mut self, current_bytes: u64) -> f64 {
            let now = Instant::now();

            if let Some(&(_, last_bytes)) = self.history.back() {
                if current_bytes >= last_bytes {
                    self.history.push_back((now, current_bytes));
                }
            } else {
                self.history.push_back((now, current_bytes));
            }

            // Maintain capacity
            while self.history.len() > Self::CAPACITY {
                self.history.pop_front();
            }

            self.get_speed()
        }

        pub fn get_speed(&self) -> f64 {
            // Compute speed: (latest_bytes - oldest_bytes) / time_between_ticks
            if self.history.len() >= 2 {
                if let (Some(&(oldest_time, oldest_bytes)), Some(&(latest_time, latest_bytes))) =
                    (self.history.front(), self.history.back())
                {
                    let delta_time = latest_time.duration_since(oldest_time).as_secs_f64();
                    let delta_bytes = latest_bytes.saturating_sub(oldest_bytes);

                    if delta_time > 0.0 {
                        return delta_bytes as f64 / delta_time;
                    }
                }
            }

            0.0
        }

        pub fn get_time_span(&self) -> Duration {
            if self.history.len() >= 2 {
                if let (Some(&(oldest_time, _)), Some(&(latest_time, _))) =
                    (self.history.front(), self.history.back())
                {
                    return latest_time.duration_since(oldest_time);
                }
            }
            Duration::from_secs(0)
        }
    }
}
