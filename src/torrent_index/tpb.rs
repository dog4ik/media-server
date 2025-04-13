use std::str::FromStr;

use reqwest::{Client, Method, Request, Url};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::{
    app_state::AppError,
    metadata::{FetchParams, provod_agent, request_client::LimitedRequestClient},
};

use super::{Torrent, TorrentIndex, TorrentIndexIdentifier};

/// List of default trackers appended to each [Magnet Link](torrent::MagnetLink) in ThePirateBay
const TRACKERS: [&str; 9] = [
    "udp://tracker.opentrackr.org:1337",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://tracker.bittor.pw:1337/announce",
    "udp://public.popcorn-tracker.org:6969/announce",
    "udp://tracker.dler.org:6969/announce",
    "udp://exodus.desync.com:6969",
    "udp://opentracker.i2p.rocks:6969/announce",
];

/// ThePirateBay content caterogy
#[derive(Debug, Clone, Copy)]
pub enum Category {
    Show,
    Movie,
    Any,
}

impl Category {
    pub fn as_str(&self) -> &str {
        match self {
            // movies,hd-movies,4k-movies
            Category::Movie => "201,207,211",
            // shows,hd-shows,4k-shows
            Category::Show => "205,208,212",
            Category::Any => "",
        }
    }
}

#[derive(Debug)]
pub struct TpbApi {
    client: LimitedRequestClient,
    base_url: Url,
}

impl Default for TpbApi {
    fn default() -> Self {
        Self::new()
    }
}

impl TpbApi {
    pub fn new() -> Self {
        let (client, base_url) = provod_agent::new_client("tpb").unwrap_or_else(|e| {
            tracing::warn!("Failed to initialize Provod TPB API: {e}");
            tracing::info!("Using public TPB API");
            (Client::new(), Url::parse("https://apibay.org").unwrap())
        });
        let limited_client =
            LimitedRequestClient::new(client, 1, std::time::Duration::from_secs(1));
        Self {
            client: limited_client,
            base_url,
        }
    }

    pub async fn search(&self, query: &str, cat: Category) -> Result<Vec<TpbTorrent>, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut().expect("base url").push("q.php");
        url.query_pairs_mut().append_pair("q", query);
        url.query_pairs_mut().append_pair("cat", cat.as_str());
        let request = Request::new(Method::GET, url);
        self.client.request(request).await
    }

    pub async fn get_magnet_link(&self, id: &str) -> Result<torrent::MagnetLink, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut().expect("base url").push("t.php");
        url.query_pairs_mut().append_pair("id", id);
        let request = Request::new(Method::GET, url);
        let torrent: TpbTorrent = self.client.request(request).await?;
        Ok(torrent::MagnetLink::from_str(
            &torrent.magnet_link().to_string(),
        )?)
    }
}

#[async_trait::async_trait]
impl TorrentIndex for TpbApi {
    async fn search_movie_torrent(
        &self,
        query: &str,
        _: &FetchParams,
    ) -> Result<Vec<super::Torrent>, AppError> {
        Ok(self
            .search(query, Category::Movie)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    async fn search_show_torrent(
        &self,
        query: &str,
        _: &FetchParams,
    ) -> Result<Vec<super::Torrent>, AppError> {
        Ok(self
            .search(query, Category::Show)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }
    async fn search_any_torrent(
        &self,
        query: &str,
        _: &FetchParams,
    ) -> Result<Vec<super::Torrent>, AppError> {
        Ok(self
            .search(query, Category::Any)
            .await?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    async fn fetch_magnet_link(&self, torrent_id: &str) -> Result<torrent::MagnetLink, AppError> {
        Ok(self.get_magnet_link(torrent_id).await?)
    }

    fn provider_identifier(&self) -> TorrentIndexIdentifier {
        TorrentIndexIdentifier::Tpb
    }
}

#[allow(unused)]
#[derive(Deserialize, Debug, Clone)]
pub struct TpbTorrent {
    id: String,
    name: String,
    info_hash: String,
    leechers: String,
    seeders: String,
    num_files: String,
    size: String,
    username: String,
    added: String,
    status: String,
    category: String,
    imdb: String,
}

impl TpbTorrent {
    pub fn magnet_link(&self) -> Url {
        let mut url = Url::parse("magnet:").unwrap();
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("xt", &format!("urn:btih:{}", self.info_hash));
            query.append_pair("dn", &self.name);
            for tracker in TRACKERS {
                query.append_pair("tr", tracker);
            }
        }
        url
    }
}

impl From<TpbTorrent> for Torrent {
    fn from(val: TpbTorrent) -> Self {
        let magnet_link = val.magnet_link();
        let t: i64 = val.added.parse().unwrap();
        let created = OffsetDateTime::from_unix_timestamp(t).unwrap();
        Torrent {
            name: val.name,
            magnet: Some(magnet_link),
            author: Some(val.username),
            leechers: val.leechers.parse().unwrap(),
            seeders: val.seeders.parse().unwrap(),
            size: val.size.parse().unwrap(),
            created,
            imdb_id: Some(val.imdb),
            provider: super::TorrentIndexIdentifier::Tpb,
            provider_id: val.id,
        }
    }
}
