use std::{fmt::Display, net::SocketAddr, time::Duration};

use anyhow::{anyhow, ensure, Context};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_stream::StreamExt;
use tokio_util::{codec::Framed, sync::CancellationToken};
use uuid::Uuid;

use crate::protocol::{
    extension::Extension,
    peer::{ExtensionHandshake, HandShake, MessageFramer, PeerMessage},
    ut_metadata::{UtMessage, UtMetadata},
    Info,
};

#[derive(Debug)]
pub struct PeerIPC {
    pub message_tx: flume::Sender<PeerMessage>,
    pub message_rx: flume::Receiver<PeerMessage>,
}

#[derive(Debug, Clone)]
pub struct PeerError {
    pub msg: String,
    pub error_type: PeerErrorCause,
}

impl std::error::Error for PeerError {}

impl From<anyhow::Error> for PeerError {
    fn from(err: anyhow::Error) -> Self {
        Self {
            msg: err.to_string(),
            error_type: PeerErrorCause::Unhandled,
        }
    }
}

impl Display for PeerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let error_type = match self.error_type {
            PeerErrorCause::Timeout => "Timeout",
            PeerErrorCause::Connection => "Connection",
            PeerErrorCause::PeerLogic => "Peer logic",
            PeerErrorCause::Unhandled => "Unhandled",
        };
        write!(f, "{error_type} with message: {}", self.msg)
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
}

#[derive(Debug, Clone)]
pub struct PeerLogicError(String);

impl Display for PeerLogicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "logic error: {}", self.0)
    }
}
impl std::error::Error for PeerLogicError {}

impl From<PeerLogicError> for PeerError {
    fn from(value: PeerLogicError) -> Self {
        Self {
            msg: value.0,
            error_type: PeerErrorCause::PeerLogic,
        }
    }
}

impl From<anyhow::Error> for PeerLogicError {
    fn from(err: anyhow::Error) -> Self {
        Self(err.to_string())
    }
}

#[derive(Debug)]
pub struct Peer {
    pub uuid: Uuid,
    pub peer_ip: SocketAddr,
    pub stream: Framed<TcpStream, MessageFramer>,
    pub bitfield: BitField,
    pub handshake: HandShake,
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
            .context("receive peer handshake")?;
        let his_handshake = HandShake::from_bytes(&handshake_response)?;
        ensure!(his_handshake.info_hash == info_hash);

        let mut messages_stream = Framed::new(socket, MessageFramer);
        let first_message = messages_stream
            .next()
            .await
            .context("peer to send bitfield/extension handshake")?
            .context("bitfield/extension handshake")?;

        let (bitfield, his_extension_handshake) = if his_handshake.supports_extensions() {
            let payload = ExtensionHandshake::my_handshake();
            let message = PeerMessage::ExtensionHandshake { payload };
            let socket = messages_stream.get_mut();
            message
                .write_to(socket)
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
                            "Second message must be the extension message if first is bitfield, got {second_message}"
                        ));
                    };
                    (bitfield, Some(extension))
                }
                PeerMessage::ExtensionHandshake { payload: extension } => {
                    let PeerMessage::Bitfield { payload: bitfield } = second_message else {
                        return Err(anyhow!(
                            "Second message must be the bitfield message if first is extension, got {second_message}"
                        ));
                    };
                    (bitfield, Some(extension))
                }
                _ => {
                    return Err(anyhow!(
                    "First 2 messages must be bitfield or extension handshake, got {first_message}"
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
            .context("receive peer handshake")?;
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
            let payload = ExtensionHandshake::my_handshake();
            let message = PeerMessage::ExtensionHandshake { payload };
            let socket = messages_stream.get_mut();
            message
                .write_to(socket)
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
                            "Second message must be the extension message if first is bitfield, got {second_message}"
                        ));
                    };
                    (bitfield, Some(extension))
                }
                PeerMessage::ExtensionHandshake { payload: extension } => {
                    let PeerMessage::Bitfield { payload: bitfield } = second_message else {
                        return Err(anyhow!(
                            "Second message must be the bitfield message if first is extension, got {second_message}"
                        ));
                    };
                    (bitfield, Some(extension))
                }
                _ => {
                    return Err(anyhow!(
                    "First 2 messages must be bitfield or extension handshake, got {first_message}"
                ))
                }
            }
        } else {
            let PeerMessage::Bitfield { payload: bitfield } = first_message else {
                return Err(anyhow!(
                    "First message must be the bitfield, got {first_message}"
                ));
            };
            (bitfield, None)
        };

        Ok(Self {
            uuid: Uuid::new_v4(),
            peer_ip,
            bitfield,
            stream: messages_stream,
            handshake: his_handshake,
            extension_handshake: his_extension_handshake,
        })
    }

    pub async fn new_from_ip(ip: SocketAddr, info_hash: [u8; 20]) -> anyhow::Result<Self> {
        let socket = TcpStream::connect(ip).await?;
        let peer = Self::new(socket, info_hash).await?;
        let client_name = peer
            .extension_handshake
            .as_ref()
            .and_then(|h| h.client_name());
        tracing::trace!(%ip, ?client_name, "Connected peer");
        Ok(peer)
    }

    pub async fn fetch_ut_metadata(&mut self) -> anyhow::Result<Info> {
        let handshake = self
            .extension_handshake
            .as_ref()
            .context("peer does not support extensions")?;
        let mut ut_metadata = UtMetadata::empty_from_handshake(handshake)
            .context("peer does not support ut_metadata")?;
        while let Some(msg) = ut_metadata.request_next_block() {
            self.send_peer_msg(PeerMessage::Extension {
                extension_id: ut_metadata.metadata_id,
                payload: msg.as_bytes().into(),
            })
            .await?;
            loop {
                let response = self.stream.next().await.context("stream to be open")??;
                let PeerMessage::Extension {
                    extension_id: UtMessage::CLIENT_ID,
                    payload,
                } = response
                else {
                    continue;
                };
                let message: UtMessage = serde_bencode::from_bytes(&payload)?;
                match message {
                    UtMessage::Request { piece } => {
                        tracing::warn!("Ignoring ut metadata request piece: {piece}");
                        // reject it
                    }
                    UtMessage::Data { piece, total_size } => {
                        ensure!(total_size == ut_metadata.size);
                        let piece_len = ut_metadata.piece_len(piece);
                        let data_slice = payload.slice(payload.len() - piece_len..);
                        ut_metadata
                            .save_block(piece, data_slice)
                            .context("peer send block that does not exist")?;
                        break;
                    }
                    UtMessage::Reject { piece } => {
                        return Err(anyhow!("peer rejected piece {piece}"));
                    }
                }
            }
        }

        Ok(serde_bencode::from_bytes(&ut_metadata.as_bytes())?)
    }

    pub async fn download(
        mut self,
        ipc: PeerIPC,
        cancellation_token: CancellationToken,
    ) -> (Uuid, Result<(), PeerError>) {
        let peer_result = loop {
            tokio::select! {
                Ok(command_msg) = ipc.message_rx.recv_async() => {
                    match self.send_peer_msg(command_msg).await {
                        Ok(_) => {},
                        Err(e) => break Err(e),
                    }
                },
                Some(Ok(peer_msg)) = self.stream.next() => {
                    if let Err(_) = ipc.message_tx.try_send(peer_msg) {
                        tracing::error!("Peer channel is closed or overflowed");
                        break Err(PeerError::timeout("Channel is closed or overflowed"));
                    };
                },
                _ = cancellation_token.cancelled() => {
                    tracing::debug!(ip = self.ip().to_string(), "Peer quit using cancellation token");
                    break Ok(());
                }
            };
        };
        let mut stream = self.stream.into_inner();
        let _ = stream.shutdown().await;
        (self.uuid, peer_result)
    }

    pub async fn send_peer_msg(&mut self, peer_msg: PeerMessage) -> Result<(), PeerError> {
        let msg_description = peer_msg.to_string();
        let socket = self.stream.get_mut();
        match tokio::time::timeout(Duration::from_secs(2), peer_msg.write_to(socket)).await {
            Ok(Ok(_)) => Ok(()),
            Err(_) => {
                tracing::debug!("Peer write timed out");
                Err(PeerError::timeout(
                    "failed to send message to peer (Timeout)",
                ))
            }
            Ok(Err(e)) => {
                tracing::debug!(
                    "Peer connection error while sending {msg_description} message: {e}"
                );
                Err(PeerError::connection("peer connection failed"))
            }
        }
    }

    pub fn ip(&self) -> SocketAddr {
        self.peer_ip
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BitField(pub Vec<u8>);

impl BitField {
    pub fn new(data: &[u8]) -> Self {
        Self(data.to_vec())
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
        let bytes = &mut self.0;
        let Some(block) = bytes.get_mut(piece / 8) else {
            return Err(anyhow!("piece {piece} does not exist"));
        };
        let position = (piece % 8) as u32;
        let new_value = *block | 1u8.rotate_right(position + 1);
        *block = new_value;
        Ok(())
    }

    pub fn all_pieces(&self, total_pieces: usize) -> impl IntoIterator<Item = bool> + '_ {
        self.0.iter().enumerate().flat_map(move |(i, byte)| {
            (0..8).filter_map(move |position| {
                let piece_i = i * 8 + (position as usize);
                if piece_i > total_pieces {
                    None
                } else {
                    let mask = 1u8.rotate_right(position + 1);
                    Some(byte & mask != 0)
                }
            })
        })
    }

    pub fn is_full(&self, max_pieces: usize) -> bool {
        if self.0.is_empty() {
            return true;
        }
        let mut pieces = 0;
        for byte in &self.0[..self.0.len() - 1] {
            if *byte != u8::MAX {
                return false;
            }
            pieces += byte.count_ones();
        }
        let last = self.0.last().unwrap();
        pieces += last.count_ones();
        pieces as usize == max_pieces
    }

    pub fn remove(&mut self, piece: usize) -> anyhow::Result<()> {
        let bytes = &mut self.0;
        let Some(block) = bytes.get_mut(piece / 8) else {
            return Err(anyhow!("piece {piece} does not exist"));
        };
        let position = (piece % 8) as u32;
        let new_value = *block & !1u8.rotate_right(position + 1);
        *block = new_value;
        Ok(())
    }

    pub fn pieces(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(i, byte)| {
            (0..8).filter_map(move |position| {
                let piece_i = i * 8 + (position as usize);
                let mask = 1u8.rotate_right(position + 1);
                (byte & mask != 0).then_some(piece_i)
            })
        })
    }

    pub fn missing_pieces(&self, total_pieces: usize) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(move |(i, byte)| {
            (0..8).filter_map(move |position| {
                let piece_i = i * 8 + (position as usize);
                if piece_i >= total_pieces {
                    return None;
                }
                let mask = 1u8.rotate_right(position + 1);
                (byte & mask == 0).then_some(piece_i)
            })
        })
    }

    pub fn empty(pieces_amount: usize) -> Self {
        Self(vec![0; std::cmp::max(pieces_amount.div_ceil(8), 1)])
    }

    /// Make sure that bitield is appropriate for given pieces amount.
    /// Fails if there are any 1's after the end or it is small or large to fit given pieces.
    pub fn validate(&self, total_pieces: usize) -> anyhow::Result<()> {
        let bitfield_pieces = self.0.len() * 8;
        let leftover = bitfield_pieces
            .checked_sub(total_pieces)
            .context("bitfield has less capacity than needed")?;
        if leftover >= 8 {
            anyhow::bail!("bitfield is larger than needed")
        }
        for piece in (bitfield_pieces - leftover)..bitfield_pieces {
            anyhow::ensure!(!self.has(piece));
        }
        Ok(())
    }

    /// Perform bitwise | with other
    pub fn or(&mut self, other: &Self) {
        for (self_byte, other_byte) in self.0.iter_mut().zip(other.0.iter()) {
            *self_byte |= other_byte;
        }
    }
}

impl From<Vec<u8>> for BitField {
    fn from(value: Vec<u8>) -> Self {
        BitField(value)
    }
}

#[cfg(test)]
mod test {

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
    fn bitfield_remove() {
        let data = [0b01110101, 0b01110001];
        let mut bitfield = BitField::new(&data);
        bitfield.remove(1).unwrap();
        bitfield.remove(4).unwrap();
        bitfield.remove(9).unwrap();
        bitfield.remove(15).unwrap();
        assert!(!bitfield.has(0));
        assert!(!bitfield.has(1));
        assert!(bitfield.has(2));
        assert!(bitfield.has(3));
        assert!(!bitfield.has(4));
        assert!(bitfield.has(5));
        assert!(!bitfield.has(6));
        assert!(bitfield.has(7));
        assert!(!bitfield.has(8));
        assert!(!bitfield.has(9));
        assert!(bitfield.has(10));
        assert!(bitfield.has(11));
        assert!(!bitfield.has(12));
        assert!(!bitfield.has(13));
        assert!(!bitfield.has(14));
        assert!(!bitfield.has(15));
        assert!(!bitfield.has(16));
        assert!(!bitfield.has(17));
        assert!(bitfield.remove(16).is_err());
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
    fn bitfiled_validate() {
        let data = [0b01110101, 0b01110001, 0b00100000];
        let bitfield = BitField::new(&data);
        assert!(bitfield.validate(16).is_err());
        assert!(bitfield.validate(1).is_err());
        assert!(bitfield.validate(13).is_err());
        assert!(bitfield.validate(18).is_err());
        assert!(bitfield.validate(19).is_ok());
        assert!(bitfield.validate(20).is_ok());
        assert!(bitfield.validate(24).is_ok());
        assert!(bitfield.validate(25).is_err());
        assert!(bitfield.validate(100).is_err());
        let data = [0b01110100];
        let bitfield = BitField::new(&data);
        assert!(bitfield.validate(1).is_err());
        assert!(bitfield.validate(4).is_err());
        assert!(bitfield.validate(5).is_err());
        assert!(bitfield.validate(6).is_ok());
        assert!(bitfield.validate(7).is_ok());
        assert!(bitfield.validate(8).is_ok());
        assert!(bitfield.validate(9).is_err());
        assert!(bitfield.validate(100).is_err());
        let data = [0b11111111, 0b00000000];
        let bitfield = BitField::new(&data);
        assert!(bitfield.validate(1).is_err());
        assert!(bitfield.validate(4).is_err());
        assert!(bitfield.validate(5).is_err());
        assert!(bitfield.validate(6).is_err());
        assert!(bitfield.validate(7).is_err());
        assert!(bitfield.validate(8).is_err());
        assert!(bitfield.validate(9).is_ok());
        assert!(bitfield.validate(100).is_err());
    }

    #[test]
    fn parse_extension_handshake() {
        let data = b"d1:md11:LT_metadatai1e6:qT_PEXi2ee1:pi6881e1:v13:\xc2\xb5Torreet 1.2e";
        let extension_handshake: ExtensionHandshake = serde_bencode::from_bytes(data).unwrap();
        let back = serde_bencode::to_string(&extension_handshake).unwrap();
        assert_eq!(*extension_handshake.dict.get("LT_metadata").unwrap(), 1);
        assert_eq!(*extension_handshake.dict.get("qT_PEX").unwrap(), 2);
        assert_eq!(std::str::from_utf8(data).unwrap(), back);
    }

    #[test]
    fn parse_ut_metadata_message() {
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
}
