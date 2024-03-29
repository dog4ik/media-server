#![feature(array_chunks)]
#![feature(iter_repeat_n)]
#![feature(ip_bits)]
#![feature(iter_array_chunks)]

use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
    str::FromStr,
    time::Duration,
};

use anyhow::anyhow;
use file::{Info, MagnetLink, TorrentFile};
use peers::Peer;
use reqwest::Url;
use tokio::{
    sync::{broadcast, mpsc},
    task::JoinSet,
    time::timeout,
};
use tracker::AnnouncePayload;
use uuid::Uuid;

use crate::{
    download::Download,
    tracker::{DownloadStat, Tracker},
};

mod download;
mod file;
mod peers;
mod scheduler;
mod storage;
mod tracker;
mod utils;

#[derive(Debug)]
pub struct ClientConfig {
    port: u16,
    max_connections: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            port: 6881,
            max_connections: 100,
        }
    }
}

#[derive(Debug)]
pub enum NewPeer {
    ListenerOrigin(Peer),
    TrackerOrigin(SocketAddrV4),
}

#[derive(Debug)]
struct PeerListener {
    new_torrent_channel: mpsc::Sender<([u8; 20], mpsc::Sender<NewPeer>)>,
}

impl PeerListener {
    pub async fn spawn(port: u16) -> anyhow::Result<Self> {
        let addr = SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port);
        let listener = utils::bind_tcp_listener(addr).await?;
        let (tx, mut rx) = mpsc::channel(100);
        tokio::spawn(async move {
            let mut map: HashMap<[u8; 20], mpsc::Sender<NewPeer>> = HashMap::new();
            loop {
                tokio::select! {
                    Ok((socket,ip)) = listener.accept() => {
                        let timeout_duration = Duration::from_secs(1);
                        match timeout(timeout_duration, Peer::new_without_info_hash(socket)).await {
                            Ok(Ok(peer)) => {
                                let info_hash = peer.handshake.info_hash();
                                if let Some(channel) = map.get_mut(&info_hash) {
                                    tracing::trace!("Peer connected via listener {}", ip);
                                    if let Err(_) = channel.send(NewPeer::ListenerOrigin(peer)).await {
                                        tracing::warn!(?info_hash, "Peer connected to outdated torrent");
                                        map.remove(&info_hash);
                                    };
                                } else {
                                    tracing::warn!(?info_hash, "Peer () connected but torrent does not exist", );
                                    peer.close();
                                }
                            }
                            Ok(Err(e)) => {
                                tracing::warn!("Failed to construct handshake with peer: {}", e);
                            }
                            Err(_) => {
                                tracing::trace!("Peer with ip {} timed out", ip);
                            }
                        }

                    },
                    Some((info_hash, sender)) = rx.recv() => {
                        map.insert(info_hash, sender);
                    }
                    else => break
                };
            }
            tracing::warn!("Peer listener finished");
        });
        Ok(Self {
            new_torrent_channel: tx,
        })
    }

    pub async fn subscribe(&mut self, info_hash: [u8; 20], sender: mpsc::Sender<NewPeer>) {
        self.new_torrent_channel
            .send((info_hash, sender))
            .await
            .unwrap();
    }
}

#[derive(Debug)]
enum WorkerCommand {
    Download(Torrent),
    Cancel(Uuid),
}

#[derive(Debug)]
pub struct Client {
    config: ClientConfig,
    torrents: Vec<Torrent>,
    peer_listener: PeerListener,
    tracker_set: JoinSet<()>,
    progress_channel: broadcast::Sender<DownloadStat>,
}

impl Client {
    pub async fn new(config: ClientConfig) -> anyhow::Result<Self> {
        let peer_listener = PeerListener::spawn(config.port).await?;
        let (tx, _) = broadcast::channel(10);

        Ok(Self {
            config,
            torrents: Vec::new(),
            tracker_set: JoinSet::new(),
            peer_listener,
            progress_channel: tx,
        })
    }

    pub async fn download(&mut self, torrent: Torrent) {
        let (tx, rx) = mpsc::channel(100);
        let hash = torrent.info.hash();
        self.peer_listener.subscribe(hash, tx.clone()).await;
        for tracker in torrent.trackers.clone() {
            let broadcast = self.progress_channel.subscribe();
            let tracker = Tracker::from_url(
                tracker,
                hash,
                tx.clone(),
                broadcast,
                // TODO: resume torrents
                DownloadStat::empty(torrent.info.total_size()),
            )
            .unwrap();
            tracker.work();
        }
        torrent.download("", rx).await;
    }
}

#[derive(Debug)]
pub struct Torrent {
    info: Info,
    trackers: Vec<Url>,
    config: ClientConfig,
}

impl Torrent {
    pub fn from_file(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let file = TorrentFile::from_path(path)?;
        Ok(Self {
            trackers: file.all_trackers(),
            info: file.info,
            config: ClientConfig::default(),
        })
    }

    pub async fn from_mangnet_link(url: &str) -> anyhow::Result<Self> {
        let magnet_link = MagnetLink::from_str(url)?;
        let Some(trackers) = magnet_link.announce_list.clone() else {
            unimplemented!("magnet links without trackers");
        };
        let info_hash = magnet_link.hash();
        let announce = AnnouncePayload::from_magnet_link(magnet_link).unwrap();
        let announce_result = announce.announce().await.unwrap();
        let mut peers_set = JoinSet::new();

        for ip in announce_result.peers {
            peers_set.spawn(async move {
                timeout(Duration::from_millis(500), Peer::new_from_ip(ip, info_hash)).await
            });
        }
        while let Some(peer) = peers_set.join_next().await {
            let Ok(Ok(Ok(mut peer))) = peer else {
                continue;
            };
            let Ok(info) = peer.fetch_ut_metadata().await else {
                continue;
            };
            return Ok(Self {
                info,
                config: ClientConfig::default(),
                trackers,
            });
        }
        Err(anyhow!("Could not fetch ut_metadata"))
    }

    pub async fn download(self, output_path: impl AsRef<Path>, new_peers: mpsc::Receiver<NewPeer>) {
        let mut download = Download::new(self.info, new_peers, self.config.max_connections).await;
        download.concurrent_download().await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tracing_test::traced_test;

    use crate::{Client, ClientConfig, Torrent};
    // bug with storage
    //"magnet:?xt=urn:btih:05ECEC0B8BE9110A70395A911018CC48C4071537&dn=Halo.S02E04.1080p.WEB.H264-SuccessfulCrab%5BTGx%5D&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337&tr=udp%3A%2F%2Ftracker.openbittorrent.com%3A6969%2Fannounce&tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce&tr=udp%3A%2F%2Ftracker.torrent.eu.org%3A451%2Fannounce&tr=udp%3A%2F%2Ftracker.bittor.pw%3A1337%2Fannounce&tr=udp%3A%2F%2Fpublic.popcorn-tracker.org%3A6969%2Fannounce&tr=udp%3A%2F%2Ftracker.dler.org%3A6969%2Fannounce&tr=udp%3A%2F%2Fexodus.desync.com%3A6969&tr=udp%3A%2F%2Fopentracker.i2p.rocks%3A6969%2Fannounce"
    #[tokio::test]
    #[traced_test]
    async fn test_download() {
        let mut client = Client::new(ClientConfig::default()).await.unwrap();
        let content = fs::read_to_string("torrents/halo.magnet").unwrap();
        let torrent = Torrent::from_mangnet_link(&content).await.unwrap();
        client.download(torrent).await;
    }
}
