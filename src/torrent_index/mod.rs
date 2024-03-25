use reqwest::Url;
use serde::{Serialize, Serializer};
use time::OffsetDateTime;

use crate::app_state::AppError;

pub mod tpb;

fn serialize_url<S: Serializer>(url: &Url, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&url.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct Torrent {
    pub name: String,
    #[serde(serialize_with = "serialize_url")]
    pub magnet: Url,
    pub author: Option<String>,
    pub leechers: usize,
    pub seeders: usize,
    pub size: usize,
    pub created: OffsetDateTime,
    pub imdb_id: String,
}

#[axum::async_trait]
pub trait TorrentIndex {
    async fn search_torrent(&self, query: &str) -> Result<Vec<Torrent>, AppError>;
}
