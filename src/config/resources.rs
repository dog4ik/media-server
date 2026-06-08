use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
    time::SystemTime,
};

use serde::{Deserialize, Serialize};
use sysinfo::System;

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AppResources {
    #[schema(value_type = String)]
    pub start_time: SystemTime,
    #[schema(value_type = String)]
    pub database_path: PathBuf,
    #[schema(value_type = String)]
    #[serde(skip)]
    pub config_path: PathBuf,
    #[schema(value_type = String)]
    pub resources_path: PathBuf,
    #[schema(value_type = String)]
    pub temp_path: PathBuf,
    #[schema(value_type = String)]
    pub statics_path: PathBuf,
    #[schema(value_type = String)]
    pub log_path: PathBuf,
    pub os: String,
    pub os_version: String,
    pub app_version: &'static str,
}

pub static APP_RESOURCES: LazyLock<AppResources> = LazyLock::new(AppResources::new);

/// Service directory environment variables exported by systemd when the unit
/// declares the matching `*Directory=` option. systemd creates the directories
/// with the correct ownership, so a system service stores its data under the FHS
/// locations (e.g. `/var/lib/media-server`) instead of an XDG path under the
/// service account's home directory.
///
/// systemd is Linux-only, so this is gated to `target_os = "linux"`.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
enum SystemdDirectory {
    /// Persistent service state, e.g. the database and config (`StateDirectory=`).
    State,
    /// Non-essential cached/temporary data (`CacheDirectory=`).
    Cache,
    /// Service log files (`LogsDirectory=`).
    Logs,
}

#[cfg(target_os = "linux")]
impl SystemdDirectory {
    const fn as_str(self) -> &'static str {
        match self {
            Self::State => "STATE_DIRECTORY",
            Self::Cache => "CACHE_DIRECTORY",
            Self::Logs => "LOGS_DIRECTORY",
        }
    }
}

impl AppResources {
    pub const APP_NAME: &'static str = "media-server";

    fn static_storage() -> PathBuf {
        if Self::is_prod() {
            #[cfg(windows)]
            {
                std::env::current_exe()
                    .ok()
                    .as_deref()
                    .and_then(Path::parent)
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| {
                        tracing::error!("Failed to get exe path, fallinig back to Program Files");
                        PathBuf::from(
                            std::env::var("PROGRAMFILES")
                                .expect("program files are always defined"),
                        )
                        .join(Self::APP_NAME)
                    })
            }

            #[cfg(not(windows))]
            {
                Path::new("/usr/share").join(Self::APP_NAME)
            }
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        }
    }

    /// First path from a colon-separated systemd directory environment variable
    /// or `None` when the variable is unset (i.e. the process is not running under a systemd unit that declares the directory).
    #[cfg(target_os = "linux")]
    fn systemd_dir(dir: SystemdDirectory) -> Option<PathBuf> {
        let value = std::env::var_os(dir.as_str())?;
        std::env::split_paths(&value)
            .next()
            .filter(|p| !p.as_os_str().is_empty())
    }

    fn data_storage() -> PathBuf {
        if Self::is_prod() {
            #[cfg(target_os = "linux")]
            if let Some(dir) = Self::systemd_dir(SystemdDirectory::State) {
                return dir;
            }
            dirs::data_local_dir()
                .expect("target to have data directory")
                .join(Self::APP_NAME)
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        }
    }

    pub fn is_prod() -> bool {
        !cfg!(debug_assertions)
    }

    pub fn default_config_path() -> PathBuf {
        if Self::is_prod() {
            #[cfg(target_os = "linux")]
            if let Some(dir) = Self::systemd_dir(SystemdDirectory::State) {
                return dir.join("configuration.toml");
            }
            dirs::config_local_dir()
                .expect("target supports config dir")
                .join(Self::APP_NAME)
        } else {
            Self::data_storage()
        }
        .join("configuration.toml")
    }

    fn temp_storage() -> PathBuf {
        #[cfg(target_os = "linux")]
        if Self::is_prod() {
            if let Some(dir) = Self::systemd_dir(SystemdDirectory::Cache) {
                return dir;
            }
        }
        Self::data_storage().join("tmp")
    }

    fn database_directory() -> PathBuf {
        Self::data_storage().join("db")
    }

    fn resources() -> PathBuf {
        Self::data_storage().join("resources")
    }

    fn database() -> PathBuf {
        Self::database_directory().join("database.sqlite")
    }

    pub fn log() -> PathBuf {
        #[cfg(target_os = "linux")]
        if Self::is_prod() {
            if let Some(dir) = Self::systemd_dir(SystemdDirectory::Logs) {
                return dir.join("log.log");
            }
        }
        Self::data_storage().join("log.log")
    }

    pub fn initiate() -> Result<(), std::io::Error> {
        use std::fs;
        fs::create_dir_all(Self::resources())?;
        fs::create_dir_all(Self::database_directory())?;
        fs::create_dir_all(Self::temp_storage())?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(Self::database())?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(Self::log())?;
        Ok(())
    }

    pub fn new() -> Self {
        let start_time = SystemTime::now();
        let config_path = Self::default_config_path();
        let resources_path = Self::resources();
        let database_path = Self::database();
        let temp_path = Self::temp_storage();
        let log_path = Self::log();

        let statics_path = Self::static_storage();
        let (os_version, os) = System::kernel_version()
            .zip(System::long_os_version())
            .expect("all supported targets give us os version");
        let app_version = std::env!("CARGO_PKG_VERSION");

        tracing::debug!(path = %config_path.display(), "Selected config path");
        tracing::debug!(path = %statics_path.display(), "Selected statics folder path");
        tracing::debug!(path = %resources_path.display(), "Selected resources path");
        tracing::debug!(path = %database_path.display(), "Selected database path");
        tracing::debug!(path = %temp_path.display(), "Selected tmp path");
        tracing::debug!(path = %log_path.display(), "Selected log path");
        tracing::info!("Server version: {app_version}");

        Self {
            start_time,
            config_path,
            database_path,
            resources_path,
            temp_path,
            statics_path,
            log_path,
            os_version,
            os,
            app_version,
        }
    }
}

impl Default for AppResources {
    fn default() -> Self {
        Self::new()
    }
}
