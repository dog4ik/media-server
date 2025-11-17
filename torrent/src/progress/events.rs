use std::net::SocketAddr;

use crate::FullStatePeer;

#[derive(Debug)]
pub struct PeerEvent {
    pub ip: SocketAddr,
    pub kind: PeerEventKind,
}

#[derive(Debug)]
pub enum PeerEventKind {
    StatUpdate(PeerStateChange),
    /// Peer disconnected from the download
    Disconnect,
    Connect {
        state: Box<FullStatePeer>,
    },
}

#[derive(Debug, PartialEq)]
pub struct PeerStateChange {
    pub downloaded: u64,
    pub uploaded: u64,
    pub upload_speed: u64,
    pub download_speed: u64,
    pub in_choked: bool,
    pub in_interested: bool,
    pub out_choked: bool,
    pub out_interested: bool,
}

#[derive(Debug)]
pub struct TorrentStateChange(pub crate::DownloadState);

#[derive(Debug)]
pub struct TorrentTickEvents {
    events: Vec<ProgressEvent>,
}

impl TorrentTickEvents {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    pub fn emit_tracker(&mut self, url: String, kind: TrackerEventKind) {
        self.emit(ProgressEvent::Tracker(TrackerEvent { kind, url }))
    }

    pub fn emit_peer(&mut self, ip: SocketAddr, kind: PeerEventKind) {
        self.emit(ProgressEvent::Peer(PeerEvent { ip, kind }))
    }

    pub fn emit_piece(&mut self, piece: usize, kind: StoragePieceEventKind) {
        self.emit(ProgressEvent::StoragePiece(StoragePieceEvent {
            piece,
            kind,
        }))
    }

    pub fn emit_file(&mut self, idx: usize, kind: StorageFileEventKind) {
        self.emit(ProgressEvent::StorageFile(StorageFileEvent { idx, kind }))
    }

    pub fn emit_state(&mut self, state: TorrentStateChange) {
        self.emit(ProgressEvent::State(state))
    }

    pub fn emit_session(&mut self, session_event: SessionEvent) {
        self.emit(ProgressEvent::Session(session_event))
    }

    pub fn emit(&mut self, event: ProgressEvent) {
        self.events.push(event);
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn drain_events(&mut self, target: &mut Vec<ProgressEvent>) {
        target.reserve(self.events.len());
        std::mem::swap(&mut self.events, target);
    }

    pub fn into_inner(self) -> Vec<ProgressEvent> {
        self.events
    }
}

#[derive(Debug)]
pub struct TrackerEvent {
    /// Full URL of the tracker
    pub url: String,
    pub kind: TrackerEventKind,
}

#[derive(Debug)]
pub enum TrackerEventKind {
    /// Tracker reannounced, contains newly obtained interval
    Reannounce { interval: std::time::Duration },
    /// Tracker failed with error
    Failed { reason: String },
}

#[derive(Debug)]
pub struct StorageFileEvent {
    /// Index of the file
    pub idx: usize,
    pub kind: StorageFileEventKind,
}

#[derive(Debug)]
pub enum StorageFileEventKind {
    /// Fires when the file priority changes
    PriorityChange(crate::Priority),
}

#[derive(Debug)]
pub struct StoragePieceEvent {
    /// Piece index
    pub piece: usize,
    pub kind: StoragePieceEventKind,
}

#[derive(Debug)]
pub enum StoragePieceEventKind {
    Validated,
    HashFailed,
    SaveFailed,
    Finished,
}

#[derive(Debug)]
pub enum SessionEvent {
    TorrentAdd(Box<crate::FullState>),
    TorrentRemove { info_hash: [u8; 20] },
}

#[derive(Debug)]
pub enum ProgressEvent {
    Peer(PeerEvent),
    State(TorrentStateChange),
    Tracker(TrackerEvent),
    StoragePiece(StoragePieceEvent),
    StorageFile(StorageFileEvent),
    Session(SessionEvent),
}
