use std::str::FromStr;

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Deserializer, Serialize};
use torrent::Priority;

use crate::{
    app_state::AppError,
    torrent::{TorrentClient, TorrentInfo},
};

#[derive(Debug, Clone)]
pub struct InfoHash(pub [u8; 20]);

impl<'de> Deserialize<'de> for InfoHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HexVisitor;

        impl<'de> serde::de::Visitor<'de> for HexVisitor {
            type Value = InfoHash;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a hex string representing 20 bytes")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                InfoHash::from_str(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(HexVisitor)
    }
}

impl InfoHash {
    /// Hex string of info hash
    pub fn hex(&self) -> String {
        self.to_string()
    }
}

impl FromStr for InfoHash {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        pub fn decode_hex(s: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
                .collect()
        }
        let bytes = decode_hex(s)?;
        if bytes.len() != 20 {
            anyhow::bail!("Expected 20 bytes");
        }
        let mut array = [0u8; 20];
        array.copy_from_slice(&bytes);
        Ok(Self(array))
    }
}

impl AsRef<[u8; 20]> for InfoHash {
    fn as_ref(&self) -> &[u8; 20] {
        &self.0
    }
}

impl std::fmt::Display for InfoHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:x}", bytes::Bytes::copy_from_slice(&self.0))
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct PriorityPayload {
    file: usize,
    #[schema(minimum = 0, maximum = 3)]
    priority: usize,
}

/// Get list of all torrents
#[utoipa::path(
    get,
    path = "/api/torrent/all",
    responses(
        (status = 200, body = Vec<TorrentInfo>),
    ),
    tag = "Torrent",
)]
pub async fn all_torrents(State(client): State<&'static TorrentClient>) -> Json<Vec<TorrentInfo>> {
    Json(client.all_downloads())
}

/// Set file priority
#[utoipa::path(
    post,
    path = "/api/torrent/{info_hash}/file_priority",
    params(
        ("info_hash", description = "Hex encoded info_hash of the torrent"),
    ),
    request_body = PriorityPayload,
    responses(
        (status = 200),
    ),
    tag = "Torrent",
)]
pub async fn set_file_priority(
    Path(info_hash): Path<InfoHash>,
    State(client): State<&'static TorrentClient>,
    Json(payload): Json<PriorityPayload>,
) -> Result<(), AppError> {
    let torrent = client
        .get_download(info_hash.as_ref())
        .ok_or(AppError::not_found("Torrent is not found"))?;
    let priority = Priority::try_from(payload.priority)?;
    if payload.file > torrent.torrent_info.contents.files.len() - 1 {
        return Err(AppError::bad_request("File is out of bounds"));
    }
    torrent
        .download_handle
        .set_file_priority(payload.file, priority)
        .await?;

    Ok(())
}
