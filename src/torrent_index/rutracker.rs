use anyhow::Context;
use reqwest::{Method, Request, Url};
use serde::Deserialize;

use crate::{
    app_state::AppError,
    metadata::{FetchParams, provod_agent, request_client::LimitedRequestClient},
};

use super::{Torrent, TorrentIndex};

/// Rutracker torrent index Provod adapter.
///
/// Note that Rutracker does not have its own api that makes scraping very hard.
/// This is why we can't "mimic" its api surface with provod.
#[derive(Debug)]
pub struct ProvodRuTrackerAdapter {
    pub base_url: Url,
    pub limited_client: LimitedRequestClient,
}

#[derive(Debug, Deserialize)]
pub struct ProvodRuTrackerTorrent {
    title: String,
    author: String,
    size: u64,
    seeds: usize,
    leeches: usize,
    created_at: String,
    index_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ProvodRuTrackerMagnetLink {
    magnet_link: String,
}

impl From<ProvodRuTrackerTorrent> for Torrent {
    fn from(value: ProvodRuTrackerTorrent) -> Self {
        let created = time::OffsetDateTime::parse(
            &value.created_at,
            &time::format_description::well_known::Iso8601::DEFAULT,
        )
        .expect("iso8601 date");
        Self {
            name: value.title,
            magnet: None,
            author: Some(value.author),
            leechers: value.leeches,
            seeders: value.seeds,
            size: value.size,
            created,
            imdb_id: None,
            provider: super::TorrentIndexIdentifier::RuTracker,
            provider_id: value.index_id,
        }
    }
}

impl ProvodRuTrackerAdapter {
    pub fn new() -> Self {
        let (client, base_url) = provod_agent::new_client("rutracker");
        let limited_client =
            LimitedRequestClient::new(client, 5, std::time::Duration::from_secs(1));
        Self {
            base_url,
            limited_client,
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<ProvodRuTrackerTorrent>, AppError> {
        let mut url = self.base_url.clone();
        url.query_pairs_mut().append_pair("search", query);
        let req = Request::new(Method::GET, url);
        self.limited_client.request(req).await
    }

    pub async fn get_torrent_file(&self, id: &str) -> Result<torrent::TorrentFile, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut().expect("base url").push("download");
        url.query_pairs_mut().append_pair("id", id);
        let req = Request::new(Method::GET, url);
        let res = self.limited_client.request_raw(req).await?;
        let bytes = res.bytes().await.context("collect response body bytes")?;
        Ok(torrent::TorrentFile::from_bytes(&bytes)?)
    }

    pub async fn get_magnet_link(&self, id: &str) -> Result<torrent::MagnetLink, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .expect("base url")
            .push("magnet_link");
        url.query_pairs_mut().append_pair("id", id);
        let req = Request::new(Method::GET, url);
        let res: ProvodRuTrackerMagnetLink = self.limited_client.request(req).await?;

        Ok(res.magnet_link.parse()?)
    }
}

#[async_trait::async_trait]
impl TorrentIndex for ProvodRuTrackerAdapter {
    async fn search_show_torrent(
        &self,
        query: &str,
        _fetch_params: &FetchParams,
    ) -> Result<Vec<Torrent>, AppError> {
        Ok(self
            .search(query)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn search_movie_torrent(
        &self,
        query: &str,
        _fetch_params: &FetchParams,
    ) -> Result<Vec<Torrent>, AppError> {
        Ok(self
            .search(query)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn search_any_torrent(
        &self,
        query: &str,
        _fetch_params: &FetchParams,
    ) -> Result<Vec<Torrent>, AppError> {
        Ok(self
            .search(query)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn fetch_magnet_link(&self, torrent_id: &str) -> Result<torrent::MagnetLink, AppError> {
        Ok(self.get_magnet_link(torrent_id).await?)
    }

    fn provider_identifier(&self) -> &'static str {
        "rutracker"
    }
}
