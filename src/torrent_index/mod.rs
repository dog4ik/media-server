use std::{fmt::Display, str::FromStr};

use reqwest::Url;
use serde::{Deserialize, Serialize, Serializer};
use time::OffsetDateTime;

use crate::{app_state::AppError, metadata::FetchParams};

pub mod rutracker;
pub mod tpb;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum TorrentIndexIdentifier {
    Tpb,
    RuTracker,
}

impl FromStr for TorrentIndexIdentifier {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tpb" => Ok(Self::Tpb),
            "rutracker" => Ok(Self::RuTracker),
            _ => Err(anyhow::anyhow!("Unrecoginzed torrent index: {s}")),
        }
    }
}

impl Display for TorrentIndexIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TorrentIndexIdentifier::Tpb => write!(f, "tpb"),
            TorrentIndexIdentifier::RuTracker => write!(f, "rutracker"),
        }
    }
}

fn serialize_magnet<S: Serializer>(url: &Option<Url>, serializer: S) -> Result<S::Ok, S::Error> {
    if let Some(url) = url {
        serializer.serialize_str(url.as_ref())
    } else {
        serializer.serialize_none()
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Torrent {
    pub name: String,
    #[serde(serialize_with = "serialize_magnet")]
    pub magnet: Option<Url>,
    pub author: Option<String>,
    pub leechers: usize,
    pub seeders: usize,
    pub size: u64,
    pub created: OffsetDateTime,
    pub imdb_id: Option<String>,
    pub provider: TorrentIndexIdentifier,
    pub provider_id: String,
}

#[async_trait::async_trait]
pub trait TorrentIndex {
    async fn search_show_torrent(
        &self,
        query: &str,
        fetch_params: &FetchParams,
    ) -> Result<Vec<Torrent>, AppError>;
    async fn search_movie_torrent(
        &self,
        query: &str,
        fetch_params: &FetchParams,
    ) -> Result<Vec<Torrent>, AppError>;
    async fn search_any_torrent(
        &self,
        query: &str,
        fetch_params: &FetchParams,
    ) -> Result<Vec<Torrent>, AppError>;
    async fn fetch_magnet_link(&self, torrent_id: &str) -> Result<torrent::MagnetLink, AppError>;
    fn provider_identifier(&self) -> &'static str;
}
