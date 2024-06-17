use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
    time::Duration,
};

use anyhow::{anyhow, Context};
use bytes::BytesMut;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, oneshot, watch},
};
use tokio_util::sync::CancellationToken;

use crate::{
    protocol::tracker::{
        TrackerEvent, UdpTrackerMessage, UdpTrackerMessageType, UdpTrackerRequest,
        UdpTrackerRequestType,
    },
    utils, NewPeer,
};

pub const ID: [u8; 20] = *b"00112233445566778899";
pub const PORT: u16 = 6881;

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

impl Into<AnnounceResult> for HttpAnnounceResponse {
    fn into(self) -> AnnounceResult {
        AnnounceResult {
            interval: self.interval as usize,
            leeachs: None,
            seeds: None,
            peers: self.peers(),
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
    downloaded: u64,
    uploaded: u64,
    left: u64,
}

impl DownloadStat {
    pub fn empty(left: u64) -> Self {
        Self {
            downloaded: 0,
            uploaded: 0,
            left,
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
        rx.await.context("recieve response")
    }

    pub async fn connect(&self, addr: SocketAddr) -> anyhow::Result<u64> {
        let res = self.send(UdpTrackerRequestType::Connect, addr).await?;
        if let UdpTrackerMessageType::Connect { connection_id } = res.message_type {
            return Ok(connection_id);
        } else {
            return Err(anyhow::anyhow!(
                "Expected connect response, got {:?}",
                res.message_type
            ));
        }
    }
}

#[derive(Debug)]
pub enum TrackerType {
    Http,
    Udp(UdpTrackerChannel),
}

impl TrackerType {
    pub fn from_url(url: &Url, sender: UdpTrackerChannel) -> anyhow::Result<Self> {
        match url.scheme() {
            "https" | "http" => Ok(Self::Http),
            "udp" => Ok(Self::Udp(sender)),
            rest => Err(anyhow::anyhow!("url scheme {rest} is not supported")),
        }
    }
}

#[derive(Debug)]
pub struct Tracker {
    pub tracker_type: TrackerType,
    pub url: Url,
    pub peers: HashSet<SocketAddr>,
    pub progress: watch::Receiver<DownloadStat>,
    pub peer_tx: mpsc::Sender<NewPeer>,
    pub announce_payload: AnnouncePayload,
    pub udp_connection_id: Option<u64>,
}

impl Tracker {
    pub async fn new(
        info_hash: [u8; 20],
        tracker_type: TrackerType,
        url: Url,
        mut progress: watch::Receiver<DownloadStat>,
        peer_tx: mpsc::Sender<NewPeer>,
    ) -> anyhow::Result<Self> {
        let stats = { progress.borrow_and_update().clone() };
        let announce_payload = AnnouncePayload {
            announce: url.clone(),
            info_hash,
            peer_id: ID,
            port: PORT,
            uploaded: stats.uploaded,
            downloaded: stats.downloaded,
            left: stats.left,
            event: TrackerEvent::Started,
        };

        let udp_connection_id = match &tracker_type {
            TrackerType::Http => None,
            TrackerType::Udp(c) => {
                let addrs = url.socket_addrs(|| None)?;
                let addr = addrs.first().context("could not resove url hostname")?;
                let res = c.connect(*addr).await?;
                Some(res)
            }
        };

        Ok(Self {
            tracker_type,
            url,
            peers: HashSet::new(),
            progress,
            peer_tx,
            announce_payload,
            udp_connection_id,
        })
    }

    pub async fn work(mut self, cancellation_token: CancellationToken) -> anyhow::Result<()> {
        let initial_announce = self.announce().await?;
        let interval_duration = Duration::from_secs(initial_announce.interval as u64);
        self.handle_announce(initial_announce).await;
        self.announce_payload.event = TrackerEvent::Empty;

        let mut reannounce_interval = tokio::time::interval(interval_duration);
        // immediate tick
        reannounce_interval.tick().await;

        loop {
            tokio::select! {
                _ = reannounce_interval.tick() => {
                    let announce_result = self.announce().await?;
                    self.handle_announce(announce_result).await;
                }
                Ok(_) = self.progress.changed() => self.handle_progress_update(),
                _ = cancellation_token.cancelled() => {
                    break;
                },
            }
        }
        Ok(())
    }

    pub async fn announce(&mut self) -> anyhow::Result<AnnounceResult> {
        tracing::debug!("Announcing tracker {}", self.url);
        match &self.tracker_type {
            TrackerType::Http => self.announce_payload.announce_http().await,
            TrackerType::Udp(chan) => {
                self.announce_payload
                    .announce_udp(chan, self.udp_connection_id.unwrap())
                    .await
            }
        }
    }

    pub async fn handle_announce(&mut self, announce_result: AnnounceResult) {
        let mut count = 0;
        for ip in announce_result.peers {
            if self.peers.insert(ip) {
                count += 1;
                self.peer_tx.send(NewPeer::TrackerOrigin(ip)).await.unwrap();
            };
        }
        tracing::debug!("Tracker {} announced {count} new peers", self.url);
    }

    fn handle_progress_update(&mut self) {
        let new = self.progress.borrow_and_update();
        self.announce_payload.downloaded = new.downloaded;
        self.announce_payload.uploaded = new.uploaded;
        self.announce_payload.left = new.left;
        if new.left == 0 {
            self.announce_payload.event = TrackerEvent::Completed;
        }
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
                    tracing::debug!("Recieved {read} bytes from UDP worker from {:?} address", addr);
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
                            "Recieved message {:?} for non existant transaction: {}",
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
