#![feature(array_chunks)]
#![feature(iter_repeat_n)]
#![feature(assert_matches)]
#![feature(iter_array_chunks)]
#![feature(cursor_remaining)]
#![feature(iter_collect_into)]

use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::Path,
    sync::Arc,
    time::Duration,
};

use download::DownloadHandle;
pub use download::{DownloadProgress, ProgressConsumer};
use file::{MagnetLink, TorrentFile};
use peers::Peer;
use protocol::Info;
use reqwest::Url;
use storage::{verify_integrety, StorageMethod, TorrentStorage};
use tokio::{
    sync::{mpsc, watch, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tracker::{TrackerType, UdpTrackerChannel, UdpTrackerWorker};

use crate::{
    download::Download,
    tracker::{DownloadStat, Tracker},
};

pub mod download;
pub mod file;
mod peers;
pub mod protocol;
pub mod scheduler;
pub mod storage;
mod tracker;
mod utils;

#[derive(Debug)]
pub struct ClientConfig {
    port: u16,
    udp_listener_port: u16,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            port: 6881,
            udp_listener_port: 7897,
        }
    }
}

#[derive(Debug)]
pub enum NewPeer {
    ListenerOrigin(Peer),
    TrackerOrigin(SocketAddr),
}

#[derive(Debug)]
struct PeerListener {
    new_torrent_channel: mpsc::Sender<([u8; 20], mpsc::Sender<NewPeer>)>,
}

impl PeerListener {
    pub async fn spawn(port: u16) -> anyhow::Result<Self> {
        let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let listener = utils::bind_tcp_listener(addr).await?;
        let (tx, mut rx) = mpsc::channel(100);
        tokio::spawn(async move {
            let mut map: HashMap<[u8; 20], mpsc::Sender<NewPeer>> = HashMap::new();
            loop {
                tokio::select! {
                    Ok((socket,ip)) = listener.accept() => {
                        let timeout_duration = Duration::from_secs(3);
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
                                    tracing::warn!(?info_hash, "Peer {ip} connected but torrent does not exist", );
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

    pub async fn subscribe(&self, info_hash: [u8; 20], sender: mpsc::Sender<NewPeer>) {
        self.new_torrent_channel
            .send((info_hash, sender))
            .await
            .unwrap();
    }
}

#[derive(Debug)]
pub struct Client {
    config: ClientConfig,
    peer_listener: PeerListener,
    udp_tracker_tx: UdpTrackerChannel,
}

impl Client {
    pub async fn new(config: ClientConfig) -> anyhow::Result<Self> {
        let peer_listener = PeerListener::spawn(config.port).await?;
        let udp_listener_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, config.udp_listener_port);
        let udp_channel = UdpTrackerWorker::spawn(udp_listener_addr).await?;

        Ok(Self {
            config,
            peer_listener,
            udp_tracker_tx: udp_channel,
        })
    }

    pub async fn download(
        &self,
        save_location: impl AsRef<Path>,
        mut torrent: Torrent,
        progress_consumer: impl ProgressConsumer,
    ) -> anyhow::Result<DownloadHandle> {
        let hash = torrent.info.hash();
        self.peer_listener
            .subscribe(hash, torrent.new_peers_tx.clone())
            .await;
        let storage =
            TorrentStorage::new(&torrent.info, save_location, StorageMethod::Preallocated);
        let storage_handle = storage.spawn().await?;
        torrent.tracker_set.detach_all();
        let download = Download::new(storage_handle, torrent.info, torrent.new_peers_rx).await;
        let download_handle = download.start(progress_consumer, torrent.peers);
        Ok(download_handle)
    }

    pub fn torrent_file_info(&self, path: impl AsRef<Path>) -> anyhow::Result<Info> {
        let file = TorrentFile::from_path(path)?;
        Ok(file.info)
    }

    pub async fn from_magnet_link(&self, magnet_link: MagnetLink) -> anyhow::Result<Torrent> {
        let info_hash = magnet_link.hash();
        let Some(tracker_list) = magnet_link.announce_list else {
            anyhow::bail!("Magnet links without annonuce list are not yet supported");
        };
        let (_, rx) = watch::channel(DownloadStat::empty(0));
        let (peers_tx, mut peers_rx) = mpsc::channel(1000);
        tracing::info!("Connecting trackers");
        let trackers_announce_handle = tokio::spawn(announce_trackers(
            tracker_list,
            info_hash,
            self.udp_tracker_tx.clone(),
            rx,
            peers_tx.clone(),
        ));
        let mut peers_set = JoinSet::new();

        tracing::info!("Waiting for new peers");
        let peers_semaphore = Arc::new(Semaphore::new(200));
        let mut peers = Vec::new();
        let mut info: Option<Info> = None;
        loop {
            let peers_semaphore = peers_semaphore.clone();
            tokio::select! {
                Some(new_peer) = peers_rx.recv() => {
                    match new_peer {
                        NewPeer::ListenerOrigin(_) => unreachable!("We are not listening udp socket yet"),
                        NewPeer::TrackerOrigin(addr) => {
                            peers_set.spawn(async move {
                                let _permit = peers_semaphore.acquire().await.unwrap();
                                timeout(
                                    Duration::from_secs(5),
                                    Peer::new_from_ip(addr, info_hash),
                                ).await
                            });
                        },
                    }
                },
                Some(Ok(info)) = peers_set.join_next() => {
                    match info {
                        Ok(Ok(peer)) => {
                            peers.push(peer);
                        },
                        Ok(Err(e)) => tracing::error!("Failed to construct peer: {e}"),
                        Err(_) => tracing::warn!("Failed to construct peer: Timed out"),
                    };
                    if peers_set.is_empty() {
                        break;
                    }
                },
                else => {
                    break;
                }
            };
        }
        let tracker_set = trackers_announce_handle.await.unwrap();
        tracing::info!("Fetching ut metadata");
        for peer in &mut peers {
            if peer.supports_ut_metadata() {
                match timeout(Duration::from_secs(3), peer.fetch_ut_metadata()).await {
                    Ok(Ok(i)) => {
                        info = Some(i);
                        break;
                    }
                    Ok(Err(e)) => {
                        tracing::error!("Failed to fetch ut_metadata: {e}");
                    }
                    Err(_) => {
                        tracing::error!("Failed to fetch ut_metadata: Timeout");
                    }
                }
            }
        }
        if let Some(info) = info {
            Ok(Torrent {
                info,
                peers,
                tracker_set,
                new_peers_tx: peers_tx,
                new_peers_rx: peers_rx,
            })
        } else {
            Err(anyhow::anyhow!("failed to fetch info from peers"))
        }
    }

    pub async fn from_torrent_file(&self, file: TorrentFile) -> anyhow::Result<Torrent> {
        let info_hash = file.info.hash();
        let tracker_list = file.all_trackers();
        if tracker_list.is_empty() {
            anyhow::bail!("Magnet links without annonuce list are not yet supported");
        };
        let (_, rx) = watch::channel(DownloadStat::empty(file.info.total_size()));
        let (peers_tx, peers_rx) = mpsc::channel(1000);
        tracing::info!("Connecting trackers");
        let tracker_set = announce_trackers(
            tracker_list,
            info_hash,
            self.udp_tracker_tx.clone(),
            rx,
            peers_tx.clone(),
        )
        .await;

        Ok(Torrent {
            info: file.info,
            new_peers_tx: peers_tx,
            new_peers_rx: peers_rx,
            peers: Vec::new(),
            tracker_set,
        })
    }
}

#[derive(Debug)]
pub struct Torrent {
    pub info: Info,
    pub tracker_set: JoinSet<anyhow::Result<()>>,
    pub new_peers_tx: mpsc::Sender<NewPeer>,
    pub new_peers_rx: mpsc::Receiver<NewPeer>,
    pub peers: Vec<Peer>,
}

impl Torrent {
    pub fn info_hash(&self) -> [u8; 20] {
        self.info.hash()
    }

    pub async fn verify_integrity(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        verify_integrety(path, &self.info).await
    }
}

async fn announce_trackers(
    urls: Vec<Url>,
    info_hash: [u8; 20],
    tracker_tx: UdpTrackerChannel,
    progress: watch::Receiver<DownloadStat>,
    peer_tx: mpsc::Sender<NewPeer>,
) -> JoinSet<anyhow::Result<()>> {
    let mut tracker_init_set = JoinSet::new();
    let mut tracker_set = JoinSet::new();
    for tracker in urls {
        let Ok(tracker_type) = TrackerType::from_url(tracker.clone(), tracker_tx.clone()) else {
            continue;
        };
        tracker_init_set.spawn(timeout(
            Duration::from_secs(5),
            Tracker::new(
                info_hash,
                tracker_type,
                tracker.clone(),
                progress.clone(),
                peer_tx.clone(),
            ),
        ));
    }
    while let Some(tracker_result) = tracker_init_set.join_next().await {
        match tracker_result {
            Ok(Ok(Ok(tracker))) => {
                tracing::info!("Connected to the tracker: {}", tracker.url);
                tracker_set.spawn(tracker.work());
            }
            Ok(Ok(Err(e))) => {
                tracing::error!("Failed to construct tracker: {}", e);
            }
            Ok(Err(_)) => {
                tracing::error!("Failed to connect tracker: Timeout");
            }
            Err(e) => {
                tracing::error!("Tracker task paniced: {e}");
            }
        };
    }
    tracker_set
}
