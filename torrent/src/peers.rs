use std::{
    collections::{BinaryHeap, HashSet},
    fmt::Display,
    net::SocketAddr,
    time::Duration,
};

use anyhow::{anyhow, ensure, Context};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_stream::StreamExt;
use tokio_util::{codec::Framed, sync::CancellationToken};
use uuid::Uuid;

use crate::bitfield::BitField;
use crate::protocol::{
    extension::Extension,
    peer::{ExtensionHandshake, HandShake, MessageFramer, PeerMessage},
    ut_metadata::{UtMessage, UtMetadata},
    Info,
};

const HEARTBEAT: Duration = Duration::from_secs(10);

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
            let socket = messages_stream.get_mut();
            let mut payload = ExtensionHandshake::my_handshake();
            if let Ok(peer_addr) = socket.peer_addr() {
                payload.set_your_ip(peer_addr.ip());
            }
            let message = PeerMessage::ExtensionHandshake { payload };
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
            let mut payload = ExtensionHandshake::my_handshake();
            let socket = messages_stream.get_mut();
            if let Ok(peer_ip) = socket.peer_addr() {
                payload.set_your_ip(peer_ip.ip());
            }
            let message = PeerMessage::ExtensionHandshake { payload };
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
        let reqq = peer
            .extension_handshake
            .as_ref()
            .and_then(|h| h.request_queue_size());
        tracing::trace!(%ip, ?reqq, ?client_name, "Connected peer");
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

        Info::from_bytes(&ut_metadata.as_bytes())
    }

    pub async fn download(
        mut self,
        ipc: PeerIPC,
        cancellation_token: CancellationToken,
    ) -> (Uuid, Result<(), PeerError>) {
        let peer_result = loop {
            tokio::select! {
                _ = tokio::time::sleep(HEARTBEAT) => {
                    if let Err(e) = self.send_peer_msg(PeerMessage::HeartBeat).await {
                        break Err(e);
                    }
                },
                Ok(command_msg) = ipc.message_rx.recv_async() => {
                    if let Err(e) = self.send_peer_msg(command_msg).await {
                        break Err(e);
                    }
                },
                Some(Ok(peer_msg)) = self.stream.next() => {
                    if peer_msg == PeerMessage::HeartBeat {
                        continue;
                    }
                    if let Err(_) = ipc.message_tx.send_async(peer_msg).await {
                        tracing::error!(ip = %self.ip(), "Peer -> scheduler channel is closed");
                        break Err(PeerError::timeout("Channel is closed or overflowed"));
                    };
                },
                _ = cancellation_token.cancelled() => {
                    tracing::debug!(ip = %self.ip(), "Peer quit using cancellation token");
                    break Ok(());
                }
                else => {
                    tracing::debug!(ip = %self.ip(), "Peer tcp stream closed");
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

#[derive(Debug, Clone, Copy)]
pub struct StoredPeer {
    ip: SocketAddr,
    priority: u32,
}

impl StoredPeer {
    pub fn new(ip: SocketAddr, my_ip: SocketAddr) -> Self {
        let priority = crate::protocol::peer::canonical_peer_priority(ip, my_ip);
        Self { ip, priority }
    }
    pub fn new_with_base_priority(ip: SocketAddr) -> Self {
        Self { ip, priority: 100 }
    }
}

impl PartialEq for StoredPeer {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}

impl Eq for StoredPeer {}

impl Ord for StoredPeer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority.cmp(&other.priority)
    }
}

impl PartialOrd for StoredPeer {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Holds peers that didn't fit in connection slots
#[derive(Debug)]
pub struct PeerStorage {
    my_ip: Option<SocketAddr>,
    stored_peers: HashSet<SocketAddr>,
    best_peers: BinaryHeap<StoredPeer>,
}

impl PeerStorage {
    const MAX_SIZE: usize = 1000;

    pub fn new(my_ip: Option<SocketAddr>) -> Self {
        Self {
            my_ip,
            stored_peers: HashSet::new(),
            best_peers: BinaryHeap::new(),
        }
    }

    pub fn add(&mut self, ip: SocketAddr) -> bool {
        if self.len() >= Self::MAX_SIZE {
            tracing::warn!(
                "Can't save peer for later. Peer storage is full {}/{}",
                self.len(),
                Self::MAX_SIZE
            );
            return false;
        }
        let is_new = self.stored_peers.insert(ip);
        if is_new {
            match self.my_ip {
                Some(my_ip) => self.best_peers.push(StoredPeer::new(ip, my_ip)),
                None => self.best_peers.push(StoredPeer::new_with_base_priority(ip)),
            }
        }
        is_new
    }

    pub fn pop(&mut self) -> Option<SocketAddr> {
        let best = self.best_peers.pop()?;
        self.stored_peers.remove(&best.ip);
        Some(best.ip)
    }

    pub fn set_my_ip(&mut self, ip: Option<SocketAddr>) {
        self.my_ip = ip;
        if let Some(ip) = ip {
            let mut old_heap = BinaryHeap::with_capacity(self.best_peers.len());
            std::mem::swap(&mut self.best_peers, &mut old_heap);
            for peer in old_heap {
                self.best_peers.push(StoredPeer::new(peer.ip, ip));
            }
        }
    }

    pub fn my_ip(&self) -> Option<SocketAddr> {
        self.my_ip
    }

    pub fn len(&self) -> usize {
        self.best_peers.len()
    }
}

#[cfg(test)]
mod test {
    use super::{ExtensionHandshake, UtMessage};

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
