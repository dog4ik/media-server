use std::time::{Duration, Instant};

use crate::{ban_list::BanList, progress::events::TorrentTickEvents};

#[derive(Debug)]
pub struct TickContext<'a> {
    pub allowed_connections: usize,
    pub tick_start: Instant,
    pub tick_interval: Duration,
    pub events: TorrentTickEvents,
    pub tick_num: usize,
    pub ban_list: &'a BanList,
}

impl TickContext<'_> {
    /// Take the events accumulated so far
    pub fn take_events(&mut self) -> TorrentTickEvents {
        std::mem::take(&mut self.events)
    }
}
