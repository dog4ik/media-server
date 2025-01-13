use bytes::Bytes;
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize};

use super::{
    extension::Extension,
    peer::{ExtensionHandshake, CLIENT_EXTENSIONS},
    Info,
};

#[derive(Debug, Clone)]
pub struct UtMetadata {
    pub size: usize,
    pub metadata_id: u8,
    pub blocks: Vec<Option<Bytes>>,
    pub downloaded: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum UtMessage {
    Request { piece: usize },
    Data { piece: usize, total_size: usize },
    Reject { piece: usize },
}

impl UtMessage {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_bencode::Error> {
        serde_bencode::from_bytes(bytes)
    }
    pub fn as_bytes(&self) -> Vec<u8> {
        serde_bencode::to_bytes(self).expect("serialization is infallible")
    }
}

struct UtMessageVisitor;

impl<'v> Visitor<'v> for UtMessageVisitor {
    type Value = UtMessage;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "bencoded map")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'v>,
    {
        let mut msg_type: Option<u8> = None;
        let mut piece: Option<usize> = None;
        let mut total_size: Option<usize> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_ref() {
                "msg_type" => msg_type = Some(map.next_value()?),
                "piece" => piece = Some(map.next_value()?),
                "total_size" => total_size = Some(map.next_value()?),
                _ => {
                    return Err(serde::de::Error::unknown_variant(
                        &key,
                        &["msg_type", "piece", "total_size"],
                    ))
                }
            };
        }
        let msg_type = msg_type.ok_or(serde::de::Error::missing_field("msg_type"))?;
        let piece = piece.ok_or(serde::de::Error::missing_field("piece"))?;
        match msg_type {
            0 => Ok(UtMessage::Request { piece }),
            1 => Ok(UtMessage::Data {
                piece,
                total_size: total_size.ok_or(serde::de::Error::missing_field("total_size"))?,
            }),
            2 => Ok(UtMessage::Reject { piece }),
            rest => Err(serde::de::Error::custom(format!(
                "unknown msg_type: {rest}"
            ))),
        }
    }
}

impl<'de> Deserialize<'de> for UtMessage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_map(UtMessageVisitor)
    }
}

impl Serialize for UtMessage {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let length_hint = match self {
            UtMessage::Request { .. } => 2,
            UtMessage::Data { .. } => 3,
            UtMessage::Reject { .. } => 2,
        };
        let mut map = serializer.serialize_map(Some(length_hint))?;

        match self {
            UtMessage::Request { piece } => {
                map.serialize_entry("msg_type", &0)?;
                map.serialize_entry("piece", piece)?;
            }
            UtMessage::Data { piece, total_size } => {
                map.serialize_entry("msg_type", &1)?;
                map.serialize_entry("piece", piece)?;
                map.serialize_entry("total_size", total_size)?;
            }
            UtMessage::Reject { piece } => {
                map.serialize_entry("msg_type", &2)?;
                map.serialize_entry("piece", piece)?;
            }
        };
        map.end()
    }
}

impl UtMetadata {
    const BLOCK_SIZE: usize = 1024 * 16;

    pub fn empty_from_handshake(handshake: &ExtensionHandshake) -> Option<Self> {
        let metadata_id = handshake.ut_metadata_id()?;
        let size = handshake.ut_metadata_size()?;
        let total_pieces = size.div_ceil(Self::BLOCK_SIZE);
        Some(Self {
            size,
            metadata_id,
            blocks: vec![None; total_pieces],
            downloaded: 0,
        })
    }

    /// Create metadata from existing Info
    pub fn full_from_info(info: &Info) -> Self {
        let bytes = Bytes::copy_from_slice(&serde_bencode::to_bytes(info).unwrap());
        let metadata_id = CLIENT_EXTENSIONS[0].1;
        let size = bytes.len();
        let total_pieces = size.div_ceil(Self::BLOCK_SIZE);
        let mut blocks = Vec::with_capacity(total_pieces);
        for i in 0..total_pieces - 1 {
            let start = i * Self::BLOCK_SIZE;
            let end = start + Self::BLOCK_SIZE;
            blocks.push(Some(bytes.slice(start..end)));
        }
        let last_start = (total_pieces - 1) * Self::BLOCK_SIZE;
        blocks.push(Some(bytes.slice(last_start..size)));

        debug_assert_eq!(
            &bytes[..],
            &blocks.iter().fold(Vec::<u8>::new(), |mut acc, n| {
                let n = n.as_ref().expect("ut metadata is full");
                acc.extend(n);
                acc
            })[..]
        );

        Self {
            size,
            metadata_id,
            downloaded: blocks.len(),
            blocks,
        }
    }

    pub fn piece_len(&self, piece_i: usize) -> usize {
        if piece_i == self.blocks.len() - 1 {
            self.size - piece_i * Self::BLOCK_SIZE
        } else {
            Self::BLOCK_SIZE
        }
    }

    pub fn as_bytes(self) -> Bytes {
        let iter = self.blocks.into_iter().flat_map(|x| x.unwrap());
        Bytes::from_iter(iter)
    }

    pub fn request_next_block(&mut self) -> Option<UtMessage> {
        let piece = self.blocks.iter().position(Option::is_none)?;
        Some(UtMessage::Request { piece })
    }

    pub fn save_block(&mut self, piece: usize, data: Bytes) -> Option<()> {
        let block = self.blocks.get_mut(piece)?;
        if block.is_none() {
            *block = Some(data);
            self.downloaded += 1;
        }
        Some(())
    }

    pub fn get_piece(&self, piece: usize) -> Option<Bytes> {
        self.blocks.get(piece).cloned()?
    }
}

impl From<UtMessage> for bytes::Bytes {
    fn from(value: UtMessage) -> Self {
        value.as_bytes().into()
    }
}

impl TryFrom<&[u8]> for UtMessage {
    type Error = anyhow::Error;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        let ut_message = Self::from_bytes(value)?;
        Ok(ut_message)
    }
}

impl Extension<'_> for UtMessage {
    const CLIENT_ID: u8 = 1;
    const NAME: &'static str = "ut_metadata";
}
