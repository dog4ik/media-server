use std::{fmt::Display, str::FromStr};

use anyhow::Context;
use reqwest::Url;

#[derive(Debug, Clone)]
pub struct MagnetLink {
    pub announce_list: Option<Vec<Url>>,
    pub name: Option<String>,
    pub info_hash: String,
}

impl Display for MagnetLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let mut url = Url::parse(&format!("magnet:?xt=urn:btih:{}", self.info_hash)).unwrap();
        {
            let mut query = url.query_pairs_mut();
            if let Some(name) = &self.name {
                query.append_pair("dn", name);
            };
            if let Some(announce_list) = &self.announce_list {
                for tracker in announce_list {
                    query.append_pair("tr", tracker.as_str());
                }
            }
            query.finish();
        }

        write!(f, "{}", url)
    }
}

impl FromStr for MagnetLink {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url = reqwest::Url::from_str(s)?;
        anyhow::ensure!(url.scheme() == "magnet");
        let mut info_hash = None;
        let mut name = None;
        let mut trackers = Vec::new();
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                // info_hash
                "xt" => {
                    let mut split = value.splitn(3, ':');
                    let urn = split.next().context("urn string is not found in xt")?;
                    let hash_indicator =
                        split.next().context("hash indicator is not found in xt")?;
                    anyhow::ensure!(urn == "urn");
                    anyhow::ensure!(hash_indicator == "btih");
                    let hash = split.next().context("hash is not found in xt")?;
                    anyhow::ensure!(hash.len() == 40);
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
            info_hash: info_hash.context("magnet link does not contain info_hash")?,
            name,
            announce_list: trackers,
        })
    }
}

impl MagnetLink {
    pub fn hash(&self) -> [u8; 20] {
        hex::decode(&self.info_hash).unwrap().try_into().unwrap()
    }
    pub fn all_trackers(&self) -> Option<Vec<Url>> {
        self.announce_list.clone()
    }
}

#[cfg(test)]
mod tests {

    use super::MagnetLink;

    use std::str::FromStr;

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
        let magnet_link = MagnetLink::from_str(contents).unwrap();
        let magnet_link_copy = magnet_link.clone();
        assert_eq!(magnet_link.info_hash, expected_info_hash);
        assert_eq!(magnet_link.name.unwrap(), expected_name);
        let announce_list = magnet_link.announce_list.unwrap();
        assert_eq!(announce_list.len(), expected_trackers.len());
        for (actual_url, expected_url) in announce_list.iter().zip(expected_trackers) {
            assert_eq!(actual_url.to_string(), expected_url);
        }
        assert_eq!(contents, magnet_link_copy.to_string())
    }
}
