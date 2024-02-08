use std::{
    fmt::Display,
    io::{BufRead, Read, Write},
    net::SocketAddr,
};

use anyhow::{anyhow, ensure, Context};
use bytes::{Buf, BufMut, Bytes, BytesMut};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{mpsc, oneshot},
    time::Instant,
};
use tokio_stream::StreamExt;
use tokio_util::codec::{Decoder, Encoder, Framed};
use uuid::Uuid;

use crate::download::{Block, MessageType, PeerCommand, PeerStatus};

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
        }
    }
}

impl PeerMessage {
    pub fn from_slice(data_bytes: &[u8]) -> anyhow::Result<Self> {
        if data_bytes.len() == 0 {
            return Ok(Self::HeatBeat);
        }
        let request_payload = |b: &[u8]| -> anyhow::Result<_> {
            let mut index_buffer = [0; 4];
            let mut begin_buffer = [0; 4];
            let mut length_buffer = [0; 4];
            let mut reader = b.reader();
            reader.read(&mut index_buffer).context("index buffer")?;
            reader.read(&mut begin_buffer).context("begin buffer")?;
            reader.read(&mut length_buffer).context("length buffer")?;
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
            PeerMessage::HeatBeat => return Bytes::from_static(&[0]),
            PeerMessage::Choke => return Bytes::from_static(&[0]),
            PeerMessage::Unchoke => return Bytes::from_static(&[1]),
            PeerMessage::Interested => return Bytes::from_static(&[2]),
            PeerMessage::NotInterested => return Bytes::from_static(&[3]),
            PeerMessage::Have { index } => {
                let mut bytes = BytesMut::with_capacity(5);
                bytes.extend_from_slice(&[4]);
                bytes.extend_from_slice(&index.to_be_bytes());
                return bytes.into();
            }
            PeerMessage::Bitfield { payload } => {
                let mut bytes = BytesMut::with_capacity(1 + payload.0.len());
                bytes.extend_from_slice(&[4]);
                bytes.extend_from_slice(&payload.0);
                return bytes.into();
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                let request = request_to_bytes(*index, *begin, *length);
                let mut bytes = BytesMut::with_capacity(request.len() + 1);
                bytes.extend_from_slice(&[6]);
                bytes.extend_from_slice(&request);
                bytes.into()
            }
            PeerMessage::Piece {
                index,
                begin,
                block: piece,
            } => {
                let mut bytes = BytesMut::with_capacity(8 + 1 + piece.len());
                bytes.extend_from_slice(&6_u32.to_be_bytes());
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
                bytes.extend_from_slice(&8_u32.to_be_bytes());
                bytes.extend_from_slice(&request);
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

const CONNECTION_TIMEOUT_MS: u64 = 350;
const BLOCK_SIZE: u64 = 1 << 14;

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

#[derive(Debug)]
pub struct Peer {
    pub uuid: Uuid,
    pub peer_ip: SocketAddr,
    pub stream: Framed<TcpStream, MessageFramer>,
    pub bitfield: BitField,
    pub handshake: HandShake,
    pub choked: bool,
    pub interested: bool,
    pub download_rate: isize,
    pub last_alive: Instant,
    pub ipc: PeerIPC,
}

impl Peer {
    /// Connect to peer and perform the handshake
    pub async fn new(
        mut socket: TcpStream,
        info_hash: [u8; 20],
        ipc: PeerIPC,
    ) -> anyhow::Result<Self> {
        let my_handshake = HandShake {
            peer_id: rand::random(),
            info_hash,
            reserved: [0_u8; 8],
        }
        .as_bytes();
        let peer_ip = socket.peer_addr().context("get peer ip addr")?;
        let mut handshake_response = [0_u8; 68];
        socket
            .write_all(&my_handshake)
            .await
            .context("send my handshake")?;
        socket
            .read(&mut handshake_response)
            .await
            .context("recieve peer handshake")?;

        let his_handshake = HandShake::from_bytes(&handshake_response)?;
        ensure!(his_handshake.info_hash == info_hash);
        let mut messages_stream = Framed::new(socket, MessageFramer);
        let bitfield = messages_stream
            .next()
            .await
            .expect("peer to send bitfield")
            .context("bitfield")?;
        let PeerMessage::Bitfield { payload: bitfield } = bitfield else {
            return Err(anyhow!("First meessage must be the bitfield"));
        };

        Ok(Self {
            uuid: Uuid::new_v4(),
            peer_ip,
            bitfield,
            stream: messages_stream,
            handshake: his_handshake,
            choked: true,
            interested: false,
            download_rate: 0,
            last_alive: Instant::now(),
            ipc,
        })
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

    pub async fn close(&mut self) -> anyhow::Result<()> {
        todo!()
    }

    pub async fn download(mut self) -> (Uuid, Result<(), PeerError>) {
        loop {
            tokio::select! {
                Some(command_msg) = self.ipc.commands_rx.recv() => {
                    match self.handle_peer_command(command_msg).await {
                        Ok(should_break) => if should_break { break; },
                        Err(e) => return (self.uuid, Err(e)),
                    }
                },
                Some(Ok(peer_msg)) = self.stream.next() => {
                    if let Err(e) = self.handle_peer_msg(peer_msg).await {
                        return (self.uuid, Err(e));
                    }
                    self.last_alive = Instant::now();
                },
            };
        }
        (self.uuid, Ok(()))
    }

    pub async fn handle_peer_command(
        &mut self,
        peer_command: PeerCommand,
    ) -> Result<bool, PeerError> {
        match peer_command {
            PeerCommand::Start { block } => {
                if !self.interested {
                    self.show_interest().await?;
                }
                self.send_peer_msg(PeerMessage::request(block)).await?;
            }
            PeerCommand::Have { piece } => {
                let have_msg = PeerMessage::Have { index: piece };
                self.send_peer_msg(have_msg).await?;
            }
            PeerCommand::Abort => return Ok(true),
            PeerCommand::Interested => self.show_interest().await?,
        };
        Ok(false)
    }

    pub async fn handle_peer_msg(&mut self, peer_msg: PeerMessage) -> Result<(), PeerError> {
        tracing::debug!("Peer sent {} message", peer_msg);
        match peer_msg {
            PeerMessage::HeatBeat => {}
            PeerMessage::Choke => {
                self.choked = true;
                let _ = self
                    .ipc
                    .status_tx
                    .send(PeerStatus {
                        peer_id: self.uuid,
                        message_type: MessageType::Choked,
                    })
                    .await;
            }
            PeerMessage::Unchoke => {
                self.choked = false;
                let _ = self
                    .ipc
                    .status_tx
                    .send(PeerStatus {
                        peer_id: self.uuid,
                        message_type: MessageType::Unchoked,
                    })
                    .await;
            }
            PeerMessage::Interested => todo!(),
            PeerMessage::NotInterested => todo!(),
            PeerMessage::Have { index } => {
                let _ = self.bitfield.add(index as usize);
                self.send_status(MessageType::Have { piece: index }).await?;
            }
            PeerMessage::Bitfield { .. } => {
                return Err(PeerError::logic("Peer is sending bitfield"));
            }
            PeerMessage::Request {
                index,
                begin,
                length,
            } => {
                let (tx, rx) = oneshot::channel();
                self.ipc
                    .status_tx
                    .send(PeerStatus {
                        peer_id: self.uuid,
                        message_type: MessageType::Request {
                            response: tx,
                            block: Block {
                                piece: index,
                                offset: begin,
                                length,
                            },
                        },
                    })
                    .await
                    .unwrap();
                if let Ok(Some(bytes)) = rx.await {
                    let _ = self
                        .send_peer_msg(PeerMessage::Piece {
                            index,
                            begin,
                            block: bytes,
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
                self.ipc
                    .status_tx
                    .send(PeerStatus {
                        peer_id: self.uuid,
                        message_type: MessageType::Data {
                            block: Block {
                                piece: index,
                                offset: begin,
                                length: block.len() as u32,
                            },
                            bytes: block,
                        },
                    })
                    .await
                    .unwrap();
            }
            PeerMessage::Cancel { .. } => {}
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

    pub async fn send_status(&mut self, status: MessageType) -> anyhow::Result<()> {
        self.ipc
            .status_tx
            .send(PeerStatus {
                peer_id: self.uuid,
                message_type: status,
            })
            .await?;
        Ok(())
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
}

#[cfg(test)]
mod test {

    use super::BitField;

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
}
