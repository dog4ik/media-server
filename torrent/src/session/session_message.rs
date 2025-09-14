use tokio::sync::mpsc;

use crate::{DownloadHandle, FullSessionState, Priority, ScheduleStrategy, download::Download};

#[derive(Debug)]
pub enum TorrentStateRequest {
    /// Useful for paginated requests
    Ranged(std::ops::Range<usize>),
    All,
    Single([u8; 20]),
}

#[derive(Debug)]
pub enum Action {
    Validate,
    Abort,
    Resume,
    Pause,
}

#[derive(Debug)]
pub enum SessionMessage {
    AddTorrent(Box<crate::download::Download>),
    SetStrategy {
        torrent: [u8; 20],
        strategy: ScheduleStrategy,
    },
    SetFilePriority {
        torrent: [u8; 20],
        file_idx: usize,
        priority: Priority,
    },
    PostFullState {
        tx: tokio::sync::oneshot::Sender<FullSessionState>,
        request: TorrentStateRequest,
    },
    PerformAction {
        torrents: Vec<[u8; 20]>,
        action: Action,
    },
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    tx: mpsc::Sender<SessionMessage>,
}

impl SessionHandle {
    pub(crate) fn new(tx: mpsc::Sender<SessionMessage>) -> Self {
        Self { tx }
    }

    pub async fn send(&self, message: SessionMessage) {
        self.tx
            .send(message)
            .await
            .expect("session is always available");
    }

    pub async fn add_torrent(&self, torrent: Download) -> DownloadHandle {
        let handle = torrent.make_handle();
        self.send(SessionMessage::AddTorrent(Box::new(torrent)))
            .await;
        handle
    }

    pub async fn remove_torrent(&self, torrent: [u8; 20]) {
        self.send(SessionMessage::PerformAction {
            torrents: vec![torrent],
            action: Action::Abort,
        })
        .await;
    }

    pub async fn fetch_progress(&self, request: TorrentStateRequest) -> FullSessionState {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.send(SessionMessage::PostFullState { tx, request })
            .await;
        rx.await.expect("session is available")
    }
}
