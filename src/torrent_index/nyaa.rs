use std::collections::BTreeMap;

use reqwest::Url;
use rss::Channel;

use crate::{
    AppError,
    metadata::FetchParams,
    torrent_index::{TorrentIndex, TorrentIndexIdentifier},
};

#[derive(Debug)]
pub struct NyaaApi {
    client: reqwest::Client,
    base_url: Url,
}

const TRACKER_LIST: &[&str] = &[
    "http://nyaa.tracker.wf:7777/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://exodus.desync.com:6969/announce",
    "udp://tracker.torrent.eu.org:451/announce",
];

fn extension_value<'a, 'b>(
    extension: &'a BTreeMap<String, Vec<rss::extension::Extension>>,
    key: &'b str,
) -> Option<&'a str> {
    extension
        .get(key)
        .and_then(|v| v.first())
        .and_then(|v| v.value())
}

impl NyaaApi {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client,
            base_url: "https://nyaa.si".parse().expect("nyaa url must be valid"),
        }
    }

    pub async fn search(&self, query: &str) -> anyhow::Result<Vec<super::Torrent>> {
        let mut url = self.base_url.clone();
        url.query_pairs_mut()
            .append_pair("page", "rss")
            .append_pair("q", query);
        let bytes = self.client.get(url).send().await?.bytes().await?;
        let channel = Channel::read_from(&bytes[..])?;
        let torrents = channel
            .items
            .into_iter()
            .filter_map(|item| {
                let extension = item.extensions.get("nyaa")?;

                let created = item.pub_date().and_then(|v| {
                    time::OffsetDateTime::parse(v, &time::format_description::well_known::Rfc2822)
                        .ok()
                })?;

                let info_hash = extension_value(extension, "infoHash")?;
                let name = item.title?;

                let magnet = torrent::MagnetLink {
                    announce_list: Some(
                        TRACKER_LIST
                            .iter()
                            .map(|url| Url::parse(url).expect("all static urls are valid"))
                            .collect(),
                    ),
                    name: Some(name.clone()),
                    info_hash: info_hash.to_string(),
                }
                .url();

                let leechers = extension_value(extension, "leechers")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let seeders = extension_value(extension, "seeders")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_default();
                let size = extension_value(extension, "size")
                    .and_then(parse_nyaa_size)
                    .unwrap_or_default();

                Some(super::Torrent {
                    name,
                    magnet: Some(magnet),
                    author: item.author,
                    leechers,
                    seeders,
                    size,
                    created,
                    imdb_id: None,
                    provider: super::TorrentIndexIdentifier::Nyaa,
                    provider_id: item.guid?.value,
                })
            })
            .collect();
        Ok(torrents)
    }
}

#[async_trait::async_trait]
impl TorrentIndex for NyaaApi {
    async fn search_movie_torrent(
        &self,
        _query: &str,
        _: &FetchParams,
    ) -> crate::Result<Vec<super::Torrent>> {
        Err(AppError::bad_request("movie search is not supported"))
    }

    async fn search_show_torrent(
        &self,
        query: &str,
        _: &FetchParams,
    ) -> crate::Result<Vec<super::Torrent>> {
        Ok(self.search(query).await?)
    }

    async fn search_any_torrent(
        &self,
        query: &str,
        _: &FetchParams,
    ) -> crate::Result<Vec<super::Torrent>> {
        Ok(self.search(query).await?)
    }

    async fn fetch_magnet_link(&self, _torrent_id: &str) -> crate::Result<torrent::MagnetLink> {
        Err(AppError::bad_request("magnet link fetch not supported"))
    }

    fn provider_identifier(&self) -> TorrentIndexIdentifier {
        TorrentIndexIdentifier::Nyaa
    }
}

fn parse_nyaa_size(nyaa_size_str: &str) -> Option<u64> {
    let (value, size) = nyaa_size_str.split_once(' ')?;
    let value: f64 = value.parse().ok()?;
    let bytes = match size {
        "KiB" => value * 1024.,
        "MiB" => value * 1024. * 1024.,
        "GiB" => value * 1024. * 1024. * 1024.,
        _ => return None,
    };
    u64::try_from(bytes.floor() as i64).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! parse {
        ($desc: ident, $input: literal => None) => {
            #[test]
            fn $desc() {
                assert_eq!(
                    parse_nyaa_size($input),
                    None,
                    concat!("expected ", $input, " parse to None")
                );
            }
        };
        ($desc: ident, $input: literal => $expected: expr) => {
            #[test]
            fn $desc() {
                assert_eq!(
                    parse_nyaa_size($input),
                    Some(f64::floor($expected) as u64),
                    concat!("failed to parse ", $input)
                );
            }
        };
    }

    parse!(size_rounded_kb, "292 KiB" => 292. * 1024.);
    parse!(size_float_kb, "1.11 KiB" => 1.11 * 1024.);
    parse!(size_float_mb, "4.1 MiB" => 4.1 * 1024. * 1024.);
    parse!(size_float_gb, "2.5 GiB" => 2.5 * 1024. * 1024. * 1024.);
    parse!(size_unknown_none, "2.5 DiB" => None);
    parse!(size_empty_none, "" => None);
}
