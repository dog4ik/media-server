#![feature(iter_intersperse)]
#![feature(os_str_display)]
#![feature(array_chunks)]
pub mod app_state;
pub mod config;
pub mod db;
pub mod ffmpeg;
pub mod file_browser;
pub mod intro_detection;
pub mod library;
pub mod metadata;
pub mod progress;
pub mod server;
pub mod stream;
pub mod torrent;
pub mod torrent_index;
pub mod tracing;
#[cfg(feature = "windows-tray")]
pub mod tray;
pub mod upnp;
pub mod utils;
#[allow(unused)]
pub mod watch;
