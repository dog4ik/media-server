#![feature(map_try_insert)]
#![feature(iter_intersperse)]
#![feature(async_iterator)]
pub mod admin_api;
pub mod utils;
pub mod testing;
pub mod tracing;
pub mod app_state;
pub mod auth;
pub mod db;
pub mod library;
pub mod metadata_provider;
pub mod movie_file;
pub mod process_file;
pub mod progress;
pub mod public_api;
pub mod scan;
pub mod serve_content;
pub mod show_file;
pub mod tmdb_api;
pub mod watch;

