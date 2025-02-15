use reqwest::Url;
use serde::{Serialize, Serializer};
use time::OffsetDateTime;

use crate::{app_state::AppError, metadata::FetchParams};

pub mod tpb;

fn serialize_url<S: Serializer>(url: &Url, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(url.as_ref())
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
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
    fn provider_identifier(&self) -> &'static str;
}
