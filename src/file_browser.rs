use std::path::{Component, PathBuf};

use base64::Engine;
use serde::{de::Visitor, Deserialize, Serialize};
use tokio::fs;

#[derive(Debug)]
/// Base64 -> Path deserializable path. Used for encoding paths in url
pub struct FileKey {
    pub path: PathBuf,
}

impl<'de> Deserialize<'de> for FileKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct KeyVisitor;

        impl<'de> Visitor<'de> for KeyVisitor {
            type Value = FileKey;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "base64 encoded representation of path")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let engine = base64::prelude::BASE64_URL_SAFE;
                let decoded = engine
                    .decode(v)
                    .map_err(|e| E::custom(format!("Failed to decode base64 string: {}", e)))?;
                let path = PathBuf::from(String::from_utf8_lossy(&decoded).to_string());
                Ok(FileKey { path })
            }
        }
        deserializer.deserialize_str(KeyVisitor)
    }
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BrowseDirectory {
    size: usize,
    directories: Vec<BrowseFile>,
    files: Vec<BrowseFile>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BrowseFile {
    title: String,
    #[schema(value_type = String)]
    path: PathBuf,
    key: String,
}

impl From<PathBuf> for BrowseFile {
    fn from(path: PathBuf) -> Self {
        let title = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let engine = base64::prelude::BASE64_URL_SAFE;
        let key = engine.encode(path.to_string_lossy().as_bytes());
        Self { title, path, key }
    }
}

const EXPLORE_LIMIT: usize = 500;

impl BrowseDirectory {
    pub async fn explore(key: FileKey) -> anyhow::Result<Self> {
        let mut directory = fs::read_dir(key.path).await?;
        let mut size = 0;
        let mut directories = Vec::new();
        let mut files = Vec::new();
        while let Ok(Some(entry)) = directory.next_entry().await {
            if size >= EXPLORE_LIMIT {
                tracing::warn!("File browser reached explore limit: {}", EXPLORE_LIMIT);
                break;
            }

            let Ok(file_type) = entry.file_type().await else {
                continue;
            };

            let path = entry.path();
            if file_type.is_dir() {
                directories.push(path.into());
            } else if file_type.is_file() {
                files.push(path.into())
            } else {
                continue;
            }
            size += 1;
        }

        Ok(Self {
            size,
            directories,
            files,
        })
    }
}

#[derive(Serialize, Deserialize, utoipa::ToSchema)]
pub struct BrowseRootDirs {
    home: Option<BrowseFile>,
    root: BrowseFile,
    videos: Option<BrowseFile>,
    disks: Vec<BrowseFile>,
}

impl BrowseRootDirs {
    pub fn new() -> Self {
        let disks = sysinfo::Disks::new_with_refreshed_list();
        let mut disks_mount_points = Vec::with_capacity(disks.list().len());
        let root: PathBuf = Component::RootDir.as_os_str().into();

        for disk in disks.list() {
            let mount_point = disk.mount_point();
            if mount_point != root {
                disks_mount_points.push(BrowseFile::from(mount_point.to_owned()));
            }
        }

        let videos = dirs::video_dir().and_then(|d| {
            d.try_exists()
                .unwrap_or(false)
                .then_some(BrowseFile::from(d))
        });
        let home = dirs::home_dir().and_then(|d| {
            d.try_exists()
                .unwrap_or(false)
                .then_some(BrowseFile::from(d))
        });

        Self {
            home,
            root: root.into(),
            disks: disks_mount_points,
            videos,
        }
    }
}
