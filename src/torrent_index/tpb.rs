use reqwest::{Client, Method, Request, Url};
use serde::Deserialize;
use time::OffsetDateTime;

use crate::{app_state::AppError, metadata::LimitedRequestClient};

use super::{Torrent, TorrentIndex};

const TRACKERS: &[&str] = &[
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

#[derive(Debug)]
pub struct TpbApi {
    client: LimitedRequestClient,
    base_url: Url,
}

impl TpbApi {
    pub fn new() -> Self {
        let client = Client::new();
        let limited_client = LimitedRequestClient::new(client, 3, std::time::Duration::SECOND);
        let base_url = Url::parse("https://apibay.org/q.php").unwrap();
        Self {
            client: limited_client,
            base_url,
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<TpbTorrent>, AppError> {
        let mut url = self.base_url.clone();
        url.query_pairs_mut().append_pair("q", query);
        let request = Request::new(Method::GET, url);
        self.client.request(request).await
    }
}

#[axum::async_trait]
impl TorrentIndex for TpbApi {
    async fn search_torrent(&self, query: &str) -> Result<Vec<super::Torrent>, AppError> {
        Ok(self
            .search(query)
            .await?
            .into_iter()
            .map(|x| x.into())
            .collect())
    }
}

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

impl Into<Torrent> for TpbTorrent {
    fn into(self) -> Torrent {
        let magnet_link = self.magnet_link();
        let t: i64 = self.added.parse().unwrap();
        let created = OffsetDateTime::from_unix_timestamp(t).unwrap();
        Torrent {
            name: self.name,
            magnet: magnet_link,
            author: Some(self.username),
            leechers: self.leechers.parse().unwrap(),
            seeders: self.seeders.parse().unwrap(),
            size: self.size.parse().unwrap(),
            created,
            imdb_id: self.imdb,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::torrent_index::{tpb::TpbApi, Torrent};

    #[tokio::test]
    async fn tpb_search() {
        let client = TpbApi::new();
        let result = client.search("halo").await.unwrap();
        let t: Torrent = result.into_iter().next().unwrap().into();
        dbg!(&t.magnet.to_string());
        dbg!(&t.name);
        dbg!(&t.created.to_string());
        dbg!(&t.imdb_id);
    }
}
