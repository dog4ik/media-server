use std::{
    ops::Deref,
    path::{Path, PathBuf},
};

use serde::{de::Visitor, Deserialize, Serialize};
use sha1::{Digest, Sha1};

pub mod dht;
pub mod peer;
pub mod tracker;

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
    pub piece_length: u32,
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
                .map(|f| {
                    OutputFile::new(
                        f.length,
                        base.join(sanitize_path(PathBuf::from_iter(f.path.iter()))),
                    )
                })
                .collect(),
            SizeDescriptor::Length(length) => {
                vec![OutputFile::new(*length, base)]
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
