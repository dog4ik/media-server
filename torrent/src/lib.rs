#![feature(array_chunks)]
#![feature(assert_matches)]
#![feature(iter_array_chunks)]
#![feature(iter_collect_into)]

use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddrV4},
    sync::Arc,
    time::Duration,
};

use anyhow::bail;
use peer_listener::PeerListener;
use peers::Peer;
use reqwest::Url;
pub use resumability::DownloadParams;
use session::SessionContext;
use storage::{TorrentStorage, parts::PartsFile};
use tokio::{
    net::TcpStream,
    sync::{Semaphore, mpsc},
    task::JoinSet,
    time::timeout,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracker::{DownloadTracker, TrackerResponse, TrackerType, UdpTrackerChannel, UdpTrackerWorker};

use crate::{
    download::Download,
    tracker::{DownloadStat, Tracker},
};

/// Basic bitfield implementation
mod bitfield;
/// Event loop of the download
mod download;
/// Torrent file parsing
mod file;
/// Magnet link parsing
mod magnet;
/// Tcp listener that accepts incoming peers
mod peer_listener;
/// Peer connection task
mod peers;
/// Strategies for picking next downloaded piece
mod piece_picker;
/// BitTorrent protocol types / implementations
mod protocol;
/// Data used for download resume
mod resumability;
/// State machine that assigns blocks, chokes peers, etc.
mod scheduler;
mod seeder;
/// Client session
mod session;
/// Block saving, files revalidation
mod storage;
/// Http / Udp tracker implementations
mod tracker;
mod utils;

pub use bitfield::BitField;
pub use download::DownloadError;
pub use download::DownloadHandle;
pub use download::DownloadMessage;
pub use download::DownloadState;
pub use download::peer::Status;
pub use download::progress_consumer::DownloadProgress;
pub use download::progress_consumer::FullState;
pub use download::progress_consumer::FullStateFile;
pub use download::progress_consumer::FullStatePeer;
pub use download::progress_consumer::FullStateTracker;
pub use download::progress_consumer::PeerDownloadStats;
pub use download::progress_consumer::PeerStateChange;
pub use download::progress_consumer::ProgressConsumer;
pub use download::progress_consumer::StateChange;
pub use file::TorrentFile;
pub use magnet::MagnetLink;
pub use piece_picker::Priority;
pub use piece_picker::ScheduleStrategy;
pub use protocol::Info;
pub use protocol::OutputFile;
pub use storage::StorageError;
pub use storage::StorageErrorKind;
pub use tracker::TrackerStatus;

pub(crate) const CLIENT_NAME: &str = "SkibidiTorrent";
pub(crate) const MAX_PEER_CONNECTIONS: usize = 600;

#[derive(Debug)]
pub struct ClientConfig {
    pub port: u16,
    pub external_ip: Option<SocketAddrV4>,
    pub udp_listener_port: u16,
    pub cancellation_token: Option<CancellationToken>,
    pub upnp_nat_traversal_enabled: bool,
    pub max_peer_connections: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            port: tracker::PORT,
            external_ip: None,
            udp_listener_port: 7897,
            cancellation_token: Some(CancellationToken::new()),
            upnp_nat_traversal_enabled: true,
            max_peer_connections: MAX_PEER_CONNECTIONS,
        }
    }
}

#[derive(Debug)]
pub struct Client {
    ip: Arc<Option<Ipv4Addr>>,
    peer_listener: PeerListener,
    udp_tracker_tx: UdpTrackerChannel,
    cancellation_token: CancellationToken,
    task_tracker: TaskTracker,
    config: ClientConfig,
    session_context: Arc<SessionContext>,
}

impl Client {
    pub async fn new(config: ClientConfig) -> anyhow::Result<Self> {
        let cancellation_token = config.cancellation_token.clone().unwrap_or_default();
        let task_tracker = TaskTracker::new();
        let upnp_client = match config.upnp_nat_traversal_enabled {
            true => utils::search_upnp_gateway().await.ok(),
            false => None,
        };
        // WARN: handle case where we can't resolve client's external ip
        let external_ip = utils::external_ip(upnp_client.as_ref()).await.ok();
        let peer_listener = if let Some(upnp_client) = upnp_client {
            PeerListener::spawn_with_upnp(
                config.port,
                upnp_client,
                &task_tracker,
                cancellation_token.clone(),
            )
            .await
        } else {
            PeerListener::spawn(config.port).await
        }?;
        let udp_listener_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, config.udp_listener_port);
        let udp_worker = UdpTrackerWorker::bind(udp_listener_addr).await?;
        let udp_tracker_channel = udp_worker.spawn().await?;

        Ok(Self {
            ip: Arc::new(external_ip),
            peer_listener,
            udp_tracker_tx: udp_tracker_channel,
            cancellation_token,
            task_tracker,
            session_context: Arc::new(SessionContext::new(config.max_peer_connections)),
            config,
        })
    }

    /// Call cancel on cancellation token and wait until all tasks are closed
    pub async fn shutdown(&self) {
        self.task_tracker.close();
        self.cancellation_token.cancel();
        self.task_tracker.wait().await
    }

    pub async fn open(
        &self,
        params: DownloadParams,
        progress_consumer: impl ProgressConsumer,
    ) -> anyhow::Result<DownloadHandle> {
        let child_token = self.cancellation_token.child_token();
        let hash = params.info.hash();
        let initial_stat = DownloadStat::new(&params.bitfield, &params.info);
        let (peers_tx, peers_rx) = mpsc::channel(1000);
        let (feedback_tx, feedback_rx) = mpsc::channel(100);
        tracing::info!("Connecting trackers");
        let urls = params.trackers.clone();
        let trackers = spawn_trackers(
            urls,
            hash,
            self.udp_tracker_tx.clone(),
            initial_stat,
            self.task_tracker.clone(),
            child_token.clone(),
        )
        .await;

        self.peer_listener.subscribe(hash, peers_tx).await;
        let parts_file = PartsFile::init(&params.info, &params.save_location).await?;
        let storage = TorrentStorage::new(feedback_tx, parts_file, params.clone());
        let storage_handle = storage.spawn(&self.task_tracker).await?;

        let download = Download::new(
            self.session_context.clone(),
            feedback_rx,
            storage_handle,
            params,
            peers_rx,
            trackers,
            child_token,
            self.ip
                .map(|ip| std::net::SocketAddr::V4(SocketAddrV4::new(ip, self.config.port))),
        );
        self.session_context.add_torrent();
        let download_handle = download.start(progress_consumer, &self.task_tracker);
        Ok(download_handle)
    }

    pub async fn validate(&self, params: DownloadParams) -> anyhow::Result<BitField> {
        let (feedback_tx, _) = mpsc::channel(100);
        let parts_file = PartsFile::init(&params.info, &params.save_location).await?;
        let mut storage = TorrentStorage::new(feedback_tx, parts_file, params);
        storage.revalidate().await;
        Ok(storage.bitfield().to_owned())
    }

    pub async fn resolve_magnet_link(&self, link: &MagnetLink) -> anyhow::Result<Info> {
        let info_hash = link.hash();
        let Some(ref tracker_list) = link.announce_list else {
            bail!("magnet links without announce list are not supported yet");
        };
        let (response_tx, mut response_rx) = mpsc::channel(100);
        // don't care about download stats
        let downloaded = DownloadStat::empty(0);
        let mut tracker_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        let mut ut_metadata_set: JoinSet<anyhow::Result<Info>> = JoinSet::new();
        for tracker_url in tracker_list.clone() {
            let tracker_type = TrackerType::from_url(&tracker_url, &self.udp_tracker_tx)?;
            {
                let response_tx = response_tx.clone();
                tracker_set.spawn(async move {
                    let (_, mut tracker) = Tracker::new(
                        info_hash,
                        tracker_type,
                        tracker_url,
                        downloaded,
                        response_tx,
                    );
                    if let Err(e) = tracker.announce().await {
                        tracing::warn!("Failed to announce tracker: {e}");
                    };
                    Ok(())
                });
            }
        }
        let peer_semaphore = Arc::new(Semaphore::new(100));
        let duration = Duration::from_secs(2);
        let mut pending_peers = HashSet::new();
        loop {
            let peer_semaphore = peer_semaphore.clone();
            tokio::select! {
                Some(TrackerResponse::AnnounceResponse { peers, .. }) = response_rx.recv() => {
                    for peer in peers {
                        if pending_peers.insert(peer) {
                            let peer_semaphore = peer_semaphore.clone();
                            ut_metadata_set.spawn(async move {
                                let _lock = peer_semaphore.acquire().await;
                                let socket = timeout(duration, TcpStream::connect(peer)).await??;
                                let mut peer = timeout(duration, Peer::new(socket, info_hash)).await??;
                                let metadata = timeout(Duration::from_secs(5), peer.fetch_ut_metadata()).await??;
                                Ok(metadata)
                            });
                        }
                    }
                }
                Some(join) = ut_metadata_set.join_next() => {
                    match join {
                        Ok(Ok(info)) => return Ok(info),
                        Ok(Err(e)) => {
                            if ut_metadata_set.is_empty() {
                                bail!("No one managed to send metadata");
                            }
                            tracing::warn!("ut_metadata retrieval task failed: {e}");
                        },
                        Err(e) => {
                            panic!("ut_metadata retrieval task panicked: {e}");
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
    initial_progress: DownloadStat,
    task_tracker: TaskTracker,
    cancellation_token: CancellationToken,
) -> Vec<DownloadTracker> {
    let mut handles = Vec::new();
    for url in urls {
        let Ok(tracker_type) = TrackerType::from_url(&url, &tracker_tx) else {
            continue;
        };
        {
            let (handle, mut tracker) =
                DownloadTracker::new(info_hash, tracker_type, url, initial_progress);
            tracing::debug!("Started tracker: {}", tracker.url);
            let cancellation_token = cancellation_token.clone();
            task_tracker.spawn(async move {
                match tracker.work(cancellation_token).await {
                    Ok(_) => tracing::debug!(url = %tracker.url, "Gracefully stopped tracker"),
                    Err(e) => tracing::warn!(url = %tracker.url, "Tracker errored: {e}"),
                };
            });
            handles.push(handle);
        }
    }
    handles
}
