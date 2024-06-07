#![feature(map_try_insert)]
#![feature(duration_constructors)]
#![feature(duration_constants)]
#![feature(iter_intersperse)]
#![feature(os_str_display)]
pub mod app_state;
pub mod config;
pub mod db;
pub mod ffmpeg;
pub mod library;
pub mod metadata;
pub mod progress;
pub mod server;
pub mod stream;
pub mod torrent_index;
pub mod tracing;
#[cfg(feature = "windows-tray")]
pub mod tray;
pub mod utils;
pub mod watch;
