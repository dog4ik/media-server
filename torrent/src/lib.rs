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

use anyhow::bail;
use download::DownloadHandle;
pub use download::{DownloadProgress, ProgressConsumer};
use file::MagnetLink;
use peers::Peer;
use protocol::Info;
use reqwest::Url;
use storage::{StorageMethod, TorrentStorage};
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tokio_util::sync::CancellationToken;
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
    cancellation_token: Option<CancellationToken>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            port: 6881,
            udp_listener_port: 7897,
            cancellation_token: Some(CancellationToken::new()),
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
    cancellation_token: CancellationToken,
}

impl Client {
    pub async fn new(config: ClientConfig) -> anyhow::Result<Self> {
        let peer_listener = PeerListener::spawn(config.port).await?;
        let udp_listener_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, config.udp_listener_port);
        let cancellation_token = config.cancellation_token.clone().unwrap_or_default();
        let udp_worker =
            UdpTrackerWorker::bind(udp_listener_addr, cancellation_token.clone()).await?;
        let udp_tracker_channel = udp_worker.spawn().await?;

        Ok(Self {
            config,
            peer_listener,
            udp_tracker_tx: udp_tracker_channel,
            cancellation_token,
        })
    }

    pub async fn download(
        &self,
        save_location: impl AsRef<Path>,
        trackers: Vec<Url>,
        info: Info,
        progress_consumer: impl ProgressConsumer,
    ) -> anyhow::Result<DownloadHandle> {
        let child_cancellation_token = self.cancellation_token.child_token();
        let hash = info.hash();
        // TODO: Trackers should know download/upload stats
        let (_, rx) = watch::channel(DownloadStat::empty(info.total_size()));
        let (peers_tx, peers_rx) = mpsc::channel(1000);
        tracing::info!("Connecting trackers");
        let mut tracker_set = spawn_trackers(
            trackers,
            hash,
            self.udp_tracker_tx.clone(),
            rx,
            peers_tx.clone(),
            child_cancellation_token.clone(),
        )
        .await;

        self.peer_listener.subscribe(hash, peers_tx.clone()).await;
        let storage = TorrentStorage::new(&info, save_location, StorageMethod::Preallocated);
        let storage_handle = storage.spawn().await?;
        tracker_set.detach_all();
        let download =
            Download::new(storage_handle, info, peers_rx, child_cancellation_token).await;
        let download_handle = download.start(progress_consumer);
        Ok(download_handle)
    }

    pub async fn resolve_magnet_link(&self, link: &MagnetLink) -> anyhow::Result<Info> {
        let info_hash = link.hash();
        let Some(ref tracker_list) = link.announce_list else {
            bail!("magnet links without announce list are not supported yet");
        };
        let (new_peers_tx, mut new_peers_rx) = mpsc::channel(100);
        // don't care about download stats
        let (_, download_rx) = watch::channel(DownloadStat::empty(0));
        let mut tracker_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        let mut ut_metadata_set: JoinSet<anyhow::Result<Info>> = JoinSet::new();
        for tracker_url in tracker_list.clone() {
            let tracker_type = TrackerType::from_url(&tracker_url, self.udp_tracker_tx.clone())
                .expect("http | udp | https scheme");
            {
                let new_peers_tx = new_peers_tx.clone();
                let download_rx = download_rx.clone();
                tracker_set.spawn(async move {
                    let mut tracker = Tracker::new(
                        info_hash,
                        tracker_type,
                        tracker_url,
                        download_rx,
                        new_peers_tx,
                    )
                    .await?;
                    let announce = tracker.announce().await?;
                    tracker.handle_announce(announce).await;
                    Ok(())
                });
            }
        }
        let peer_semaphore = Arc::new(Semaphore::new(100));
        let duration = Duration::from_secs(2);
        loop {
            let peer_semaphore = peer_semaphore.clone();
            tokio::select! {
                Some(NewPeer::TrackerOrigin(addr)) = new_peers_rx.recv() => {
                    ut_metadata_set.spawn(async move {
                        let _lock = peer_semaphore.acquire().await;
                        let socket = timeout(duration, TcpStream::connect(addr)).await??;
                        let mut peer = timeout(duration, Peer::new(socket, info_hash)).await??;
                        let metadata = timeout(Duration::from_secs(5), peer.fetch_ut_metadata()).await??;
                        Ok(metadata)
                    });
                }
                Some(join) = ut_metadata_set.join_next() => {
                    match join {
                        Ok(Ok(info)) => return Ok(info),
                        Ok(Err(e)) => {
                            if ut_metadata_set.is_empty() {
                                bail!("No one managed to send metadata");
                            }
                            tracing::warn!("ut_metadata retrival task failed: {e}");
                        },
                        Err(e) => {
                            if ut_metadata_set.is_empty() {
                                bail!("No one managed to send metadata");
                            }
                            tracing::error!("ut_metadata retrieval task paniced: {e}");
                        },
                    }
                }
            }
        }
    }
}

async fn spawn_trackers(
    urls: Vec<Url>,
    info_hash: [u8; 20],
    tracker_tx: UdpTrackerChannel,
    progress: watch::Receiver<DownloadStat>,
    peer_tx: mpsc::Sender<NewPeer>,
    cancellation_token: CancellationToken,
) -> JoinSet<anyhow::Result<()>> {
    let mut tracker_init_set = JoinSet::new();
    let mut tracker_set = JoinSet::new();
    for tracker in urls {
        let Ok(tracker_type) = TrackerType::from_url(&tracker, tracker_tx.clone()) else {
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
                tracker_set.spawn(tracker.work(cancellation_token.clone()));
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
