#![feature(iter_intersperse)]
#![feature(iter_array_chunks)]
#![feature(array_chunks)]
#![doc = include_str!("../README.md")]

/// Shared state of the application
pub mod app_state;
/// All server related configuration
pub mod config;
/// Sqlite database
pub mod db;
/// FFmpeg cli api
///
/// Currently used for everything except probing
pub mod ffmpeg;
/// FFmpeg abi api
///
/// Currently used only for metadata retrieval
pub mod ffmpeg_abi;
/// File browser
pub mod file_browser;
/// Chromaprint intro detection module
pub mod intro_detection;
/// Everything related to local media files
pub mod library;
/// Integrations with movie and TV databases.
pub mod metadata;
/// Progress notifications dispatched to the connected Websockets clients
pub mod progress;
/// Api surface of the media server
pub mod server;
/// Content streams
pub mod stream;
/// Glue between torrent crate and media server
pub mod torrent;
/// Torrent providers
pub mod torrent_index;
/// Everything related to logging
pub mod tracing;
/// Tray icon implementation. Currently supports only windows
#[cfg(feature = "windows-tray")]
pub mod tray;
/// Universal Plug and Play capabilities of the server
pub mod upnp;
pub mod utils;
/// Library files, config file watcher
#[allow(unused)]
pub mod watch;
/// Websockets clients connection
pub mod ws;
