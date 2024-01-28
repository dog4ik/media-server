use std::{
    ops::Deref,
    path::{Path, PathBuf},
};

use serde::{de::Visitor, Deserialize, Serialize};
use sha1::{Digest, Sha1};

use crate::tracker::{AnnouncePayload, AnnounceResult};

#[derive(Debug, Deserialize, Serialize)]
pub struct File {
    pub path: Vec<String>,
    pub length: u64,
}

#[derive(Debug, Deserialize, Serialize)]
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

#[derive(Debug, Deserialize, Serialize)]
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

    pub fn all_files(&self) -> Vec<OutputFile> {
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
