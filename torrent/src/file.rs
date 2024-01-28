use std::{
    ops::Deref,
    path::{Path, PathBuf},
};

use serde::{de::Visitor, Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::tracker::{AnnouncePayload, AnnounceResult};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct File {
    pub path: Vec<String>,
    pub length: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SizeDescriptor {
    Files(Vec<File>),
    Length(u64),
}

impl SizeDescriptor {
    pub fn files_amount(&self) -> usize {
        match self {
            SizeDescriptor::Files(files) => files.len(),
            SizeDescriptor::Length(_) => 1,
        }
    }
}

/// Torrent output file that is normalized and safe against path attack
#[derive(Clone, Debug)]
pub struct OutputFile {
    length: u64,
    path: PathBuf,
}

impl OutputFile {
    pub fn new(length: u64, path: PathBuf) -> Self {
        let path = sanitize_path(path);
        Self { length, path }
    }

    pub fn length(&self) -> u64 {
        self.length
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Info {
    /// In the single file case is the name of a file, in the muliple file case, it's the name of a directory.
    pub name: String,
    pub pieces: Hashes,
    #[serde(rename = "piece length")]
    pub piece_length: u64,
    #[serde(flatten)]
    pub file_descriptor: SizeDescriptor,
}

impl Info {
    pub fn total_size(&self) -> u64 {
        match &self.file_descriptor {
            SizeDescriptor::Files(files) => files.iter().map(|f| f.length).sum(),
            SizeDescriptor::Length(length) => *length,
        }
    }

    pub fn output_files(&self, output_dir: impl AsRef<Path>) -> Vec<OutputFile> {
        let base = output_dir.as_ref().join(&self.name);
        match &self.file_descriptor {
            SizeDescriptor::Files(files) => files
                .iter()
                .map(|f| OutputFile {
                    length: f.length,
                    path: PathBuf::from_iter(f.path.iter()),
                })
                .collect(),
            SizeDescriptor::Length(length) => {
                vec![OutputFile {
                    length: *length,
                    path: self.name.clone().into(),
                }]
            }
        }
    }

    pub fn hash(&self) -> [u8; 20] {
        let mut hasher = <Sha1 as sha1::Digest>::new();
        let bytes = serde_bencode::to_bytes(self).unwrap();
        hasher.update(&bytes);
        hasher.finalize().try_into().unwrap()
    }

    pub fn hex_hash(&self) -> String {
        let result = self.hash();
        hex::encode(result)
    }

    pub fn hex_peices_hashes(&self) -> Vec<String> {
        self.pieces.0.iter().map(|x| hex::encode(x)).collect()
    }
}

#[derive(Debug, Clone)]
pub struct Hashes(pub Vec<[u8; 20]>);

impl Hashes {
    pub fn get_hash(&self, piece: usize) -> Option<[u8; 20]> {
        self.0.get(piece).copied()
    }
}

impl Deref for Hashes {
    type Target = Vec<[u8; 20]>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

struct HashesVisitor;

impl Visitor<'_> for HashesVisitor {
    type Value = Hashes;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("Value that length can be divided by 20")
    }

    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v.len() % 20 != 0 {
            return Err(serde::de::Error::custom(
                "payload is not muliple of 20 bytes long",
            ));
        }
        let chunks = v.array_chunks::<20>().cloned().collect();
        Ok(Hashes(chunks))
    }

    fn visit_borrowed_bytes<E>(self, v: &'_ [u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        if v.len() % 20 != 0 {
            return Err(serde::de::Error::custom("payload is not 20 bytes long"));
        }
        let chunks = v.array_chunks::<20>().cloned().collect();
        Ok(Hashes(chunks))
    }
}

impl<'de> Deserialize<'de> for Hashes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(HashesVisitor)
    }
}

impl Serialize for Hashes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(&self.0.concat())
    }
}

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

    pub async fn announce(&self) -> anyhow::Result<AnnounceResult> {
        let announce_payload = AnnouncePayload::from_torrent(self)?;
        announce_payload.announce().await
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

/// Prevent traversal attack on path by ignoring suspicious components
fn sanitize_path(path: PathBuf) -> PathBuf {
    use std::path::Component;
    let mut normalized_path = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) => {
                tracing::warn!("Path starts with prefix component");
            }
            Component::RootDir => {
                tracing::warn!("Path starts with root directory component");
            }
            Component::CurDir | Component::ParentDir => {
                tracing::warn!("Path contains relative directory component");
            }
            Component::Normal(component) => normalized_path.push(component),
        }
    }
    normalized_path
}

#[cfg(test)]
mod tests {
    use crate::file::TorrentFile;

    pub const TORRENTS_LIST: &[&str] = &[
        //UDP announce
        "torrents/yts.torrent",
        //DNT announce
        "torrents/archlinux.torrent",
        //HTTP announce
        "torrents/rutracker.torrent",
        //HTTP announce
        "torrents/thelastgames.torrent",
    ];

    #[test]
    fn info_hash() {
        use std::fs;
        let contents = fs::read(TORRENTS_LIST[0]).unwrap();
        let torrent_file = TorrentFile::from_bytes(&contents).unwrap();
        let hash = torrent_file.info.hex_hash();
        assert_eq!(hash, "b55ad44f0bc643abc1bd17bb3e672ace55e8009f")
    }

    #[test]
    fn piece_hashes() {
        use std::fs;
        let contents = fs::read(TORRENTS_LIST[0]).unwrap();
        let torrent_file = TorrentFile::from_bytes(&contents).unwrap();
        let hashes = torrent_file.info.hex_peices_hashes();
        dbg!(hashes.len(), torrent_file.info.piece_length);
    }

    #[test]
    fn parse_torrent_file() {
        use std::fs;
        let contents = fs::read(TORRENTS_LIST[0]).unwrap();
        let torrent_file = TorrentFile::from_bytes(&contents).unwrap();
        assert_eq!(
            torrent_file.announce,
            "udp://tracker.opentrackr.org:1337/announce"
        );
        dbg!(torrent_file.announce_list);
        dbg!(&torrent_file.info.file_descriptor);
        assert_eq!(torrent_file.info.total_size(), 3144327239);
    }
}
