use std::{path::Path, str::FromStr};

use anyhow::{anyhow, ensure};
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::protocol::Info;

#[derive(Debug, Deserialize, Serialize)]
pub struct TorrentFile {
    pub info: Info,
    pub announce: String,
    pub encoding: Option<String>,
    #[serde(rename = "announce-list")]
    pub announce_list: Option<Vec<Vec<String>>>,
    #[serde(rename = "creation date")]
    pub creation_date: Option<u64>,
    pub comment: Option<String>,
    #[serde(rename = "created by")]
    pub created_by: Option<String>,
}

impl TorrentFile {
    pub fn from_bytes(bytes: impl AsRef<[u8]>) -> Result<Self, serde_bencode::Error> {
        serde_bencode::from_bytes(bytes.as_ref())
    }

    pub fn from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        use std::fs;
        let bytes = fs::read(path)?;
        let torrent = Self::from_bytes(bytes)?;
        Ok(torrent)
    }

    /// Get all trackers contained in file
    pub fn all_trackers(&self) -> Vec<Url> {
        let mut trackers =
            Vec::with_capacity(1 + self.announce_list.as_ref().map_or(0, |l| l.len()));
        if let Ok(url) = Url::parse(&self.announce) {
            trackers.push(url);
        } else {
            tracing::error!(
                self.announce,
                "failed to parce announce url in .torrent file"
            );
        }
        if let Some(list) = &self.announce_list {
            trackers.extend(
                list.into_iter()
                    .flatten()
                    .filter_map(|url| Url::parse(url).ok()),
            );
        };
        trackers
    }
}

#[derive(Debug, Clone)]
pub struct MagnetLink {
    pub announce_list: Option<Vec<Url>>,
    pub name: Option<String>,
    pub info_hash: String,
}

impl FromStr for MagnetLink {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = reqwest::Url::from_str(s)?;
        ensure!(url.scheme() == "magnet");
        let mut info_hash = None;
        let mut name = None;
        let mut trackers = Vec::new();
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                // info_hash
                "xt" => {
                    let mut split = value.splitn(3, ':');
                    let urn = split
                        .next()
                        .ok_or(anyhow!("urn string is not found in xt"))?;
                    let hash_indicator = split
                        .next()
                        .ok_or(anyhow!("hash indicator is not found in xt"))?;
                    ensure!(urn == "urn");
                    ensure!(hash_indicator == "btih");
                    let hash = split.next().ok_or(anyhow!("hash is not found in xt"))?;
                    ensure!(hash.len() == 40);
                    info_hash = Some(hash.to_string());
                }
                // torrent name
                "dn" => {
                    name = Some(value.to_string());
                }
                // tracker
                "tr" => {
                    if let Ok(url) = Url::from_str(&value) {
                        trackers.push(url)
                    } else {
                        tracing::warn!("Failed to parse magnet tracker: {}", value);
                    }
                }
                _ => {}
            }
        }
        let trackers = (!trackers.is_empty()).then_some(trackers);
        Ok(Self {
            info_hash: info_hash.ok_or(anyhow!("magnet link does not contain info_hash"))?,
            name,
            announce_list: trackers,
        })
    }
}

impl MagnetLink {
    pub fn hash(&self) -> [u8; 20] {
        hex::decode(&self.info_hash).unwrap().try_into().unwrap()
    }
}

#[cfg(test)]
mod tests {

    use crate::file::{MagnetLink, TorrentFile};

    use std::fs;
    use std::str::FromStr;

    #[test]
    fn parse_torrent_file() {
        let contents = fs::read("sample.torrent").unwrap();
        let torrent_file = TorrentFile::from_bytes(&contents).unwrap();
        assert_eq!(
            torrent_file.announce,
            "http://bittorrent-test-tracker.codecrafters.io/announce"
        );
        assert_eq!(torrent_file.info.name, "sample.txt");
        assert_eq!(torrent_file.created_by.unwrap(), "mktorrent 1.1");
        assert_eq!(torrent_file.info.total_size(), 92063);
        assert_eq!(
            torrent_file.info.hex_hash(),
            "d69f91e6b2ae4c542468d1073a71d4ea13879a7f"
        );
    }

    #[test]
    fn parse_magnet_link() {
        let contents = "magnet:?xt=urn:btih:BE2D7CD9F6B0FDFC035EDFEE4EBD567003EBC254&dn=Rick.and.Morty.S07E01.1080p.WEB.H264-NHTFS%5BTGx%5D&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce&tr=udp%3A%2F%2Ftracker.torrent.eu.org%3A451%2Fannounce&tr=udp%3A%2F%2Ftracker.bittor.pw%3A1337%2Fannounce&tr=udp%3A%2F%2Fpublic.popcorn-tracker.org%3A6969%2Fannounce&tr=udp%3A%2F%2Ftracker.dler.org%3A6969%2Fannounce&tr=udp%3A%2F%2Fexodus.desync.com%3A6969&tr=udp%3A%2F%2Fopen.demonii.com%3A1337%2Fannounce";
        let expected_trackers = [
            "udp://tracker.opentrackr.org:1337",
            "udp://open.stealth.si:80/announce",
            "udp://tracker.torrent.eu.org:451/announce",
            "udp://tracker.bittor.pw:1337/announce",
            "udp://public.popcorn-tracker.org:6969/announce",
            "udp://tracker.dler.org:6969/announce",
            "udp://exodus.desync.com:6969",
            "udp://open.demonii.com:1337/announce",
        ];
        let expected_info_hash = "BE2D7CD9F6B0FDFC035EDFEE4EBD567003EBC254";
        let expected_name = "Rick.and.Morty.S07E01.1080p.WEB.H264-NHTFS[TGx]";
        let magnet_link = MagnetLink::from_str(&contents).unwrap();
        assert_eq!(magnet_link.info_hash, expected_info_hash);
        assert_eq!(magnet_link.name.unwrap(), expected_name);
        let announce_list = magnet_link.announce_list.unwrap();
        assert_eq!(announce_list.len(), expected_trackers.len());
        for (actual_url, expected_url) in announce_list.iter().zip(expected_trackers) {
            assert_eq!(actual_url.to_string(), expected_url);
        }
    }
}
