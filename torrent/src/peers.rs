use std::{fmt::Display, net::SocketAddr, time::Duration};

use anyhow::{anyhow, ensure, Context};
use bytes::BytesMut;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::mpsc,
};
use tokio_stream::StreamExt;
use tokio_util::{
    codec::{Encoder, Framed},
    sync::CancellationToken,
};
use uuid::Uuid;

use crate::{
    download::{Block, PeerCommand, PeerStatus, PeerStatusMessage},
    protocol::{
        peer::{ExtensionHandshake, HandShake, MessageFramer, PeerMessage, CLIENT_EXTENSIONS},
        pex::PexMessage,
        ut_metadata::{UtMessage, UtMetadata},
        Info,
    },
};

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
            let my_handshake = ExtensionHandshake::my_handshake();
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
            let my_handshake = ExtensionHandshake::my_handshake();
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
            extension_handshake: his_extension_handshake,
        })
    }

    pub async fn new_from_ip(ip: SocketAddr, info_hash: [u8; 20]) -> anyhow::Result<Self> {
        let socket = TcpStream::connect(ip).await?;
        tracing::info!("Connected peer: {:?}", ip);
        Self::new(socket, info_hash).await
    }

    pub fn supports_ut_metadata(&self) -> bool {
        self.extension_handshake
            .as_ref()
            .and_then(|handshake| UtMetadata::empty_from_handshake(&handshake))
            .is_some()
    }

    pub async fn fetch_ut_metadata(&mut self) -> anyhow::Result<Info> {
        let handshake = self
            .extension_handshake
            .as_ref()
            .context("peer does not support extensions")?;
        let mut ut_metadata = UtMetadata::empty_from_handshake(handshake)
            .ok_or(anyhow!("peer does not support ut_metadata"))?;
        while let Some(msg) = ut_metadata.request_next_block() {
            self.send_peer_msg(PeerMessage::Extension {
                extension_id: ut_metadata.metadata_id,
                payload: msg.as_bytes().into(),
            })
            .await?;
            loop {
                let response = self.stream.next().await.context("stream to be open")??;
                let PeerMessage::Extension {
                    extension_id,
                    payload,
                } = response
                else {
                    continue;
                };
                if extension_id != 1 {
                    continue;
                }
                let message: UtMessage = serde_bencode::from_bytes(&payload)?;
                let message_length = serde_bencode::to_bytes(&message).unwrap().len();
                match message {
                    UtMessage::Request { piece } => {
                        tracing::warn!("Ignoring ut metadata request piece: {piece}");
                    }
                    UtMessage::Data { piece, total_size } => {
                        ensure!(total_size == ut_metadata.size);
                        let data_slice = payload.slice(message_length..);
                        ut_metadata.save_block(piece, data_slice).unwrap();
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

    pub async fn show_interest(&mut self) -> Result<(), PeerError> {
        self.send_peer_msg(PeerMessage::Interested).await?;
        self.interested = true;
        Ok(())
    }

    pub async fn download(
        mut self,
        mut ipc: PeerIPC,
        cancellation_token: CancellationToken,
    ) -> (Uuid, Result<(), PeerError>) {
        let mut afk_interval = tokio::time::interval(Duration::from_secs(1));
        let mut pex_update_interval = tokio::time::interval(Duration::from_secs(90));
        afk_interval.tick().await;
        pex_update_interval.tick().await;
        let peer_result = loop {
            tokio::select! {
                Some(command_msg) = ipc.commands_rx.recv() => {
                    afk_interval.reset();
                    match self.handle_peer_command(command_msg).await {
                        Ok(_) => {},
                        Err(e) => break Err(e),
                    }
                },
                Some(Ok(peer_msg)) = self.stream.next() => {
                    if let PeerMessage::Piece { .. } = peer_msg {
                        afk_interval.reset();
                    }
                    if let Err(e) = self.handle_peer_msg(peer_msg, &mut ipc).await {
                        break Err(e);
                    }
                },
                _ = afk_interval.tick() => {
                    let _ = self.send_status(PeerStatusMessage::Afk, &mut ipc).await;
                }
                _ = pex_update_interval.tick() => {
                    let _ = self.send_status(PeerStatusMessage::PexRequest, &mut ipc).await;
                }
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

    pub async fn handle_peer_command(
        &mut self,
        peer_command: PeerCommand,
    ) -> Result<(), PeerError> {
        match peer_command {
            PeerCommand::Start { block } => {
                if !self.choked {
                    self.send_peer_msg(PeerMessage::request(block)).await?;
                } else {
                    self.send_peer_msg(PeerMessage::request(block)).await?;
                    tracing::warn!("ignoring new job (choked)");
                }
            }
            // Cancel does not provide guarentee that this block will not arrive
            PeerCommand::Cancel { block } => {
                self.send_peer_msg(PeerMessage::Cancel {
                    index: block.piece,
                    begin: block.offset,
                    length: block.length,
                })
                .await?;
            }
            PeerCommand::Have { piece } => {
                let have_msg = PeerMessage::Have { index: piece };
                self.send_peer_msg(have_msg).await?;
            }
            PeerCommand::Interested => self.show_interest().await?,
            PeerCommand::Choke => self.send_peer_msg(PeerMessage::Choke).await?,
            PeerCommand::Unchoke => self.send_peer_msg(PeerMessage::Unchoke).await?,
            PeerCommand::NotInterested => self.send_peer_msg(PeerMessage::NotInterested).await?,
            PeerCommand::Block { block, data } => {
                self.send_peer_msg(PeerMessage::Piece {
                    index: block.piece,
                    begin: block.offset,
                    block: data,
                })
                .await?
            }
            PeerCommand::Pex { msg } => {
                if let Some(pex_id) = self.extension_handshake.as_ref().and_then(|h| h.pex_id()) {
                    self.send_peer_msg(PeerMessage::Extension {
                        extension_id: pex_id,
                        payload: msg.as_bytes().into(),
                    })
                    .await?;
                };
            }
            PeerCommand::UtMetadata { msg, data } => {
                if let Some(ut_metadata_id) = self
                    .extension_handshake
                    .as_ref()
                    .and_then(|h| h.ut_metadata_id())
                {
                    let msg_bytes = msg.as_bytes();
                    let mut bytes = bytes::BytesMut::with_capacity(msg_bytes.len() + data.len());
                    bytes.extend_from_slice(&msg_bytes);
                    bytes.extend_from_slice(&data);

                    self.send_peer_msg(PeerMessage::Extension {
                        extension_id: ut_metadata_id,
                        payload: bytes.freeze(),
                    })
                    .await?;
                };
            }
        };
        Ok(())
    }

    pub async fn handle_peer_msg(
        &mut self,
        peer_msg: PeerMessage,
        ipc: &mut PeerIPC,
    ) -> Result<(), PeerError> {
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
                self.send_status(
                    PeerStatusMessage::Request {
                        block: Block {
                            piece: index,
                            offset: begin,
                            length,
                        },
                    },
                    ipc,
                )
                .await;
            }
            PeerMessage::Piece {
                index,
                begin,
                block: bytes,
            } => {
                let block = Block {
                    piece: index,
                    offset: begin,
                    length: bytes.len() as u32,
                };
                self.send_status(PeerStatusMessage::Data { block, bytes }, ipc)
                    .await;
            }
            PeerMessage::Cancel { .. } => {
                tracing::warn!(%peer_msg, "Not implemented")
            }
            PeerMessage::ExtensionHandshake { .. } => {
                tracing::warn!(%peer_msg, "Not implemented")
            }
            PeerMessage::Extension {
                extension_id,
                payload,
            } => {
                if let Some(name) = CLIENT_EXTENSIONS
                    .iter()
                    .find(|(_, id)| *id == extension_id)
                    .map(|(name, _)| *name)
                {
                    match name {
                        "ut_metadata" => match UtMessage::from_bytes(&payload) {
                            Ok(msg) => match msg {
                                UtMessage::Request { piece } => {
                                    tracing::debug!("Peer asked for ut_metadata block({})", piece);
                                    self.send_status(
                                        PeerStatusMessage::UtMetadataBlockRequest { block: piece },
                                        ipc,
                                    )
                                    .await
                                }
                                UtMessage::Data { .. } => {
                                    tracing::warn!("Peer sent ut_metadata data message");
                                }
                                UtMessage::Reject { piece } => {
                                    tracing::warn!(
                                        "Peer sent ut_metadata reject message for block: {}",
                                        piece
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::error!("Failed to decode ut_metadata message: {e}");
                                return Err(PeerError::logic(
                                    "Failed to decode ut_metadata message",
                                ));
                            }
                        },
                        "ut_pex" => match PexMessage::from_bytes(&payload) {
                            Ok(msg) => {
                                self.send_status(PeerStatusMessage::PexMessage { msg }, ipc)
                                    .await;
                            }
                            Err(e) => {
                                tracing::error!("Failed to decode pex message: {e}");
                                return Err(PeerError::logic("Failed to decode pex message"));
                            }
                        },
                        _ => return Err(PeerError::logic("Unrecognized extension")),
                    }
                };
            }
        }
        Ok(())
    }

    pub async fn send_peer_msg(&mut self, peer_msg: PeerMessage) -> Result<(), PeerError> {
        let mut framer = MessageFramer;
        let mut buf = BytesMut::new();
        let msg_description = peer_msg.to_string();
        framer
            .encode(peer_msg, &mut buf)
            .expect("our own message to encode");
        let socket = self.stream.get_mut();
        match tokio::time::timeout(Duration::from_secs(2), socket.write_all(&buf)).await {
            Ok(Ok(_)) => Ok(()),
            Err(_) => {
                tracing::error!("Peer write timed out");
                Err(PeerError::timeout(
                    "failed to send message to peer (Timeout)",
                ))
            }
            Ok(Err(e)) => {
                tracing::error!(
                    "Peer connection error while sending {msg_description} message: {e}"
                );
                Err(PeerError::connection("peer connection failed"))
            }
        }
    }

    pub async fn send_status(&mut self, status: PeerStatusMessage, ipc: &mut PeerIPC) {
        ipc.status_tx
            .send(PeerStatus {
                peer_id: self.uuid,
                message_type: status,
            })
            .await
            .unwrap();
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
                let piece_i = i * (8 as usize) + (position as usize);
                let mask = 1u8.rotate_right(position + 1);
                (byte & mask != 0).then_some(piece_i)
            })
        })
    }

    pub fn empty(pieces_amount: usize) -> Self {
        Self(vec![0; std::cmp::max((pieces_amount + 8 - 1) / 8, 1)])
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
    fn parse_extension_handshake() {
        let data = b"d1:md11:LT_metadatai1e6:qT_PEXi2ee1:pi6881e1:v13:\xc2\xb5Torreet 1.2e";
        let extenstion_handshake: ExtensionHandshake = serde_bencode::from_bytes(data).unwrap();
        let back = serde_bencode::to_string(&extenstion_handshake).unwrap();
        assert_eq!(*extenstion_handshake.dict.get("LT_metadata").unwrap(), 1);
        assert_eq!(*extenstion_handshake.dict.get("qT_PEX").unwrap(), 2);
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
