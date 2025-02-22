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
pub use download::{DownloadProgress, ProgressConsumer};
use peer_listener::{NewPeer, PeerListener};
use peers::Peer;
use reqwest::Url;
pub use resumability::DownloadParams;
use storage::{parts::PartsFile, TorrentStorage};
use tokio::{
    net::TcpStream,
    sync::{mpsc, Semaphore},
    task::JoinSet,
    time::timeout,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracker::{DownloadTracker, TrackerResponse, TrackerType, UdpTrackerChannel, UdpTrackerWorker};

use crate::{
    download::Download,
    tracker::{DownloadStat, Tracker},
};

mod download;
mod file;
mod peer_listener;
mod peers;
mod piece_picker;
mod protocol;
mod resumability;
mod scheduler;
#[allow(unused)]
mod seeder;
mod storage;
mod tracker;
mod utils;

pub use download::DownloadHandle;
pub use download::DownloadMessage;
pub use download::DownloadState;
pub use download::FullState;
pub use download::FullStateFile;
pub use download::FullStatePeer;
pub use download::FullStateTracker;
pub use download::PeerDownloadStats;
pub use download::PeerStateChange;
pub use download::StateChange;
pub use download::Status;
pub use file::MagnetLink;
pub use file::TorrentFile;
pub use peers::BitField;
pub use piece_picker::Priority;
pub use piece_picker::ScheduleStrategy;
pub use protocol::Info;
pub use protocol::OutputFile;
pub use tracker::TrackerStatus;

#[derive(Debug)]
pub struct ClientConfig {
    pub port: u16,
    pub udp_listener_port: u16,
    pub cancellation_token: Option<CancellationToken>,
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
pub struct Client {
    peer_listener: PeerListener,
    udp_tracker_tx: UdpTrackerChannel,
    cancellation_token: CancellationToken,
    task_tracker: TaskTracker,
}

impl Client {
    pub async fn new(config: ClientConfig) -> anyhow::Result<Self> {
        let cancellation_token = config.cancellation_token.clone().unwrap_or_default();
        let task_tracker = TaskTracker::new();
        let peer_listener =
            PeerListener::spawn(config.port, &task_tracker, cancellation_token.clone()).await?;
        let udp_listener_addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, config.udp_listener_port);
        let udp_worker =
            UdpTrackerWorker::bind(udp_listener_addr, cancellation_token.clone()).await?;
        let udp_tracker_channel = udp_worker.spawn().await?;

        Ok(Self {
            peer_listener,
            udp_tracker_tx: udp_tracker_channel,
            cancellation_token,
            task_tracker,
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

        self.peer_listener.subscribe(hash, peers_tx.clone()).await;
        let parts_file = PartsFile::init(&params.info, &params.save_location).await?;
        let storage = TorrentStorage::new(feedback_tx, parts_file, params.clone());
        let storage_handle = storage
            .spawn(&self.task_tracker, child_token.clone())
            .await?;

        let download = Download::new(
            feedback_rx,
            storage_handle,
            params,
            peers_rx,
            trackers,
            child_token,
        );
        let download_handle = download.start(progress_consumer, &self.task_tracker);
        Ok(download_handle)
    }

    pub async fn validate(&self, params: DownloadParams) -> anyhow::Result<BitField> {
        let (feedback_tx, _) = mpsc::channel(100);
        let parts_file = PartsFile::init(&params.info, &params.save_location).await?;
        let mut storage = TorrentStorage::new(feedback_tx, parts_file, params);
        storage.revalidate().await
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
                let cancellation_token = self.cancellation_token.clone();
                tracker_set.spawn(async move {
                    let (_, mut tracker) = Tracker::new(
                        info_hash,
                        tracker_type,
                        tracker_url,
                        downloaded,
                        response_tx,
                        cancellation_token,
                    );
                    tracker.announce().await?;
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
            let cancellation_token = cancellation_token.clone();
            let (handle, mut tracker) = DownloadTracker::new(
                info_hash,
                tracker_type,
                url,
                initial_progress,
                cancellation_token,
            );
            tracing::info!("Started tracker: {}", tracker.url);
            task_tracker.spawn(async move {
                match tracker.work().await {
                    Ok(_) => tracing::info!(url = %tracker.url, "Gracefully stopped tracker"),
                    Err(e) => tracing::warn!(url = %tracker.url, "Tracker errored: {e}"),
                };
            });
            handles.push(handle);
        }
    }
    handles
}
