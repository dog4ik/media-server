use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
    time::{Duration, Instant},
};

use anyhow::{anyhow, Context};
use bytes::BytesMut;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot},
};
use tokio_util::sync::CancellationToken;

use crate::{
    protocol::tracker::{
        TrackerEvent, UdpTrackerMessage, UdpTrackerMessageType, UdpTrackerRequest,
        UdpTrackerRequestType,
    },
    utils, BitField, Info,
};

pub const ID: [u8; 20] = *b"00112233445566778899";
pub const PORT: u16 = 6881;
pub const ANNOUNCE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct AnnounceResult {
    pub interval: usize,
    pub leeachs: Option<usize>,
    pub seeds: Option<usize>,
    pub peers: Vec<SocketAddr>,
}

#[derive(Debug, Clone)]
pub struct AnnouncePayload {
    pub announce: Url,
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub event: TrackerEvent,
}

impl AnnouncePayload {
    async fn announce_http(&self) -> anyhow::Result<AnnounceResult> {
        tracing::debug!("Announcing tracker {} via HTTP", self.announce);
        let url_params = HttpAnnounceUrlParams::from_payload(self);
        let tracker_url = format!(
            "{}?{}&info_hash={}",
            self.announce,
            serde_urlencoded::to_string(&url_params)?,
            &urlencode(&self.info_hash)
        );
        let response = reqwest::get(tracker_url).await?;
        let announce_bytes = response.bytes().await?;
        let response: HttpAnnounceResponse = serde_bencode::from_bytes(&announce_bytes)?;
        Ok(response.into())
    }

    async fn announce_udp(
        &self,
        channel: &UdpTrackerChannel,
        connection_id: u64,
    ) -> anyhow::Result<AnnounceResult> {
        let addrs = self.announce.socket_addrs(|| None)?;
        let addr = addrs.first().context("domain resoved in 0 addrs")?;

        let res = channel
            .send(
                UdpTrackerRequestType::Announce {
                    connection_id,
                    info_hash: self.info_hash,
                    peer_id: self.peer_id,
                    downloaded: self.downloaded,
                    left: self.left,
                    uploaded: self.uploaded,
                    event: self.event,
                    ip: 0,
                    key: rand::random(),
                    num_want: -1,
                    port: self.port,
                },
                *addr,
            )
            .await?;

        if let UdpTrackerMessageType::Announce {
            interval,
            leechers,
            seeders,
            peers,
        } = res.message_type
        {
            Ok(AnnounceResult {
                interval: interval as usize,
                leeachs: Some(leechers as usize),
                seeds: Some(seeders as usize),
                peers,
            })
        } else {
            Err(anyhow!(
                "Expected announce response, got {:?}",
                res.message_type
            ))
        }
    }
}

fn urlencode(t: &[u8; 20]) -> String {
    let mut encoded = String::with_capacity(3 * t.len());
    for &byte in t {
        encoded.push('%');
        encoded.push_str(&hex::encode([byte]));
    }
    encoded
}

#[derive(Serialize, Debug, Clone)]
struct HttpAnnounceUrlParams {
    peer_id: String,
    port: u16,
    uploaded: u64,
    downloaded: u64,
    left: u64,
}

impl HttpAnnounceUrlParams {
    pub fn from_payload(announce: &AnnouncePayload) -> Self {
        Self {
            peer_id: String::from_utf8(announce.peer_id.to_vec()).unwrap(),
            port: announce.port,
            uploaded: announce.uploaded,
            downloaded: announce.downloaded,
            left: announce.left,
        }
    }
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct HttpAnnounceFullPeer {
    #[serde(rename = "peer id")]
    peer_id: bytes::Bytes,
    ip: String,
    port: u16,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(untagged)]
pub enum HttpPeerList {
    Full(Vec<HttpAnnounceFullPeer>),
    Compact(bytes::Bytes),
}

#[derive(Deserialize, Serialize, Debug, Clone)]
struct HttpAnnounceResponse {
    interval: u32,
    peers: HttpPeerList,
}

impl From<HttpAnnounceResponse> for AnnounceResult {
    fn from(val: HttpAnnounceResponse) -> AnnounceResult {
        AnnounceResult {
            interval: val.interval as usize,
            leeachs: None,
            seeds: None,
            peers: val.peers(),
        }
    }
}

impl HttpAnnounceResponse {
    pub fn peers(&self) -> Vec<SocketAddr> {
        let mut result = Vec::new();
        match &self.peers {
            HttpPeerList::Full(peers) => {
                for peer in peers {
                    let Ok(ip) = IpAddr::from_str(&peer.ip) else {
                        continue;
                    };
                    result.push(SocketAddr::new(ip, peer.port));
                }
            }
            HttpPeerList::Compact(bytes) => {
                for slice in bytes.array_chunks::<6>() {
                    let ip = u32::from_be_bytes(slice[0..4].try_into().unwrap());
                    let port = u16::from_be_bytes(slice[4..6].try_into().unwrap());
                    let ip = Ipv4Addr::from_bits(ip);
                    result.push(SocketAddr::new(ip.into(), port));
                }
            }
        };
        result
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DownloadStat {
    pub downloaded: u64,
    pub uploaded: u64,
    pub left: u64,
}

impl DownloadStat {
    pub fn empty(left: u64) -> Self {
        Self {
            downloaded: 0,
            uploaded: 0,
            left,
        }
    }

    pub fn new(bitfield: &BitField, info: &Info) -> Self {
        let total_pieces = info.pieces.len();
        let piece_len = info.piece_length;
        let total_len = info.total_size();
        let mut downloaded = 0;
        for piece_i in bitfield.pieces() {
            if piece_i == total_pieces - 1 {
                downloaded += utils::piece_size(piece_i, piece_len, total_len)
            } else {
                downloaded += piece_len as u64
            }
        }
        Self {
            downloaded,
            uploaded: 0,
            left: total_len - downloaded,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UdpTrackerChannel {
    sender: mpsc::Sender<UdpTrackerRequest>,
}

impl UdpTrackerChannel {
    pub fn new(sender: mpsc::Sender<UdpTrackerRequest>) -> Self {
        Self { sender }
    }

    pub async fn send(
        &self,
        request: UdpTrackerRequestType,
        addr: SocketAddr,
    ) -> anyhow::Result<UdpTrackerMessage> {
        let (tx, rx) = oneshot::channel();
        let request = UdpTrackerRequest::new(request, addr, tx);
        self.sender.send(request).await?;
        rx.await.context("receive response")
    }

    pub async fn connect(&self, addr: SocketAddr) -> anyhow::Result<u64> {
        let res = self.send(UdpTrackerRequestType::Connect, addr).await?;
        if let UdpTrackerMessageType::Connect { connection_id } = res.message_type {
            Ok(connection_id)
        } else {
            Err(anyhow::anyhow!(
                "Expected connect response, got {:?}",
                res.message_type
            ))
        }
    }
}

#[derive(Debug)]
pub enum TrackerType {
    Http,
    Udp(UdpTrackerChannel),
}

impl TrackerType {
    pub fn from_url(url: &Url, sender: &UdpTrackerChannel) -> anyhow::Result<Self> {
        match url.scheme() {
            "https" | "http" => Ok(Self::Http),
            "udp" => Ok(Self::Udp(sender.clone())),
            rest => Err(anyhow::anyhow!("url scheme {rest} is not supported")),
        }
    }
}

#[derive(Debug, Clone)]
pub struct TrackerHandle {
    command_tx: mpsc::Sender<TrackerCommand>,
    cancellation_token: CancellationToken,
    url: Url,
}

impl TrackerHandle {
    pub fn announce(&self, stat: DownloadStat) {
        self.command_tx
            .try_send(TrackerCommand::Reannounce(stat))
            .unwrap();
    }
    pub fn close(&self) {
        self.cancellation_token.cancel();
    }

    pub fn url(&self) -> &str {
        self.url.as_ref()
    }
}

#[derive(Debug, Clone, Copy)]
pub enum TrackerCommand {
    Reannounce(DownloadStat),
}

#[derive(Debug, Clone)]
pub enum TrackerResponse {
    Failure {
        reason: String,
    },
    AnnounceResponse {
        peers: Vec<SocketAddr>,
        interval: Duration,
    },
}

const MAX_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug)]
pub struct Tracker {
    pub tracker_type: TrackerType,
    pub url: Url,
    pub commands: mpsc::Receiver<TrackerCommand>,
    pub rensponse_tx: mpsc::Sender<TrackerResponse>,
    pub announce_payload: AnnouncePayload,
    pub udp_connection_id: Option<u64>,
    pub cancellation_token: CancellationToken,
}

impl Tracker {
    pub fn new(
        info_hash: [u8; 20],
        tracker_type: TrackerType,
        url: Url,
        initial_stats: DownloadStat,
        rensponse_tx: mpsc::Sender<TrackerResponse>,
        cancellation_token: CancellationToken,
    ) -> (TrackerHandle, Self) {
        let (command_tx, command_rx) = mpsc::channel(10);
        let announce_payload = AnnouncePayload {
            announce: url.clone(),
            info_hash,
            peer_id: ID,
            port: PORT,
            uploaded: initial_stats.uploaded,
            downloaded: initial_stats.downloaded,
            left: initial_stats.left,
            event: TrackerEvent::Started,
        };

        let tracker = Self {
            tracker_type,
            url: url.clone(),
            commands: command_rx,
            rensponse_tx,
            announce_payload,
            udp_connection_id: None,
            cancellation_token: cancellation_token.clone(),
        };

        let handle = TrackerHandle {
            cancellation_token,
            command_tx,
            url,
        };

        (handle, tracker)
    }

    pub async fn work(&mut self) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(command) = self.commands.recv() => {
                    match command {
                        TrackerCommand::Reannounce(stat) => {
                            if stat.downloaded == 0 {
                                self.announce_payload.event = TrackerEvent::Started;
                            } else {
                                self.announce_payload.event = TrackerEvent::Empty;
                            }
                            self.announce_payload.downloaded = stat.downloaded;
                            self.announce_payload.uploaded = stat.uploaded;
                            self.announce_payload.left = stat.left;
                            if stat.left == 0 {
                                self.announce_payload.event = TrackerEvent::Completed;
                            }
                            match tokio::time::timeout(ANNOUNCE_TIMEOUT, self.announce()).await {
                                Ok(Ok(_)) => {},
                                Ok(Err(e)) => {
                                    tracing::warn!(url = %self.url, "Announce request failed: {e}");
                                    self.send_response(TrackerResponse::Failure{ reason: e.to_string() }).await?;
                                },
                                Err(_) => {
                                    tracing::warn!(url = %self.url, "Announce request timed out");
                                    self.send_response(
                                        TrackerResponse::Failure {
                                            reason: format!("Tracker announce timed out") }
                                    )
                                    .await?;
                                }
                            };
                        },
                    }
                },
                _ = self.cancellation_token.cancelled() => {
                    break;
                },
            }
        }
        Ok(())
    }

    pub async fn announce(&mut self) -> anyhow::Result<()> {
        tracing::debug!("Announcing tracker {}", self.url);
        let announce_result = match &self.tracker_type {
            TrackerType::Http => self.announce_payload.announce_http().await,
            TrackerType::Udp(chan) => {
                let conn_id = match self.udp_connection_id {
                    Some(id) => id,
                    None => {
                        tracing::debug!(
                            "Trying to get connection id from udp tracker {}",
                            self.url
                        );
                        let addrs = self.url.socket_addrs(|| None)?;
                        let addr = addrs.first().context("could not resove url hostname")?;
                        let id = chan.connect(*addr).await?;
                        self.udp_connection_id = Some(id);
                        id
                    }
                };
                self.announce_payload.announce_udp(chan, conn_id).await
            }
        };
        self.handle_announce(announce_result?).await
    }

    async fn handle_announce(&self, announce_result: AnnounceResult) -> anyhow::Result<()> {
        self.send_response(TrackerResponse::AnnounceResponse {
            interval: Duration::from_secs(announce_result.interval as u64)
                .max(MAX_ANNOUNCE_INTERVAL),
            peers: announce_result.peers,
        })
        .await?;
        Ok(())
    }

    async fn send_response(&self, response: TrackerResponse) -> anyhow::Result<()> {
        self.rensponse_tx.send(response).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub enum TrackerStatus {
    Working,
    #[default]
    NotContacted,
    Error(String),
}

/// This tracker handle is used inside download loop
#[derive(Debug)]
pub struct DownloadTracker {
    pub response_rx: mpsc::Receiver<TrackerResponse>,
    pub status: TrackerStatus,
    pub announce_interval: Duration,
    pub last_announced_at: Instant,
    handle: TrackerHandle,
}

impl DownloadTracker {
    pub fn new(
        info_hash: [u8; 20],
        tracker_type: TrackerType,
        url: Url,
        initial_stats: DownloadStat,
        cancellation_token: CancellationToken,
    ) -> (Self, Tracker) {
        let (response_tx, response_rx) = mpsc::channel(10);
        let (handle, tracker) = Tracker::new(
            info_hash,
            tracker_type,
            url,
            initial_stats,
            response_tx,
            cancellation_token,
        );
        let download_tracker = Self {
            response_rx,
            status: TrackerStatus::default(),
            announce_interval: MAX_ANNOUNCE_INTERVAL,
            last_announced_at: Instant::now(),
            handle,
        };
        (download_tracker, tracker)
    }

    pub fn announce(&mut self, stat: DownloadStat) {
        self.last_announced_at = Instant::now();
        self.handle.announce(stat);
    }

    pub fn handle_messages(&mut self) -> Vec<SocketAddr> {
        let mut announce_peers = Vec::new();
        while let Ok(message) = self.response_rx.try_recv() {
            match message {
                TrackerResponse::Failure { reason } => {
                    self.status = TrackerStatus::Error(reason);
                }
                TrackerResponse::AnnounceResponse { peers, interval } => {
                    self.announce_interval = interval;
                    announce_peers.extend(peers.into_iter());
                    self.status = TrackerStatus::Working;
                }
            }
        }
        announce_peers
    }
    pub fn url(&self) -> &str {
        self.handle.url()
    }
}

#[derive(Debug)]
/// Entity that owns udp socket and handles all udp tracker messages
pub struct UdpTrackerWorker {
    socket: UdpSocket,
    cancellation_token: CancellationToken,
}

impl UdpTrackerWorker {
    pub async fn bind(
        local_addr: SocketAddrV4,
        cancellation_token: CancellationToken,
    ) -> anyhow::Result<Self> {
        let socket = utils::bind_udp_socket(local_addr).await?;
        Ok(Self {
            socket,
            cancellation_token,
        })
    }

    pub async fn spawn(self) -> anyhow::Result<UdpTrackerChannel> {
        let (data_tx, data_rx) = mpsc::channel(100);
        tokio::spawn(self.udp_tracker_worker(data_rx));
        let channel = UdpTrackerChannel::new(data_tx);
        Ok(channel)
    }

    async fn udp_tracker_worker(self, mut data_rx: mpsc::Receiver<UdpTrackerRequest>) {
        let mut pending_transactions: HashMap<u32, oneshot::Sender<UdpTrackerMessage>> =
            HashMap::new();
        loop {
            let mut buffer = BytesMut::with_capacity(1024 * 10);
            tokio::select! {
                Ok((read, addr)) = self.socket.recv_buf_from(&mut buffer) => {
                    tracing::debug!("Received {read} bytes from UDP worker from {:?} address", addr);
                    let message = match UdpTrackerMessage::from_bytes(&buffer[..read]) {
                        Ok(msg) => msg,
                        Err(e) => {
                            tracing::error!("Failed to construct message from udp tracker: {e}");
                            continue;
                        }
                    };
                    if let Some(chan) = pending_transactions.remove(&message.transaction_id) {
                        let _ = chan.send(message);
                    } else {
                        tracing::error!(
                            "Received message {:?} for non existent transaction: {}",
                            message.message_type,
                            message.transaction_id
                        );
                    }
                },
                Some(request) = data_rx.recv() => {
                    let _ = self.socket.send_to(&request.as_bytes(), request.tracker_addr).await;
                    pending_transactions.insert(request.transaction_id, request.response);
                }
                _ = self.cancellation_token.cancelled() => {
                    break;
                }
            }
        }
    }
}
