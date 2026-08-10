#![windows_subsystem = "windows"]
#![doc = include_str!("../README.md")]

use std::{error::Error, fmt::Display};

use reqwest::StatusCode;

use crate::api::Json;

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
pub mod lists;
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

pub type BoxedFuture<'a, T> = std::pin::Pin<Box<dyn Future<Output = T> + 'a + Send>>;

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

impl From<OffsetDateTime> for time::OffsetDateTime {
    fn from(OffsetDateTime(value): OffsetDateTime) -> Self {
        value
    }
}

/// Wrapper around [std::time::Duration] that is serialized in milliseconds
#[derive(Debug, utoipa::ToSchema, Clone, PartialEq, PartialOrd, Eq, Ord)]
#[schema(value_type = u128)]
pub struct MediaDuration(pub std::time::Duration);

impl serde::Serialize for MediaDuration {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u128(self.0.as_millis())
    }
}

impl<'de> serde::Deserialize<'de> for MediaDuration {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct DurationVisitor;
        impl serde::de::Visitor<'_> for DurationVisitor {
            type Value = MediaDuration;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("Duration time in milliseconds")
            }
            fn visit_u64<E>(self, v: u64) -> std::result::Result<Self::Value, E>
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

pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct AppError {
    pub message: String,
    pub kind: AppErrorKind,
}

#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AppErrorKind {
    /// Unclassified internal error
    InternalError,
    /// Resource was not found
    NotFound,
    /// There resource is duplicated
    Duplicate,
    BadRequest,
    /// Sqlite database is busy
    DatabaseLocked,
    Unprocessable,
}

impl Display for AppErrorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppErrorKind::InternalError => f.write_str("Internal error"),
            AppErrorKind::NotFound => f.write_str("Not found"),
            AppErrorKind::Duplicate => f.write_str("Duplicate"),
            AppErrorKind::BadRequest => f.write_str("Bad request"),
            AppErrorKind::DatabaseLocked => f.write_str("Database locked"),
            AppErrorKind::Unprocessable => f.write_str("Unprocessable entity"),
        }
    }
}

impl Error for AppError {}

impl Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl From<AppErrorKind> for StatusCode {
    fn from(val: AppErrorKind) -> Self {
        match val {
            AppErrorKind::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
            AppErrorKind::NotFound => StatusCode::NOT_FOUND,
            AppErrorKind::Duplicate => StatusCode::CONFLICT,
            AppErrorKind::BadRequest => StatusCode::BAD_REQUEST,
            AppErrorKind::DatabaseLocked => StatusCode::SERVICE_UNAVAILABLE,
            AppErrorKind::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
        }
    }
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            message: err.to_string(),
            kind: AppErrorKind::InternalError,
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(value: sqlx::Error) -> Self {
        match value {
            sqlx::Error::RowNotFound => AppError {
                message: "Database row not found".to_string(),
                kind: AppErrorKind::NotFound,
            },
            sqlx::Error::Database(err) if err.is_unique_violation() => AppError {
                message: "Database unique violation".to_string(),
                kind: AppErrorKind::Duplicate,
            },
            rest => AppError {
                message: format!("{}", rest),
                kind: AppErrorKind::InternalError,
            },
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        match value.kind() {
            std::io::ErrorKind::NotFound => AppError {
                message: value.to_string(),
                kind: AppErrorKind::NotFound,
            },
            _ => AppError {
                message: value.to_string(),
                kind: AppErrorKind::InternalError,
            },
        }
    }
}

impl From<std::num::ParseIntError> for AppError {
    fn from(value: std::num::ParseIntError) -> Self {
        AppError {
            message: value.to_string(),
            kind: AppErrorKind::BadRequest,
        }
    }
}

impl AppError {
    pub fn new(message: impl AsRef<str>, kind: AppErrorKind) -> Self {
        Self {
            message: message.as_ref().into(),
            kind,
        }
    }

    pub fn not_found(msg: impl AsRef<str>) -> AppError {
        AppError {
            message: msg.as_ref().into(),
            kind: AppErrorKind::NotFound,
        }
    }

    pub fn bad_request(msg: impl AsRef<str>) -> AppError {
        AppError {
            message: msg.as_ref().into(),
            kind: AppErrorKind::BadRequest,
        }
    }

    pub fn unprocessable(msg: impl AsRef<str>) -> AppError {
        AppError {
            message: msg.as_ref().into(),
            kind: AppErrorKind::Unprocessable,
        }
    }

    pub fn internal_error(msg: impl AsRef<str>) -> AppError {
        AppError {
            message: msg.as_ref().into(),
            kind: AppErrorKind::InternalError,
        }
    }
}

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let status: StatusCode = self.kind.clone().into();
        (status, Json(self)).into_response()
    }
}
