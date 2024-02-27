use std::{
    collections::HashSet,
    io::Write,
    mem,
    net::{Ipv4Addr, SocketAddrV4},
    str::FromStr,
    time::Duration,
};

use anyhow::{anyhow, Context};
use bytes::BufMut;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tokio::{
    net::UdpSocket,
    sync::{broadcast, mpsc},
};

use crate::{
    file::{MagnetLink, TorrentFile},
    peers::Peer,
    NewPeer,
};

pub const ID: [u8; 20] = *b"00112233445566778899";
pub const PORT: u16 = 6881;

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum TrackerEvent {
    Started,
    Completed,
    Stopped,
    Empty,
}

#[derive(Debug, Clone)]
pub struct AnnounceResult {
    pub interval: u32,
    pub leeachs: u32,
    pub seeds: u32,
    pub peers: Vec<SocketAddrV4>,
}

#[derive(Debug, Clone)]
pub struct AnnouncePayload {
    pub announce: reqwest::Url,
    pub info_hash: [u8; 20],
    pub peer_id: [u8; 20],
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
    pub event: Option<TrackerEvent>,
}

impl AnnouncePayload {
    pub fn from_torrent(torrent: &TorrentFile) -> anyhow::Result<Self> {
        Ok(Self {
            announce: reqwest::Url::parse(&torrent.announce).context("parse announce url")?,
            info_hash: torrent.info.hash(),
            peer_id: ID,
            port: PORT,
            uploaded: 0,
            downloaded: 0,
            left: torrent.info.total_size(),
            event: Some(TrackerEvent::Started),
        })
    }

    pub fn from_magnet_link(magnet_link: MagnetLink) -> anyhow::Result<Self> {
        Ok(Self {
            announce: magnet_link
                .announce_list
                .as_ref()
                .unwrap()
                .first()
                .unwrap()
                .clone(),
            info_hash: hex::decode(magnet_link.info_hash)?.try_into().unwrap(),
            peer_id: ID,
            port: PORT,
            uploaded: 0,
            downloaded: 0,
            left: 0,
            event: Some(TrackerEvent::Started),
        })
    }

    async fn announce_http(&self) -> anyhow::Result<HttpAnnounceResponse> {
        let url_params = HttpAnnounceUrlParams {
            peer_id: String::from_utf8(self.peer_id.to_vec())?,
            port: self.port,
            uploaded: self.uploaded,
            downloaded: self.downloaded,
            left: self.left,
        };
        let tracker_url = format!(
            "{}?{}&info_hash={}",
            self.announce,
            serde_urlencoded::to_string(&url_params)?,
            &urlencode(&self.info_hash)
        );
        let response = reqwest::get(tracker_url).await?;
        let announce_bytes = response.bytes().await?;
        let response: HttpAnnounceResponse = serde_bencode::from_bytes(&announce_bytes)?;
        Ok(response)
    }

    async fn announce_udp(&self, socket: UdpSocket) -> anyhow::Result<UdpAnnounceResponse> {
        let announce_url = format!(
            "{}:{}",
            self.announce
                .domain()
                .ok_or(anyhow!("url domain missing {}", self.announce))?,
            self.announce
                .port()
                .ok_or(anyhow!("url port misisng {}", self.announce))?
        );

        socket.connect(announce_url).await?;
        let connect_payload = UdpConnectRequest::new();
        let connect_payload_bytes = connect_payload.as_bytes();

        socket.send(&connect_payload_bytes).await?;
        let mut connect_response_buffer = [0u8; mem::size_of::<UdpConnectResponse>()];
        socket.recv(&mut connect_response_buffer).await?;
        let connect_response = UdpConnectResponse::from_bytes(&connect_response_buffer)
            .context("construct connect response from peer")?;
        let peers_request = UdpAnnounce::new(connect_response.connection_id, &self);
        let peers_request_bytes = peers_request.as_bytes();
        socket.send(&peers_request_bytes).await?;

        let mut announce_response_buffer = Vec::with_capacity(10240);
        socket.recv_buf(&mut announce_response_buffer).await?;
        let announce_response = UdpAnnounceResponse::from_bytes(&announce_response_buffer)?;
        Ok(announce_response)
    }

    pub async fn announce(&self) -> anyhow::Result<AnnounceResult> {
        let scheme = self.announce.scheme();
        match scheme {
            "http" | "https" => self.announce_http().await.map(|x| x.into()),
            "udp" => {
                let local_ip = SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), PORT);
                // TODO: bind single udp socket and communicate over it
                let connection = crate::utils::bind_udp_socket(local_ip).await?;
                self.announce_udp(connection).await.map(|x| x.into())
            }
            scheme => return Err(anyhow!("Unsupproted url scheme: {}", scheme)),
        }
    }
}

fn urlencode(t: &[u8; 20]) -> String {
    let mut encoded = String::with_capacity(3 * t.len());
    for &byte in t {
        encoded.push('%');
        encoded.push_str(&hex::encode(&[byte]));
    }
    encoded
}

#[derive(Serialize, Debug, Clone)]
pub struct HttpAnnounceUrlParams {
    pub peer_id: String,
    pub port: u16,
    pub uploaded: u64,
    pub downloaded: u64,
    pub left: u64,
}

impl HttpAnnounceUrlParams {
    pub fn new(announce: &AnnouncePayload) -> Self {
        Self {
            peer_id: String::from_utf8(ID.to_vec()).unwrap(),
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
            interval: self.interval,
            leeachs: 0,
            seeds: 0,
            peers: self.peers(),
        }
    }
}

impl HttpAnnounceResponse {
    pub fn peers(&self) -> Vec<SocketAddrV4> {
        let mut result = Vec::new();
        match &self.peers {
            HttpPeerList::Full(peers) => {
                for peer in peers {
                    let Ok(ip) = Ipv4Addr::from_str(&peer.ip) else {
                        continue;
                    };
                    result.push(SocketAddrV4::new(ip, peer.port));
                }
            }
            HttpPeerList::Compact(bytes) => {
                for slice in bytes.array_chunks::<6>() {
                    let ip = u32::from_be_bytes(slice[0..4].try_into().unwrap());
                    let port = u16::from_be_bytes(slice[4..6].try_into().unwrap());
                    let ip = Ipv4Addr::from_bits(ip);
                    result.push(SocketAddrV4::new(ip, port));
                }
            }
        };
        result
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UdpAnnounce {
    connection_id: u64,
    action: u32,
    transaction_id: u32,
    info_hash: [u8; 20],
    peer_id: [u8; 20],
    downloaded: u64,
    left: u64,
    uploaded: u64,
    event: u32,
    ip: u32,
    key: u32,
    num_want: i32,
    port: u16,
}

impl UdpAnnounce {
    pub fn new(connection_id: u64, announce: &AnnouncePayload) -> Self {
        let transaction_id = 2;
        Self {
            connection_id,
            action: 1,
            transaction_id,
            info_hash: announce.info_hash,
            peer_id: announce.peer_id,
            downloaded: announce.downloaded,
            left: announce.left,
            uploaded: announce.uploaded,
            event: 1,
            ip: 0,
            key: 1,
            num_want: -1,
            port: announce.port,
        }
    }

    pub fn as_bytes(&self) -> [u8; 98] {
        let mut bytes: [u8; 98] = [0_u8; 98];
        let mut writer = bytes.writer();
        writer.write(&self.connection_id.to_be_bytes()).unwrap();
        writer.write(&self.action.to_be_bytes()).unwrap();
        writer.write(&self.transaction_id.to_be_bytes()).unwrap();
        writer.write(&self.info_hash).unwrap();
        writer.write(&self.peer_id).unwrap();
        writer.write(&self.downloaded.to_be_bytes()).unwrap();
        writer.write(&self.left.to_be_bytes()).unwrap();
        writer.write(&self.uploaded.to_be_bytes()).unwrap();
        writer.write(&self.event.to_be_bytes()).unwrap();
        writer.write(&self.ip.to_be_bytes()).unwrap();
        writer.write(&self.key.to_be_bytes()).unwrap();
        writer.write(&self.num_want.to_be_bytes()).unwrap();
        writer.write(&self.port.to_be_bytes()).unwrap();
        bytes
    }
}

#[derive(Debug, Clone)]
struct UdpAnnounceResponse {
    action: u32,
    transaction_id: u32,
    interval: u32,
    leechers: u32,
    seeders: u32,
    ips: Vec<SocketAddrV4>,
}

impl Into<AnnounceResult> for UdpAnnounceResponse {
    fn into(self) -> AnnounceResult {
        AnnounceResult {
            interval: self.interval,
            leeachs: self.leechers,
            seeds: self.seeders,
            peers: self.ips,
        }
    }
}

impl UdpAnnounceResponse {
    pub fn from_bytes(bytes: &[u8]) -> anyhow::Result<Self> {
        let action = u32::from_be_bytes(bytes[0..4].try_into().unwrap());
        let transaction_id = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
        let interval = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
        let leechers = u32::from_be_bytes(bytes[12..16].try_into().unwrap());
        let seeders = u32::from_be_bytes(bytes[16..20].try_into().unwrap());

        let mut ips = Vec::new();
        for slice in bytes[20..].array_chunks::<6>() {
            let ip = u32::from_be_bytes(slice[0..4].try_into().unwrap());
            let port = u16::from_be_bytes(slice[4..6].try_into().unwrap());
            let ip = Ipv4Addr::from_bits(ip);
            ips.push(SocketAddrV4::new(ip, port));
        }
        Ok(Self {
            action,
            transaction_id,
            interval,
            leechers,
            seeders,
            ips,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UdpConnectRequest {
    pub protocol_id: u64,
    pub action: u32,
    pub transaction_id: u32,
}

impl UdpConnectRequest {
    pub fn new() -> Self {
        let transaction_id = rand::random();
        let protocol_id = 0x41727101980;
        Self {
            protocol_id,
            action: 0,
            transaction_id,
        }
    }

    pub fn as_bytes(&self) -> [u8; 16] {
        let mut bytes: [u8; 16] = [0; 16];
        let mut writer = bytes.writer();
        writer.write(&self.protocol_id.to_be_bytes()).unwrap();
        writer.write(&self.action.to_be_bytes()).unwrap();
        writer.write(&self.transaction_id.to_be_bytes()).unwrap();
        bytes
    }
}

#[derive(Debug, Clone, Copy)]
pub struct UdpConnectResponse {
    pub action: u32,
    pub transaction_id: u32,
    pub connection_id: u64,
}

impl UdpConnectResponse {
    pub fn from_bytes(mut bytes: &[u8]) -> anyhow::Result<Self> {
        let action: u32 = u32::from_be_bytes(bytes[0..4].try_into()?);
        let transaction_id: u32 = u32::from_be_bytes(bytes[4..8].try_into()?);
        let connection_id: u64 = u64::from_be_bytes(bytes[8..16].try_into()?);
        Ok(Self {
            action,
            transaction_id,
            connection_id,
        })
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

#[derive(Debug)]
pub struct Tracker {
    pub url: Url,
    pub sender: mpsc::Sender<NewPeer>,
    pub update: broadcast::Receiver<DownloadStat>,
    pub peers: HashSet<SocketAddrV4>,
    pub announce_payload: AnnouncePayload,
    pub info_hash: [u8; 20],
}

impl Tracker {
    pub fn from_url(
        url: Url,
        info_hash: [u8; 20],
        sender: mpsc::Sender<NewPeer>,
        update: broadcast::Receiver<DownloadStat>,
        initial_stat: DownloadStat,
    ) -> anyhow::Result<Self> {
        let announce_payload = AnnouncePayload {
            announce: url.clone(),
            info_hash,
            peer_id: ID,
            port: PORT,
            uploaded: initial_stat.uploaded,
            downloaded: initial_stat.downloaded,
            left: initial_stat.left,
            event: Some(TrackerEvent::Started),
        };
        Ok(Self {
            url,
            sender,
            update,
            peers: HashSet::new(),
            announce_payload,
            info_hash,
        })
    }

    pub fn work(mut self) {
        tokio::spawn(async move {
            let announce_result = self.announce_payload.announce().await.unwrap();
            dbg!("announced");
            let reannounce_duration = std::cmp::max(
                Duration::from_secs(2 * 60),
                Duration::from_secs(u64::from(announce_result.interval)),
            );

            self.handle_reannounce(announce_result).await;

            let mut reannounce_interval = tokio::time::interval(reannounce_duration);
            // immediate tick
            reannounce_interval.tick().await;

            loop {
                tokio::select! {
                    _ = reannounce_interval.tick() => {
                        let announce_result = self.announce_payload.announce().await.unwrap();
                        self.handle_reannounce(announce_result).await;
                    }
                    Ok(update) = self.update.recv() => self.handle_update(update),
                    else => break
                }
            }
            dbg!("tracker finished");
        });
    }

    pub async fn handle_reannounce(&mut self, announce_result: AnnounceResult) {
        for ip in announce_result.peers {
            if self.peers.insert(ip) {
                self.sender.send(NewPeer::TrackerOrigin(ip)).await.unwrap();
            };
        }
    }

    fn handle_update(&mut self, new: DownloadStat) {
        self.announce_payload.downloaded = new.downloaded;
        self.announce_payload.uploaded = new.uploaded;
        self.announce_payload.left = new.left;
    }
}

#[cfg(test)]
mod tests {

    use crate::{file::TorrentFile, tracker::AnnouncePayload};

    #[tokio::test]
    async fn http_announce_tracker() {
        let torrent = TorrentFile::from_path("torrents/codecrafters.torrent").unwrap();
        let announce = AnnouncePayload::from_torrent(&torrent).unwrap();
        let announce = announce.announce().await.unwrap();
        dbg!(announce.peers);
    }

    #[tokio::test]
    async fn udp_announce_tracker() {
        let torrent = TorrentFile::from_path("torrents/yts.torrent").unwrap();
        let announce = AnnouncePayload::from_torrent(&torrent).unwrap();
        let announce = announce.announce().await.unwrap();
        dbg!(announce.peers);
    }
}
