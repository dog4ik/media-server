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
