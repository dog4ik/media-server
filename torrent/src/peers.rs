use std::{
    collections::HashMap,
    fmt::Display,
    io::{BufRead, Read, Write},
    net::{SocketAddr, SocketAddrV4},
    time::Duration,
};

use anyhow::{anyhow, ensure, Context};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{de::Visitor, ser::SerializeMap, Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, oneshot},
    time::Instant,
};
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, Encoder, Framed};
use uuid::Uuid;

use crate::download::{Block, PeerCommand, PeerStatus, PeerStatusMessage};

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
                write!(f, "Extension with id {extension_id}")
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
            reader.read_exact(&mut index_buffer).context("index buffer")?;
            reader.read_exact(&mut begin_buffer).context("begin buffer")?;
            reader.read_exact(&mut length_buffer).context("length buffer")?;
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
                let (index, length, begin) = request_payload(payload)?;
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
                let (index, length, begin) = request_payload(payload)?;
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
        let mut length_bytes = [0u8; 4];
        length_bytes.copy_from_slice(&src[..4]);
        let length = u32::from_be_bytes(length_bytes) as usize;

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


#[derive(Debug)]
pub struct PeerIPC {
    pub status_tx: mpsc::Sender<PeerStatus>,
    pub commands_rx: mpsc::Receiver<PeerCommand>,
}

#[derive(Debug, Clone)]
pub struct PeerError {
    pub msg: String,
    pub error_type: PeerErrorCause,
}

impl<E> From<E> for PeerError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self {
            msg: err.into().to_string(),
            error_type: PeerErrorCause::Unhandled,
        }
    }
}

#[derive(Debug, Clone)]
pub enum PeerErrorCause {
    Timeout,
    Connection,
    PeerLogic,
    Unhandled,
}

impl PeerError {
    pub fn new(cause: PeerErrorCause, msg: &str) -> Self {
        Self {
            error_type: cause,
            msg: msg.into(),
        }
    }

    pub fn timeout(msg: &str) -> Self {
        Self::new(PeerErrorCause::Timeout, msg)
    }

    pub fn connection(msg: &str) -> Self {
        Self::new(PeerErrorCause::Connection, msg)
    }

    pub fn logic(msg: &str) -> Self {
        Self::new(PeerErrorCause::PeerLogic, msg)
    }

    pub fn unhandled(msg: &str) -> Self {
        Self::new(PeerErrorCause::Unhandled, msg)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionHandshake {
    #[serde(rename = "m")]
    pub dict: HashMap<String, u8>,
    #[serde(flatten)]
    pub fields: HashMap<String, serde_bencode::value::Value>,
}

impl ExtensionHandshake {
    pub fn from_bytes(bytes: &[u8]) -> serde_bencode::Result<Self> {
        serde_bencode::from_bytes(bytes)
    }

    pub fn as_bytes(&self) -> Bytes {
        serde_bencode::to_bytes(self).unwrap().into()
    }

    pub fn new() -> Self {
        let mut dict = HashMap::new();
        let fields = HashMap::new();
        dict.insert("ut_metadata".into(), 1);

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

    pub fn client_name(&self) -> Option<String> {
        let serde_bencode::value::Value::Bytes(bytes) = self.fields.get("v")? else {
            return None;
        };
        String::from_utf8(bytes.to_vec()).ok()
    }
}

#[derive(Debug, Clone)]
pub struct UtMetadata {
    size: usize,
    peer_id: u8,
    blocks: Vec<Option<Bytes>>,
}

#[derive(Debug, Clone)]
pub enum UtMessage {
    Request { piece: usize },
    Data { piece: usize, total_size: usize },
    Reject { piece: usize },
}

impl UtMessage {
    pub fn as_bytes(&self) -> Vec<u8> {
        serde_bencode::to_bytes(self).unwrap()
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
        let peer_id = *handshake.dict.get("ut_metadata")?;
        let size = handshake.ut_metadata_size()?;
        let total_pieces = (size + Self::BLOCK_SIZE - 1) / Self::BLOCK_SIZE;
        Some(Self {
            size,
            peer_id,
            blocks: vec![None; total_pieces],
        })
    }

    pub fn full_from_info(message_id: u8, info: crate::file::Info) -> Self {
        let bytes = Bytes::copy_from_slice(&serde_bencode::to_bytes(&info).unwrap());
        let size = bytes.len();
        let total_pieces = (size + Self::BLOCK_SIZE - 1) / Self::BLOCK_SIZE;
        let mut blocks = Vec::with_capacity(total_pieces);
        for i in 0..total_pieces - 1 {
            let start = i * Self::BLOCK_SIZE;
            let end = start + Self::BLOCK_SIZE;
            blocks[i] = Some(bytes.slice(start..end));
        }
        let last_start = total_pieces - 1 * Self::BLOCK_SIZE;
        let last_length = crate::utils::piece_size(total_pieces - 1, Self::BLOCK_SIZE, size);
        let last_end = last_start + last_length;
        let last_block = blocks.last_mut().unwrap();
        *last_block = Some(bytes.slice(last_start..last_end));

        Self {
            size,
            peer_id: message_id,
            blocks,
        }
    }

    pub fn as_bytes(self) -> Bytes {
        let iter = self.blocks.into_iter().map(|x| x.unwrap()).flatten();
        Bytes::from_iter(iter)
    }

    pub fn is_full(&self) -> bool {
        self.blocks.iter().all(Option::is_some)
    }

    pub fn request_next_block(&mut self) -> Option<UtMessage> {
        let piece = self.blocks.iter().position(Option::is_none)?;
        Some(UtMessage::Request { piece })
    }

    pub fn save_block(&mut self, piece: usize, data: Bytes) -> Option<()> {
        let block = self.blocks.get_mut(piece)?;
        *block = Some(data);
        Some(())
    }

    pub fn handle_request(&self, request: Bytes) -> anyhow::Result<Bytes> {
        let message: UtMessage = serde_bencode::from_bytes(&request)?;
        let UtMessage::Request { piece } = message else {
            return Err(anyhow!("expected request message, got {:?}", message));
        };
        if !self.is_full() {
            return Ok(Self::rejection(piece));
        }
        let data_message = UtMessage::Data {
            piece,
            total_size: self.size,
        }
        .as_bytes();
        let data = self
            .blocks
            .get(piece)
            .ok_or(anyhow!("requested piece({piece}) is missing"))?
            .clone()
            .expect("full metadata");
        let mut bytes = BytesMut::with_capacity(data_message.len() + data.len());
        bytes.extend_from_slice(&data_message);
        bytes.extend_from_slice(&data);
        Ok(bytes.into())
    }

    pub fn rejection(piece: usize) -> Bytes {
        let mut dict: HashMap<&str, usize> = HashMap::new();
        dict.insert("msg_type", 2);
        dict.insert("piece", piece);
        serde_bencode::to_bytes(&dict).unwrap().into()
    }
}

#[derive(Debug)]
pub struct Peer {
    pub uuid: Uuid,
    pub peer_ip: SocketAddr,
    pub stream: Framed<TcpStream, MessageFramer>,
    pub bitfield: BitField,
    pub handshake: HandShake,
    pub choked: bool,
    pub interested: bool,
    pub last_alive: Instant,
    pub extension_handshake: Option<ExtensionHandshake>,
}

impl Peer {
    /// Connect to peer and perform the handshake
    pub async fn new(mut socket: TcpStream, info_hash: [u8; 20]) -> anyhow::Result<Self> {
        let my_handshake = HandShake::new(info_hash).as_bytes();
        let peer_ip = socket.peer_addr().context("get peer ip addr")?;
        socket
            .write_all(&my_handshake)
            .await
            .context("send my handshake")?;
        let mut handshake_response = [0_u8; HandShake::SIZE];
        socket
            .read_exact(&mut handshake_response)
            .await
            .context("recieve peer handshake")?;
        let his_handshake = HandShake::from_bytes(&handshake_response)?;
        ensure!(his_handshake.info_hash == info_hash);

        let mut messages_stream = Framed::new(socket, MessageFramer);
        let first_message = messages_stream
            .next()
            .await
            .context("peer to send bitfield/extension handshake")?
            .context("bitfield/extension handshake")?;

        let (bitfield, his_extension_handshake) = if his_handshake.supports_extensions() {
            tracing::debug!("Peer supports extensions");
            let my_handshake = ExtensionHandshake::new();
            let mut framer = MessageFramer;
            let mut my_handshake_bytes = BytesMut::new();
            framer.encode(
                PeerMessage::ExtensionHandshake {
                    payload: my_handshake,
                },
                &mut my_handshake_bytes,
            )?;
            let socket = messages_stream.get_mut();
            socket
                .write_all(&my_handshake_bytes)
                .await
                .context("write my extension handshake")?;

            let second_message = messages_stream
                .next()
                .await
                .context("peer to send 2 messages")?
                .context("second message")?;
            match first_message {
                PeerMessage::Bitfield { payload: bitfield } => {
                    let PeerMessage::ExtensionHandshake { payload: extension } = second_message
                    else {
                        return Err(anyhow!(
                            "Second message must be the extension message if first is bitfield"
                        ));
                    };
                    (bitfield, Some(extension))
                }
                PeerMessage::ExtensionHandshake { payload: extension } => {
                    let PeerMessage::Bitfield { payload: bitfield } = second_message else {
                        return Err(anyhow!(
                            "Second message must be the bitfield message if first is extension"
                        ));
                    };
                    (bitfield, Some(extension))
                }
                _ => {
                    return Err(anyhow!(
                        "First 2 messages must be bitfield or extension handshake"
                    ))
                }
            }
        } else {
            let PeerMessage::Bitfield { payload: bitfield } = first_message else {
                return Err(anyhow!("First message must be the bitfield"));
            };
            (bitfield, None)
        };

        Ok(Self {
            uuid: Uuid::new_v4(),
            peer_ip,
            bitfield,
            stream: messages_stream,
            handshake: his_handshake,
            choked: true,
            interested: false,
            last_alive: Instant::now(),
            extension_handshake: his_extension_handshake,
        })
    }

    /// Create new peer without knowing its info_hash. Mimics peer's handshake info_hash
    pub async fn new_without_info_hash(mut socket: TcpStream) -> anyhow::Result<Self> {
        let peer_ip = socket.peer_addr().context("get peer ip addr")?;
        let mut handshake_response = [0_u8; HandShake::SIZE];
        socket
            .read_exact(&mut handshake_response)
            .await
            .context("recieve peer handshake")?;
        let his_handshake = HandShake::from_bytes(&handshake_response)?;

        let my_handshake = HandShake::new(his_handshake.info_hash).as_bytes();
        socket
            .write_all(&my_handshake)
            .await
            .context("send my handshake")?;

        let mut messages_stream = Framed::new(socket, MessageFramer);
        let first_message = messages_stream
            .next()
            .await
            .context("peer to send bitfield/extension handshake")?
            .context("bitfield/extension handshake")?;

        let (bitfield, his_extension_handshake) = if his_handshake.supports_extensions() {
            tracing::debug!("Peer supports extensions");
            let my_handshake = ExtensionHandshake::new();
            let mut framer = MessageFramer;
            let mut my_handshake_bytes = BytesMut::new();
            framer.encode(
                PeerMessage::ExtensionHandshake {
                    payload: my_handshake,
                },
                &mut my_handshake_bytes,
            )?;
            let socket = messages_stream.get_mut();
            socket
                .write_all(&my_handshake_bytes)
                .await
                .context("write my extension handshake")?;

            let second_message = messages_stream
                .next()
                .await
                .context("peer to send 2 messages")?
                .context("second message")?;
            match first_message {
                PeerMessage::Bitfield { payload: bitfield } => {
                    let PeerMessage::ExtensionHandshake { payload: extension } = second_message
                    else {
                        return Err(anyhow!(
                            "Second message must be the extension message if first is bitfield"
                        ));
                    };
                    (bitfield, Some(extension))
                }
                PeerMessage::ExtensionHandshake { payload: extension } => {
                    let PeerMessage::Bitfield { payload: bitfield } = second_message else {
                        return Err(anyhow!(
                            "Second message must be the bitfield message if first is extension"
                        ));
                    };
                    (bitfield, Some(extension))
                }
                _ => {
                    return Err(anyhow!(
                        "First 2 messages must be bitfield or extension handshake"
                    ))
                }
            }
        } else {
            let PeerMessage::Bitfield { payload: bitfield } = first_message else {
                return Err(anyhow!("First message must be the bitfield"));
            };
            (bitfield, None)
        };

        Ok(Self {
            uuid: Uuid::new_v4(),
            peer_ip,
            bitfield,
            stream: messages_stream,
            handshake: his_handshake,
            choked: true,
            interested: false,
            last_alive: Instant::now(),
            extension_handshake: his_extension_handshake,
        })
    }

    pub async fn new_from_ip(ip: SocketAddrV4, info_hash: [u8; 20]) -> anyhow::Result<Self> {
        let socket = TcpStream::connect(ip).await?;
        Self::new(socket, info_hash).await
    }

    pub async fn fetch_ut_metadata(&mut self) -> anyhow::Result<crate::file::Info> {
        let handshake = self
            .extension_handshake
            .as_ref()
            .ok_or(anyhow!("peer does not support extensions"))?;
        let mut ut_metadata = UtMetadata::empty_from_handshake(handshake)
            .ok_or(anyhow!("peer does not support ut_metadata"))?;
        while let Some(block) = ut_metadata.request_next_block() {
            self.send_peer_msg(PeerMessage::Extension {
                extension_id: ut_metadata.peer_id,
                payload: block.as_bytes().into(),
            })
            .await
            .unwrap();
            let response = self
                .stream
                .next()
                .await
                .expect("stream to be open")
                .unwrap();
            let PeerMessage::Extension {
                extension_id,
                payload,
            } = response
            else {
                continue;
            };
            ensure!(extension_id == 1);
            let message: UtMessage = serde_bencode::from_bytes(&payload)?;
            let message_length = serde_bencode::to_bytes(&message).unwrap().len();
            match message {
                UtMessage::Request { piece } => todo!(),
                UtMessage::Data { piece, total_size } => {
                    ensure!(total_size == ut_metadata.size);
                    let data_slice = payload.slice(message_length..);
                    ut_metadata.save_block(piece, data_slice).unwrap();
                }
                UtMessage::Reject { piece } => {
                    return Err(anyhow!("peer rejected piece {piece}"));
                }
            }
        }

        Ok(serde_bencode::from_bytes(&ut_metadata.as_bytes())?)
    }

    pub async fn show_interest(&mut self) -> Result<(), PeerError> {
        let mut msg_bytes = BytesMut::with_capacity(2);
        let mut msg_framer = MessageFramer;
        let interested_message = PeerMessage::Interested;
        let socket = self.stream.get_mut();
        msg_framer
            .encode(interested_message, &mut msg_bytes)
            .unwrap();
        socket
            .write_all(&msg_bytes)
            .await
            .context("send interested")
            .map_err(|_| PeerError::connection("failed to send intereseted"))?;
        self.interested = true;
        Ok(())
    }

    pub fn close(self) {}

    pub async fn download(mut self, mut ipc: PeerIPC) -> (Uuid, Result<(), PeerError>) {
        let mut afk_interval = tokio::time::interval(Duration::from_secs(10));
        afk_interval.tick().await;
        loop {
            tokio::select! {
                Some(command_msg) = ipc.commands_rx.recv() => {
                    afk_interval.reset();
                    match self.handle_peer_command(command_msg).await {
                        Ok(should_break) => if should_break { break; },
                        Err(e) => return (self.uuid, Err(e)),
                    }
                },
                Some(Ok(peer_msg)) = self.stream.next() => {
                    if let PeerMessage::Piece { .. } = peer_msg {
                        afk_interval.reset();
                    }
                    if let Err(e) = self.handle_peer_msg(peer_msg, &mut ipc).await {
                        return (self.uuid, Err(e));
                    }
                },
                _ = afk_interval.tick() => {
                    let _ = self.send_status(PeerStatusMessage::Afk, &mut ipc).await;
                }
                else => break
            };
        }
        println!("PEER EXIT");
        (self.uuid, Ok(()))
    }

    pub async fn handle_peer_command(
        &mut self,
        peer_command: PeerCommand,
    ) -> Result<bool, PeerError> {
        match peer_command {
            PeerCommand::Start { block } => {
                self.send_peer_msg(PeerMessage::request(block)).await?;
            }
            PeerCommand::Have { piece } => {
                let have_msg = PeerMessage::Have { index: piece };
                self.send_peer_msg(have_msg).await?;
            }
            PeerCommand::Abort => return Ok(true),
            PeerCommand::Interested => self.show_interest().await?,
            PeerCommand::Choke => self.send_peer_msg(PeerMessage::Choke).await?,
            PeerCommand::Unchoke => self.send_peer_msg(PeerMessage::Unchoke).await?,
            PeerCommand::NotInterested => self.send_peer_msg(PeerMessage::NotInterested).await?,
        };
        Ok(false)
    }

    pub async fn handle_peer_msg(
        &mut self,
        peer_msg: PeerMessage,
        ipc: &mut PeerIPC,
    ) -> Result<(), PeerError> {
        tracing::trace!(%self.uuid, "Peer sent {} message", peer_msg);
        match peer_msg {
            PeerMessage::HeatBeat => {}
            PeerMessage::Choke => {
                self.choked = true;
                self.send_status(PeerStatusMessage::Choked, ipc).await;
            }
            PeerMessage::Unchoke => {
                self.choked = false;
                self.send_status(PeerStatusMessage::Unchoked, ipc).await;
            }
            PeerMessage::Interested => {
                tracing::warn!(%peer_msg, "Not implemented")
            }
            PeerMessage::NotInterested => {
                tracing::warn!(%peer_msg, "Not implemented")
            }
            PeerMessage::Have { index } => {
                let _ = self.bitfield.add(index as usize);
                self.send_status(PeerStatusMessage::Have { piece: index }, ipc)
                    .await;
            }
            PeerMessage::Bitfield { .. } => {
                return Err(PeerError::logic("Peer is sending bitfield"));
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                println!("peer requesting block, it will block peer exectution");
                let (tx, rx) = oneshot::channel();
                self.send_status(
                    PeerStatusMessage::Request {
                        response: tx,
                        block: Block {
                            piece: index,
                            offset: begin,
                            length,
                        },
                    },
                    ipc,
                )
                .await;
                if let Ok(Some(bytes)) = rx.await {
                    let _ = self
                        .send_peer_msg(PeerMessage::Piece {
                            index,
                            begin,
                            block: bytes.slice(begin as usize..=length as usize),
                        })
                        .await;
                }
            }
            PeerMessage::Piece {
                index,
                begin,
                block,
            } => {
                tracing::trace!(
                    "Got piece {} with offset {} with size({})",
                    index,
                    begin,
                    block.len()
                );
                self.send_status(
                    PeerStatusMessage::Data {
                        block: Block {
                            piece: index,
                            offset: begin,
                            length: block.len() as u32,
                        },
                        bytes: block,
                    },
                    ipc,
                )
                .await;
            }
            PeerMessage::Cancel { .. } => {
                tracing::warn!(%peer_msg, "Not implemented")
            }
            PeerMessage::ExtensionHandshake { .. } => {
                tracing::warn!(%peer_msg, "Not implemented")
            }
            PeerMessage::Extension { .. } => {
                tracing::warn!(%peer_msg, "Not implemented")
            }
        }
        Ok(())
    }

    pub async fn send_peer_msg(&mut self, peer_msg: PeerMessage) -> Result<(), PeerError> {
        let mut framer = MessageFramer;
        let mut buf = BytesMut::new();
        framer
            .encode(peer_msg, &mut buf)
            .expect("our own message to encode");
        let socket = self.stream.get_mut();
        socket
            .write_all(&buf)
            .await
            .map_err(|_| PeerError::connection("failed to send message to peer"))?;
        Ok(())
    }

    pub async fn send_status(&mut self, status: PeerStatusMessage, ipc: &mut PeerIPC) {
        tracing::debug!("Sending status: {}", status);
        ipc.status_tx
            .try_send(PeerStatus {
                peer_id: self.uuid,
                message_type: status,
            })
            .unwrap();
    }

    pub fn ip(&self) -> SocketAddrV4 {
        match self.peer_ip {
            SocketAddr::V4(ip) => ip,
            SocketAddr::V6(_) => unimplemented!("ipv6"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitField(pub Bytes);

impl BitField {
    pub fn new(data: &[u8]) -> Self {
        Self(Bytes::copy_from_slice(data))
    }

    pub fn has(&self, piece: usize) -> bool {
        let bytes = &self.0;
        let Some(block) = bytes.get(piece / 8) else {
            return false;
        };
        let position = (piece % 8) as u32;

        block & 1u8.rotate_right(position + 1) != 0
    }

    pub fn add(&mut self, piece: usize) -> anyhow::Result<()> {
        let mut bytes = BytesMut::with_capacity(self.0.len());
        bytes.extend_from_slice(&self.0);
        let Some(block) = bytes.get_mut(piece / 8) else {
            return Err(anyhow!("piece {piece} does not exist"));
        };
        let position = (piece % 8) as u32;
        let new_value = *block | 1u8.rotate_right(position + 1);
        *block = new_value;
        self.0 = bytes.into();
        Ok(())
    }

    pub fn pieces(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(i, byte)| {
            (0..8).filter_map(move |position| {
                let piece_i = i * (8 as usize) + (position as usize);
                let mask = 1u8.rotate_right(position + 1);
                (byte & mask != 0).then_some(piece_i)
            })
        })
    }

    pub fn empty(pieces_amount: usize) -> Self {
        let mut bytes = vec![0; std::cmp::max((pieces_amount + 8 - 1) / 8, 1)];
        bytes.fill(0);
        Self(bytes.into())
    }
}

#[derive(Debug, Clone)]
pub struct HandShake {
    reserved: [u8; 8],
    info_hash: [u8; 20],
    peer_id: [u8; 20],
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

#[cfg(test)]
mod test {
    use std::{str::FromStr, time::Duration};

    use crate::{
        file::MagnetLink,
        peers::{Peer, PeerIPC},
        tracker::AnnouncePayload,
    };

    use super::{BitField, ExtensionHandshake, UtMessage};

    #[test]
    fn bitfield_has() {
        let data = [0b01110101, 0b01110001];
        let bitfield = BitField::new(&data);
        assert!(!bitfield.has(0));
        assert!(bitfield.has(1));
        assert!(bitfield.has(2));
        assert!(bitfield.has(3));
        assert!(!bitfield.has(4));
        assert!(bitfield.has(5));
        assert!(!bitfield.has(6));
        assert!(bitfield.has(7));
        assert!(!bitfield.has(8));
        assert!(bitfield.has(9));
        assert!(bitfield.has(10));
        assert!(bitfield.has(11));
        assert!(!bitfield.has(12));
        assert!(!bitfield.has(13));
        assert!(!bitfield.has(14));
        assert!(bitfield.has(15));
        assert!(!bitfield.has(16));
        assert!(!bitfield.has(17));
    }
    #[test]
    fn bitfield_add() {
        let data = [0b01110101, 0b01110001];
        let mut bitfield = BitField::new(&data);
        bitfield.add(0).unwrap();
        bitfield.add(1).unwrap();
        bitfield.add(4).unwrap();
        bitfield.add(8).unwrap();
        bitfield.add(14).unwrap();
        assert!(bitfield.has(0));
        assert!(bitfield.has(1));
        assert!(bitfield.has(2));
        assert!(bitfield.has(3));
        assert!(bitfield.has(4));
        assert!(bitfield.has(5));
        assert!(!bitfield.has(6));
        assert!(bitfield.has(7));
        assert!(bitfield.has(8));
        assert!(bitfield.has(9));
        assert!(bitfield.has(10));
        assert!(bitfield.has(11));
        assert!(!bitfield.has(12));
        assert!(!bitfield.has(13));
        assert!(bitfield.has(14));
        assert!(bitfield.has(15));
        assert!(!bitfield.has(16));
        assert!(!bitfield.has(17));
        assert!(bitfield.add(16).is_err());
    }

    #[test]
    fn bitfield_iterator() {
        let data = [0b01110101, 0b01110001];
        let bitfield = BitField::new(&data);
        let mut iterator = bitfield.pieces();
        assert_eq!(Some(1), iterator.next());
        assert_eq!(Some(2), iterator.next());
        assert_eq!(Some(3), iterator.next());
        assert_eq!(Some(5), iterator.next());
        assert_eq!(Some(7), iterator.next());
        assert_eq!(Some(9), iterator.next());
        assert_eq!(Some(10), iterator.next());
        assert_eq!(Some(11), iterator.next());
        assert_eq!(Some(15), iterator.next());
        assert_eq!(None, iterator.next());
    }

    #[test]
    fn parse_extension_handshake() {
        let data = b"d1:md11:LT_metadatai1e6:qT_PEXi2ee1:pi6881e1:v13:\xc2\xb5Torreet 1.2e";
        let extenstion_handshake: ExtensionHandshake = serde_bencode::from_bytes(data).unwrap();
        let back = serde_bencode::to_string(&extenstion_handshake).unwrap();
        dbg!(&extenstion_handshake, &back);
        assert_eq!(*extenstion_handshake.dict.get("LT_metadata").unwrap(), 1);
        assert_eq!(*extenstion_handshake.dict.get("qT_PEX").unwrap(), 2);
        assert_eq!(std::str::from_utf8(data).unwrap(), back);
    }

    #[test]
    fn ut_metadata_message() {
        // {'msg_type': 0, 'piece': 0}
        let request = b"d8:msg_typei2e5:piecei0ee";
        // {'msg_type': 9, 'piece': 0}
        let unsupported_request = b"d8:msg_typei9e5:piecei0ee";
        // {'msg_type': 1, 'piece': 0}
        let data_request = b"d8:msg_typei1e5:piecei0e10:total_sizei34256eexxxxxxxx";

        let message: UtMessage = serde_bencode::from_bytes(request).unwrap();
        let data_message: UtMessage = serde_bencode::from_bytes(data_request).unwrap();
        let unsupported_message = serde_bencode::from_bytes::<UtMessage>(unsupported_request);
        assert!(unsupported_message.is_err());
        assert_eq!(
            serde_bencode::to_string(&message).unwrap(),
            String::from_utf8(request.to_vec()).unwrap()
        );
        assert!(String::from_utf8(data_request.to_vec())
            .unwrap()
            .starts_with(&serde_bencode::to_string(&data_message).unwrap()));
    }

    #[tokio::test]
    async fn ut_metadata_fetch() {
        use std::fs;
        use tokio::net::TcpStream;
        use tokio::sync::mpsc;

        let content = fs::read_to_string("torrents/hazbinhotel.magnet").unwrap();
        let magnet_link = MagnetLink::from_str(&content).unwrap();
        let info_hash = magnet_link.hash();
        let announce = AnnouncePayload::from_magnet_link(magnet_link).unwrap();

        let announce = announce.announce().await.unwrap();
        let (status_tx, status_rx) = mpsc::channel(100);
        for peer_ip in announce.peers {
            let Ok(Ok(socket)) =
                tokio::time::timeout(Duration::from_millis(400), TcpStream::connect(peer_ip)).await
            else {
                continue;
            };
            let (_, commands_rx) = mpsc::channel(100);
            let ipc = PeerIPC {
                status_tx: status_tx.clone(),
                commands_rx,
            };
            let mut peer = Peer::new(socket, info_hash).await.unwrap();
            let info = peer.fetch_ut_metadata().await.unwrap();
            assert_eq!(info_hash, info.hash());
            dbg!(&info.name);
            dbg!(&info.file_descriptor);
            dbg!(&info.piece_length);
            break;
        }
    }
}
