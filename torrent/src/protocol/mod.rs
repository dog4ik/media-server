use std::{
    fmt::Display,
    ops::Deref,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize, de::Visitor};
use sha1::{Digest, Sha1};

#[allow(unused)]
pub mod dht;
pub mod extension;
pub mod peer;
/// Peer Exchange (PEX) BEP 11
///
/// Peer Exchange (PEX) provides an alternative peer discovery mechanism for swarms once peers have bootstrapped via other mechanisms such as DHT or Tracker announces.
/// It provides a more up-to-date view of the swarm than most other sources and also reduces the need to query other sources frequently.
pub mod pex;
pub mod tracker;
/// Extension for Peers to Send Metadata Files BEP 9
///
/// The purpose of this extension is to allow clients to
/// join a swarm and complete a download without the need of downloading a .torrent file first.
/// This extension instead allows clients to download the metadata from peers.
///
/// It makes it possible to support magnet links,
/// a link on a web page only containing enough information to join the swarm (the info hash).
pub mod ut_metadata;

/// Represestation of the single file when [SizeDescriptor] variant is Files
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct File {
    pub length: u64,
    pub path: Vec<String>,
}

impl bendy::encoding::ToBencode for File {
    const MAX_DEPTH: usize = 2;

    fn encode(
        &self,
        encoder: bendy::encoding::SingleItemEncoder,
    ) -> Result<(), bendy::encoding::Error> {
        encoder.emit_dict(|mut e| {
            e.emit_pair(b"length", self.length)?;
            e.emit_pair(b"path", &self.path)
        })?;
        Ok(())
    }
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

/// Info dictionary is a unique descriptor of the particular torrent.
/// Sha1 hash of the info directory is a unique identifier for the torrent.
#[derive(Debug, Clone, Deserialize)]
pub struct Info {
    #[serde(skip)]
    pub raw: bytes::Bytes,
    #[serde(flatten)]
    pub file_descriptor: SizeDescriptor,
    /// In the single file case is the name of a file, in the multiple file case, it's the name of a directory.
    pub name: String,
    #[serde(rename = "piece length")]
    pub piece_length: u32,
    pub pieces: Hashes,
}

impl bendy::decoding::FromBencode for Info {
    fn decode_bencode_object(
        object: bendy::decoding::Object,
    ) -> Result<Self, bendy::decoding::Error> {
        let dict_dec = object.try_into_dictionary()?;
        let raw = bytes::Bytes::copy_from_slice(dict_dec.into_raw()?);

        let mut info: Info = serde_bencode::from_bytes(&raw)?;
        info.raw = raw;
        Ok(info)
    }
}

impl Display for Info {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Name: {}", self.name)?;
        writeln!(
            f,
            "Pieces amount: {}x{} = {} bytes",
            self.pieces.len(),
            self.piece_length,
            self.pieces.len() * self.piece_length as usize
        )?;
        let output_files = self.output_files("");
        writeln!(f, "Files ({}):", output_files.len())?;
        for file in output_files {
            writeln!(f, "   {}: {} bytes", file.path.display(), file.length())?;
        }
        Ok(())
    }
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

    pub fn files_amount(&self) -> usize {
        match &self.file_descriptor {
            SizeDescriptor::Files(f) => f.len(),
            SizeDescriptor::Length(_) => 1,
        }
    }

    pub fn hash(&self) -> [u8; 20] {
        let mut hasher = <Sha1 as sha1::Digest>::new();
        let bytes = self.as_bytes();
        hasher.update(&bytes);
        hasher.finalize().into()
    }

    pub fn hex_hash(&self) -> String {
        let result = self.hash();
        hex::encode(result)
    }

    pub fn hex_pieces_hashes(&self) -> Vec<String> {
        self.pieces.0.iter().map(hex::encode).collect()
    }

    pub fn as_bytes(&self) -> bytes::Bytes {
        self.raw.clone()
    }

    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        bendy::decoding::FromBencode::from_bencode(bytes).map_err(|e| anyhow::anyhow!("{e}"))
    }
}

/// List of piece hashes
#[derive(Debug, Clone)]
pub struct Hashes(pub Arc<[[u8; 20]]>);

impl Hashes {
    pub fn get_hash(&self, piece: usize) -> Option<&[u8; 20]> {
        self.0.get(piece)
    }
}

impl Deref for Hashes {
    type Target = [[u8; 20]];

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
                "payload is not multiple of 20 bytes long",
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
