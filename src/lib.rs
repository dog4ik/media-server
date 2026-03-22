#![windows_subsystem = "windows"]
#![doc = include_str!("../README.md")]

/// Api surface of the media server
pub mod api;
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
/// Library files, config file watcher
#[allow(unused)]
pub mod file_watcher;
/// Chromaprint intro detection module
pub mod intro_detection;
/// Everything related to local media files
pub mod library;
/// Integrations with movie and TV databases.
pub mod metadata;
/// Progress notifications dispatched to the connected Websockets clients
pub mod progress;
/// Library scan module
///
/// There are 3 things must be done during scan.
/// 1. Metadata fetch. It can be found locally or fetched from providers.
/// 2. New metadata and assets must be saved.
/// 3. Library items should be linked to their metadata
pub mod scan;
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
/// Content streams
pub mod watch;
/// Websockets clients connection
pub mod ws;

/// Wrapper around `time::OffsetDateTime`
#[derive(
    Debug,
    utoipa::ToSchema,
    serde::Serialize,
    serde::Deserialize,
    sqlx::Type,
    Clone,
    PartialEq,
    PartialOrd,
    Eq,
    Ord,
)]
#[serde(transparent)]
#[sqlx(transparent)]
pub struct OffsetDateTime(#[serde(with = "time::serde::rfc3339")] pub time::OffsetDateTime);

impl From<time::OffsetDateTime> for OffsetDateTime {
    fn from(value: time::OffsetDateTime) -> Self {
        Self(value)
    }
}

/// Wrapper around [std::time::Duration] that is serialized in milliseconds
#[derive(Debug, utoipa::ToSchema, Clone, PartialEq, PartialOrd, Eq, Ord)]
#[schema(value_type = u128)]
pub struct MediaDuration(pub std::time::Duration);

impl serde::Serialize for MediaDuration {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u128(self.0.as_millis())
    }
}

impl<'de> serde::Deserialize<'de> for MediaDuration {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DurationVisitor;
        impl serde::de::Visitor<'_> for DurationVisitor {
            type Value = MediaDuration;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("Duration time in milliseconds")
            }
            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(std::time::Duration::from_millis(v).into())
            }
        }

        deserializer.deserialize_u64(DurationVisitor)
    }
}

impl From<std::time::Duration> for MediaDuration {
    fn from(value: std::time::Duration) -> Self {
        Self(value)
    }
}
