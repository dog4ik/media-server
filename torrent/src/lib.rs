#![feature(array_chunks)]
#![feature(iter_repeat_n)]
#![feature(ip_bits)]
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

pub use download::{DownloadProgress, ProgressConsumer};
use file::{MagnetLink, TorrentFile};
use peers::Peer;
use protocol::{tracker::UdpTrackerRequest, Info};
use reqwest::Url;
use storage::verify_integrety;
use tokio::{
    sync::{broadcast, mpsc, watch, Semaphore},
    task::{JoinHandle, JoinSet},
    time::timeout,
};
use tracker::{TrackerType, UdpTrackerWorker};
use uuid::Uuid;

use crate::{
    download::Download,
    tracker::{DownloadStat, Tracker},
};

mod download;
mod file;
mod peers;
mod protocol;
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
    TrackerOrigin(SocketAddr),
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

    pub async fn subscribe(&self, info_hash: [u8; 20], sender: mpsc::Sender<NewPeer>) {
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

// TODO: cancel, pause and other control
#[derive(Debug)]
pub struct DownloadHandle {
    pub handle: JoinHandle<anyhow::Result<()>>,
}

impl DownloadHandle {
    pub fn abort(&self) {
        self.handle.abort()
    }
}

#[derive(Debug)]
pub struct Client {
    config: ClientConfig,
    torrents: Vec<Torrent>,
    peer_listener: PeerListener,
    progress_channel: broadcast::Sender<DownloadStat>,
    udp_tracker_tx: mpsc::Sender<UdpTrackerRequest>,
}

impl Client {
    pub async fn new(config: ClientConfig) -> anyhow::Result<Self> {
        let peer_listener = PeerListener::spawn(config.port).await?;
        let (progress_channel, _) = broadcast::channel(10);
        let worker = UdpTrackerWorker::new("0.0.0.0:7897".parse()?).await?;

        Ok(Self {
            config,
            torrents: Vec::new(),
            peer_listener,
            progress_channel,
            udp_tracker_tx: worker.request_tx.clone(),
        })
    }

    pub async fn download(
        &self,
        save_location: impl AsRef<Path>,
        torrent: Torrent,
        progress_consumer: impl ProgressConsumer,
    ) -> anyhow::Result<DownloadHandle> {
        let (new_peer_tx, new_peer_rx) = mpsc::channel(100);
        let hash = torrent.info.hash();
        self.peer_listener
            .subscribe(hash, new_peer_tx.clone())
            .await;
        let save_location_metadata = save_location.as_ref().metadata()?;
        if !save_location_metadata.is_dir() {
            return Err(anyhow::anyhow!(
                "Save directory must be a directory, got {:?}",
                save_location_metadata.file_type()
            ));
        }
        let download = Download::new(save_location, torrent.info, new_peer_rx).await;
        let handle = tokio::spawn(download.start(progress_consumer, torrent.peers));
        let download_handle = DownloadHandle { handle };
        Ok(download_handle)
    }

    pub fn torrent_file_info(&self, path: impl AsRef<Path>) -> anyhow::Result<Info> {
        let file = TorrentFile::from_path(path)?;
        Ok(file.info)
    }

    pub async fn from_magnet_link(&self, magnet_link: MagnetLink) -> anyhow::Result<Torrent> {
        let info_hash = magnet_link.hash();
        let Some(tracker_list) = magnet_link.announce_list else {
            return Err(anyhow::anyhow!(
                "Magnet links without annonuce list are not yet supported"
            ));
        };
        // Dont care about stats, need only ut_metadata
        let (_, rx) = watch::channel(DownloadStat::empty(0));
        let (peers_tx, mut peers_rx) = mpsc::channel(1000);
        tracing::info!("Connecting trackers");
        let trackers_announce_handle = tokio::spawn(announce_trackers(
            tracker_list,
            info_hash,
            self.udp_tracker_tx.clone(),
            rx,
            peers_tx,
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
                                    Duration::from_secs(3),
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
                        Err(_) => tracing::error!("Failed to construct peer: Timed out"),
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
        let tracker_set = trackers_announce_handle.await.unwrap().unwrap();
        tracing::info!("Fetching ut metadata");
        for peer in &mut peers {
            if peer.supports_ut_metadata() {
                match timeout(Duration::from_secs(3), peer.fetch_ut_metadata()).await {
                    Ok(Ok(i)) => {
                        dbg!(&i.name);
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
            })
        } else {
            Err(anyhow::anyhow!("failed to fetch info from peers"))
        }
    }
}

#[derive(Debug)]
pub struct Torrent {
    pub info: Info,
    pub tracker_set: JoinSet<anyhow::Result<()>>,
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
    tracker_tx: mpsc::Sender<UdpTrackerRequest>,
    progress: watch::Receiver<DownloadStat>,
    peer_tx: mpsc::Sender<NewPeer>,
) -> anyhow::Result<JoinSet<anyhow::Result<()>>> {
    let mut tracker_init_set = JoinSet::new();
    let mut tracker_set = JoinSet::new();
    for tracker in urls {
        let tracker_type = TrackerType::from_url(tracker.clone(), tracker_tx.clone())?;
        tracker_init_set.spawn(timeout(
            Duration::from_secs(3),
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
    Ok(tracker_set)
}
