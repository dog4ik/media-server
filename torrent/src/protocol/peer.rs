use std::{
    collections::HashMap,
    fmt::Display,
    io::{BufRead, Read, Write},
};

use anyhow::{anyhow, ensure, Context};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};
use tokio_util::codec::{Decoder, Encoder};

use crate::{download::Block, peers::BitField};

#[derive(Debug, Clone)]
pub struct HandShake {
    pub reserved: [u8; 8],
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
}

impl HandShake {
    pub const SIZE: usize = 68;

    pub fn new(info_hash: [u8; 20]) -> Self {
        let mut reserved = [0_u8; 8];
        // support extensions
        reserved[5] = 0 | 0x10;

        Self {
            info_hash,
            reserved,
            peer_id: rand::random(),
        }
    }

    pub fn supports_extensions(&self) -> bool {
        self.reserved[5] & 0x10 != 0
    }

    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let mut reader = bytes.reader();
        let length = bytes.get(0).ok_or(anyhow!("length byte is not set"))?;
        ensure!(*length == 19);

        let identifier =
            std::str::from_utf8(&bytes[1..20]).context("BitTorrent identifier string")?;
        ensure!(identifier == "BitTorrent protocol");
        reader.consume(20);

        let mut reserved = [0; 8];
        let mut info_hash = [0; 20];
        let mut peer_id = [0; 20];
        reader.read_exact(&mut reserved).context("reserved bytes")?;
        reader.read_exact(&mut info_hash).context("hash bytes")?;
        reader.read_exact(&mut peer_id).context("peer_id bytes")?;

        Ok(Self {
            reserved,
            peer_id,
            info_hash,
        })
    }

    pub fn as_bytes(&self) -> [u8; 68] {
        let mut out = [0_u8; 68];
        let mut writer = out.writer();

        writer.write_all(&[19]).unwrap();
        writer.write(b"BitTorrent protocol").unwrap();
        writer.write(&self.reserved).unwrap();
        writer.write(&self.info_hash).unwrap();
        writer.write(&self.peer_id).unwrap();
        out
    }

    pub fn info_hash(&self) -> [u8; 20] {
        self.info_hash
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionHandshake {
    #[serde(rename = "m")]
    pub dict: HashMap<String, u8>,
    #[serde(flatten)]
    pub fields: HashMap<String, serde_bencode::value::Value>,
}

pub const CLIENT_EXTENSIONS: [(&str, u8); 2] = [("ut_metadata", 1), ("ut_pex", 2)];

impl ExtensionHandshake {
    pub fn from_bytes(bytes: &[u8]) -> serde_bencode::Result<Self> {
        serde_bencode::from_bytes(bytes)
    }

    pub fn as_bytes(&self) -> Bytes {
        serde_bencode::to_bytes(self).unwrap().into()
    }

    pub fn my_handshake() -> Self {
        let mut dict = HashMap::with_capacity(CLIENT_EXTENSIONS.len());
        let fields = HashMap::new();
        for (name, id) in CLIENT_EXTENSIONS {
            dict.insert(name.into(), id);
        }

        Self { dict, fields }
    }

    /// Returns metadata size if it supports ut_metadata
    pub fn ut_metadata_size(&self) -> Option<usize> {
        self.fields
            .get("metadata_size")
            .and_then(|size| match size {
                // WARN: negative value
                serde_bencode::value::Value::Int(size) => Some(*size as usize),
                _ => None,
            })
    }

    /// returns pex's extenison id if handshake supports it
    pub fn pex_id(&self) -> Option<u8> {
        self.dict.get("ut_pex").copied()
    }

    /// returns ut_metadata's extenison id if handshake supports it
    pub fn ut_metadata_id(&self) -> Option<u8> {
        self.dict.get("ut_metadata").copied()
    }

    pub fn client_name(&self) -> Option<String> {
        let serde_bencode::value::Value::Bytes(bytes) = self.fields.get("v")? else {
            return None;
        };
        String::from_utf8(bytes.to_vec()).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerMessage {
    HeatBeat,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have {
        index: u32,
    },
    Bitfield {
        payload: BitField,
    },
    Request {
        index: u32,
        begin: u32,
        length: u32,
    },
    Piece {
        index: u32,
        begin: u32,
        block: Bytes,
    },
    Cancel {
        index: u32,
        begin: u32,
        length: u32,
    },
    ExtensionHandshake {
        payload: ExtensionHandshake,
    },
    Extension {
        extension_id: u8,
        payload: Bytes,
    },
}

impl Display for PeerMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerMessage::HeatBeat => write!(f, "HeatBeat"),
            PeerMessage::Choke => write!(f, "Choke"),
            PeerMessage::Unchoke => write!(f, "Unchoke"),
            PeerMessage::Interested => write!(f, "Interested"),
            PeerMessage::NotInterested => write!(f, "NotInterested"),
            PeerMessage::Have { index } => write!(f, "Have {}", index),
            PeerMessage::Bitfield { payload } => {
                write!(f, "Bitfield with length {}", payload.0.len())
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => write!(
                f,
                "Request for piece {index} with offset {begin} and length {length}"
            ),
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => write!(
                f,
                "Block for piece {index} with offset {begin} and length {}",
                block.len()
            ),
            PeerMessage::Cancel {
                index,
                begin,
                length,
            } => write!(
                f,
                "Cancel for piece {index} with offset {begin} and length {length}",
            ),
            PeerMessage::ExtensionHandshake { .. } => {
                write!(f, "Extension handshake")
            }
            PeerMessage::Extension { extension_id, .. } => {
                let name = CLIENT_EXTENSIONS
                    .iter()
                    .find(|(_, id)| id == extension_id)
                    .map(|(name, _)| *name)
                    .unwrap_or("unknown");
                write!(f, "{name} extension with id {extension_id}")
            }
        }
    }
}

impl PeerMessage {
    pub fn from_slice(data_bytes: &[u8]) -> anyhow::Result<Self> {
        if data_bytes.is_empty() {
            return Ok(Self::HeatBeat);
        }
        let request_payload = |b: &[u8]| -> anyhow::Result<_> {
            let mut index_buffer = [0; 4];
            let mut begin_buffer = [0; 4];
            let mut length_buffer = [0; 4];
            let mut reader = b.reader();
            reader
                .read_exact(&mut index_buffer)
                .context("index buffer")?;
            reader
                .read_exact(&mut begin_buffer)
                .context("begin buffer")?;
            reader
                .read_exact(&mut length_buffer)
                .context("length buffer")?;
            Ok((
                u32::from_be_bytes(index_buffer),
                u32::from_be_bytes(begin_buffer),
                u32::from_be_bytes(length_buffer),
            ))
        };
        let tag = data_bytes[0];
        let payload = &data_bytes[1..];
        match tag {
            0 => Ok(PeerMessage::Choke),
            1 => Ok(PeerMessage::Unchoke),
            2 => Ok(PeerMessage::Interested),
            3 => Ok(PeerMessage::NotInterested),
            4 => {
                let index_buffer = payload[0..4].try_into()?;

                Ok(PeerMessage::Have {
                    index: u32::from_be_bytes(index_buffer),
                })
            }
            5 => {
                let payload = BitField::new(payload);
                Ok(PeerMessage::Bitfield { payload })
            }
            6 => {
                let (index, begin, length) = request_payload(payload)?;
                Ok(PeerMessage::Request {
                    index,
                    length,
                    begin,
                })
            }
            7 => {
                let index_buffer: [u8; 4] = payload[0..4].try_into()?;
                let begin_buffer: [u8; 4] = payload[4..8].try_into()?;
                let index = u32::from_be_bytes(index_buffer);
                let begin = u32::from_be_bytes(begin_buffer);
                let piece = Bytes::copy_from_slice(&payload[8..]);
                Ok(PeerMessage::Piece {
                    index,
                    begin,
                    block: piece,
                })
            }
            8 => {
                let (index, begin, length) = request_payload(payload)?;
                Ok(PeerMessage::Cancel {
                    index,
                    length,
                    begin,
                })
            }
            20 => {
                let extension_id = payload[0];
                if extension_id == 0 {
                    Ok(PeerMessage::ExtensionHandshake {
                        payload: ExtensionHandshake::from_bytes(&payload[1..].as_ref())?,
                    })
                } else {
                    Ok(PeerMessage::Extension {
                        extension_id,
                        payload: Bytes::copy_from_slice(&payload[1..]),
                    })
                }
            }
            t => Err(anyhow!("unsupproted tag: {}", t)),
        }
    }

    pub fn as_bytes(&self) -> Bytes {
        let request_to_bytes = |index: u32, begin: u32, length: u32| {
            let mut bytes = BytesMut::with_capacity(12);
            bytes.extend_from_slice(&index.to_be_bytes());
            bytes.extend_from_slice(&begin.to_be_bytes());
            bytes.extend_from_slice(&length.to_be_bytes());
            bytes
        };
        match self {
            PeerMessage::HeatBeat => Bytes::from_static(&[]),
            PeerMessage::Choke => Bytes::from_static(&[0]),
            PeerMessage::Unchoke => Bytes::from_static(&[1]),
            PeerMessage::Interested => Bytes::from_static(&[2]),
            PeerMessage::NotInterested => Bytes::from_static(&[3]),
            PeerMessage::Have { index } => {
                let mut bytes = BytesMut::with_capacity(5);
                bytes.extend_from_slice(&4_u8.to_be_bytes());
                bytes.extend_from_slice(&index.to_be_bytes());
                bytes.into()
            }
            PeerMessage::Bitfield { payload } => {
                let mut bytes = BytesMut::with_capacity(1 + payload.0.len());
                bytes.extend_from_slice(&5_u8.to_be_bytes());
                bytes.extend_from_slice(&payload.0);
                bytes.into()
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                let request = request_to_bytes(*index, *begin, *length);
                let mut bytes = BytesMut::with_capacity(request.len() + 1);
                bytes.extend_from_slice(&6_u8.to_be_bytes());
                bytes.extend_from_slice(&request);
                bytes.into()
            }
            PeerMessage::Piece {
                index,
                begin,
                block: piece,
            } => {
                let mut bytes = BytesMut::with_capacity(8 + 1 + piece.len());
                bytes.extend_from_slice(&7_u8.to_be_bytes());
                bytes.extend_from_slice(&index.to_be_bytes());
                bytes.extend_from_slice(&begin.to_be_bytes());
                bytes.extend_from_slice(&piece);
                bytes.into()
            }
            PeerMessage::Cancel {
                index,
                begin,
                length,
            } => {
                let request = request_to_bytes(*index, *begin, *length);
                let mut bytes = BytesMut::with_capacity(request.len() + 1);
                bytes.extend_from_slice(&8_u8.to_be_bytes());
                bytes.extend_from_slice(&request);
                bytes.into()
            }
            PeerMessage::ExtensionHandshake { payload } => {
                let payload_bytes = payload.as_bytes();
                let mut bytes = BytesMut::with_capacity(1 + payload_bytes.len());
                bytes.extend_from_slice(&20u8.to_be_bytes());
                bytes.extend_from_slice(&0_u8.to_be_bytes());
                bytes.extend_from_slice(&payload_bytes);
                bytes.into()
            }
            PeerMessage::Extension {
                extension_id,
                payload,
            } => {
                let mut bytes = BytesMut::with_capacity(payload.len() + 2);
                bytes.extend_from_slice(&20u8.to_be_bytes());
                bytes.extend_from_slice(&extension_id.to_be_bytes());
                bytes.extend_from_slice(&payload);
                bytes.into()
            }
        }
    }

    pub fn request(piece: Block) -> Self {
        Self::Request {
            index: piece.piece,
            begin: piece.offset,
            length: piece.length,
        }
    }
    pub fn cancel(piece: Block) -> Self {
        Self::Cancel {
            index: piece.piece,
            begin: piece.offset,
            length: piece.length,
        }
    }
}

#[derive(Debug)]
pub struct MessageFramer;

const MAX: usize = 1 << 16;

impl Decoder for MessageFramer {
    type Item = PeerMessage;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.len() < 4 {
            // Not enough data to read length marker.
            return Ok(None);
        }

        // Read length marker.
        let length = u32::from_be_bytes(src[..4].try_into().unwrap()) as usize;

        // TODO: heartbeat
        // if length == 0 {
        //     return Ok(Some(PeerMessage::HeatBeat));
        // }

        if src.len() < 5 {
            // Not enough data to read tag marker.
            return Ok(None);
        }

        // Check that the length is not too large to avoid a denial of
        // service attack where the server runs out of memory.
        if length > MAX {
            return Err(anyhow!(
                "length({}) is higher then allowed({})",
                length,
                MAX
            ));
        }

        if src.len() < 4 + length {
            // We reserve more space in the buffer. This is not strictly
            // necessary, but is a good idea performance-wise.
            src.reserve(4 + length - src.len());

            // We inform the Framed that we need more bytes to form the next
            // frame.
            return Ok(None);
        }

        // Use advance to modify src such that it no longer contains
        // this frame.
        let data = &src[4..4 + length];
        let message = match PeerMessage::from_slice(&data) {
            Ok(msg) => msg,
            Err(e) => return Err(anyhow!("failed to construct peer message: {}", e)),
        };

        src.advance(4 + length);
        Ok(Some(message))
    }
}

impl Encoder<PeerMessage> for MessageFramer {
    type Error = anyhow::Error;

    fn encode(&mut self, item: PeerMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // Don't send a string if it is longer than the other end will
        // accept.
        let bytes = item.as_bytes();
        let length = bytes.len();
        if length > MAX {
            return Err(anyhow!(
                "length({}) is higher then allowed({})",
                length,
                MAX
            ));
        }

        // Convert the length into a byte array.
        // The cast to u32 cannot overflow due to the length check above.
        let len_slice = u32::to_be_bytes(length as u32);

        // Reserve space in the buffer.
        dst.reserve(4 + length);

        // Write the length and string to the buffer.
        dst.extend_from_slice(&len_slice);
        dst.extend_from_slice(&bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};
    use tokio_util::codec::{Decoder, Encoder};

    use crate::peers::BitField;

    use super::{ExtensionHandshake, MessageFramer, PeerMessage};

    #[test]
    fn parse_peer_message() {
        let mut framer = MessageFramer;
        let mut buffer = BytesMut::new();
        let mut re_encode_message = |msg: PeerMessage| {
            framer.encode(msg.clone(), &mut buffer).unwrap();
            let result = framer.decode(&mut buffer).unwrap().unwrap();
            assert_eq!(msg, result)
        };
        re_encode_message(PeerMessage::Choke);
        re_encode_message(PeerMessage::Unchoke);
        re_encode_message(PeerMessage::Interested);
        re_encode_message(PeerMessage::NotInterested);
        re_encode_message(PeerMessage::Have { index: 123 });
        re_encode_message(PeerMessage::Bitfield {
            payload: BitField::empty(300),
        });
        re_encode_message(PeerMessage::Request {
            index: 22,
            begin: 100,
            length: 200,
        });
        re_encode_message(PeerMessage::Piece {
            index: 22,
            begin: 100,
            block: Bytes::from_static(&[23, 222, 32]),
        });
        re_encode_message(PeerMessage::Cancel {
            index: 22,
            begin: 100,
            length: 200,
        });
        re_encode_message(PeerMessage::ExtensionHandshake {
            payload: ExtensionHandshake::my_handshake(),
        });
        re_encode_message(PeerMessage::Extension {
            extension_id: 1,
            payload: Bytes::from_static(&[22, 222, 32]),
        });
    }
}
