use torrent::progress::{events, full};

use crate::utils;

impl From<torrent::StorageError> for super::StorageError {
    fn from(value: torrent::StorageError) -> Self {
        match value {
            torrent::StorageError::Fs(e) => Self::Fs(e.to_string()),
            torrent::StorageError::Hash => Self::Hash,
            torrent::StorageError::Bounds => Self::Bounds,
            torrent::StorageError::MissingPiece => {
                unreachable!("missing piece can't be the reason why storage failed")
            }
        }
    }
}

impl From<torrent::DownloadError> for super::DownloadError {
    fn from(value: torrent::DownloadError) -> Self {
        match value {
            torrent::DownloadError::Storage(e) => Self::Storage(e.into()),
        }
    }
}

impl From<torrent::PeerStateChange> for super::PeerStateChange {
    fn from(
        torrent::PeerStateChange {
            downloaded,
            uploaded,
            upload_speed,
            download_speed,
            in_choked,
            in_interested,
            out_choked,
            out_interested,
        }: torrent::PeerStateChange,
    ) -> Self {
        Self {
            downloaded,
            uploaded,
            upload_speed,
            download_speed,
            in_choked,
            in_interested,
            out_choked,
            out_interested,
        }
    }
}

impl From<torrent::DownloadState> for super::DownloadState {
    fn from(value: torrent::DownloadState) -> Self {
        match value {
            torrent::DownloadState::Error(e) => Self::Error { error: e.into() },
            torrent::DownloadState::Validation { .. } => Self::Validation,
            torrent::DownloadState::Paused => Self::Paused,
            torrent::DownloadState::Pending => Self::Pending,
            torrent::DownloadState::Seeding => Self::Seeding,
        }
    }
}

impl From<full::FullStatePeer> for super::StatePeer {
    fn from(value: torrent::FullStatePeer) -> Self {
        Self {
            addr: value.addr.to_string(),
            uploaded: value.uploaded,
            upload_speed: value.upload_speed,
            downloaded: value.downloaded,
            download_speed: value.download_speed,
            in_status: value.in_status.into(),
            out_status: value.out_status.into(),
            interested_amount: value.interested_amount,
            pending_blocks_amount: value.pending_blocks_amount,
            client_name: value.client_name,
        }
    }
}

impl From<torrent::TrackerStatus> for super::TrackerStatus {
    fn from(value: torrent::TrackerStatus) -> Self {
        match value {
            torrent::TrackerStatus::Working => Self::Working,
            torrent::TrackerStatus::NotContacted => Self::NotContacted,
            torrent::TrackerStatus::Error(message) => Self::Error { message },
        }
    }
}

impl From<torrent::FullStateTracker> for super::StateTracker {
    fn from(value: torrent::FullStateTracker) -> Self {
        Self {
            url: value.url,
            announce_interval: value.announce_interval,
            status: value.status.into(),
        }
    }
}

impl From<torrent::FullStateFile> for super::StateFile {
    fn from(value: torrent::FullStateFile) -> Self {
        Self {
            index: value.index,
            size: value.size,
            start_piece: value.start_piece,
            end_piece: value.end_piece,
            path: super::path_components(value.path),
            priority: value.priority.into(),
        }
    }
}

impl From<torrent::FullSessionStats> for super::SessionStats {
    fn from(
        torrent::FullSessionStats {
            download_speed,
            upload_speed,
            connected_peers,
        }: torrent::FullSessionStats,
    ) -> Self {
        Self {
            download_speed,
            upload_speed,
            connected_peers,
        }
    }
}

impl From<torrent::FullSessionState> for super::SessionState {
    fn from(
        torrent::FullSessionState {
            session_stats,
            torrents,
        }: torrent::FullSessionState,
    ) -> Self {
        Self {
            session_stats: session_stats.into(),
            torrents: torrents.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<torrent::FullState> for super::TorrentState {
    fn from(value: torrent::FullState) -> Self {
        let downloaded_pieces = value
            .bitfield
            .all_pieces(value.total_pieces)
            .into_iter()
            .collect();
        Self {
            info_hash: crate::utils::stringify_info_hash(&value.info_hash),
            name: value.name,
            total_pieces: value.total_pieces,
            percent: value.percent,
            download_speed: value.download_speed,
            upload_speed: value.upload_speed,
            total_size: value.total_size,
            trackers: value.trackers.into_iter().map(Into::into).collect(),
            peers: value.peers.into_iter().map(Into::into).collect(),
            files: value.files.into_iter().map(Into::into).collect(),
            downloaded_pieces,
            state: value.state.into(),
            pending_pieces: value.pending_pieces,
        }
    }
}

// EVENT CONVERSIONS

impl From<torrent::progress::SessionUpdate> for super::SessionUpdate {
    fn from(
        torrent::progress::SessionUpdate {
            connected_peers,
            download_speed,
            upload_speed,
        }: torrent::progress::SessionUpdate,
    ) -> Self {
        Self {
            connected_peers,
            download_speed,
            upload_speed,
        }
    }
}

impl From<torrent::progress::Progress> for super::Progress {
    fn from(
        torrent::progress::Progress {
            session_update,
            changed_torrents,
            tick_num,
        }: torrent::progress::Progress,
    ) -> Self {
        Self {
            session_update: session_update.map(Into::into),
            changed_torrents: changed_torrents.into_iter().map(Into::into).collect(),
            tick_num,
        }
    }
}

impl From<torrent::progress::TorrentUpdate> for super::TorrentUpdate {
    fn from(
        torrent::progress::TorrentUpdate {
            events,
            download_speed,
            upload_speed,
            total_downloaded,
            total_uploaded,
            state,
            info_hash,
        }: torrent::progress::TorrentUpdate,
    ) -> Self {
        Self {
            events: events.into_inner().into_iter().map(Into::into).collect(),
            download_speed,
            upload_speed,
            total_downloaded,
            total_uploaded,
            state: state.into(),
            info_hash,
        }
    }
}

impl From<torrent::ProgressEvent> for super::ProgressEvent {
    fn from(value: torrent::ProgressEvent) -> Self {
        match value {
            events::ProgressEvent::Peer(peer_event) => Self::Peer(peer_event.into()),
            events::ProgressEvent::State(torrent_state_change) => {
                Self::State(torrent_state_change.into())
            }
            events::ProgressEvent::Tracker(tracker_event) => Self::Tracker(tracker_event.into()),
            events::ProgressEvent::StoragePiece(storage_piece_event) => {
                Self::StoragePiece(storage_piece_event.into())
            }
            events::ProgressEvent::StorageFile(storage_file_event) => {
                Self::StorageFile(storage_file_event.into())
            }
            events::ProgressEvent::Session(session_event) => Self::Session(session_event.into()),
        }
    }
}

impl From<events::PeerEvent> for super::PeerEvent {
    fn from(events::PeerEvent { ip, kind }: events::PeerEvent) -> Self {
        Self {
            ip: ip.to_string(),
            peer_event: kind.into(),
        }
    }
}

impl From<events::PeerEventKind> for super::PeerEventKind {
    fn from(value: events::PeerEventKind) -> Self {
        match value {
            events::PeerEventKind::StatUpdate(peer_state_change) => {
                Self::StatUpdate(peer_state_change.into())
            }
            events::PeerEventKind::Disconnect => Self::Disconnect,
            events::PeerEventKind::Connect { state } => Self::Connect {
                state: (*state).into(),
            },
        }
    }
}

impl From<events::TorrentStateChange> for super::TorrentStateChange {
    fn from(events::TorrentStateChange(state): events::TorrentStateChange) -> Self {
        Self {
            state: state.into(),
        }
    }
}

impl From<events::TrackerEvent> for super::TrackerEvent {
    fn from(events::TrackerEvent { kind, url }: events::TrackerEvent) -> Self {
        Self {
            tracker_event: kind.into(),
            url,
        }
    }
}

impl From<events::TrackerEventKind> for super::TrackerEventKind {
    fn from(value: events::TrackerEventKind) -> Self {
        match value {
            events::TrackerEventKind::Reannounce { interval } => Self::Reannounce { interval },
            events::TrackerEventKind::Failed { reason } => Self::Failed { reason },
        }
    }
}

impl From<events::StoragePieceEvent> for super::StoragePieceEvent {
    fn from(events::StoragePieceEvent { piece, kind }: events::StoragePieceEvent) -> Self {
        Self {
            piece,
            piece_event: kind.into(),
        }
    }
}

impl From<events::StoragePieceEventKind> for super::StoragePieceEventKind {
    fn from(value: events::StoragePieceEventKind) -> Self {
        match value {
            events::StoragePieceEventKind::Validated => Self::Validated,
            events::StoragePieceEventKind::HashFailed => Self::HashFailed,
            events::StoragePieceEventKind::SaveFailed => Self::SaveFailed,
            events::StoragePieceEventKind::Finished => Self::Finished,
        }
    }
}

impl From<events::StorageFileEvent> for super::StorageFileEvent {
    fn from(events::StorageFileEvent { idx, kind }: events::StorageFileEvent) -> Self {
        Self {
            idx,
            file_event: kind.into(),
        }
    }
}

impl From<events::StorageFileEventKind> for super::StorageFileEventKind {
    fn from(value: events::StorageFileEventKind) -> Self {
        match value {
            events::StorageFileEventKind::PriorityChange(priority) => Self::PriorityChange {
                priority: priority.into(),
            },
        }
    }
}

impl From<events::SessionEvent> for super::SessionEvent {
    fn from(value: events::SessionEvent) -> Self {
        match value {
            events::SessionEvent::TorrentAdd(full_state) => Self::TorrentAdd {
                state: super::TorrentState::from(*full_state),
            },
            events::SessionEvent::TorrentRemove { info_hash } => Self::TorrentRemove {
                info_hash: utils::stringify_info_hash(&info_hash),
            },
        }
    }
}

impl From<torrent::Priority> for super::Priority {
    fn from(value: torrent::Priority) -> Self {
        match value {
            torrent::Priority::Disabled => Self::Disabled,
            torrent::Priority::Low => Self::Low,
            torrent::Priority::Medium => Self::Medium,
            torrent::Priority::High => Self::High,
        }
    }
}

impl From<super::Priority> for torrent::Priority {
    fn from(val: super::Priority) -> Self {
        match val {
            super::Priority::Disabled => torrent::Priority::Disabled,
            super::Priority::Low => torrent::Priority::Low,
            super::Priority::Medium => torrent::Priority::Medium,
            super::Priority::High => torrent::Priority::High,
        }
    }
}

impl From<super::Action> for torrent::Action {
    fn from(value: super::Action) -> Self {
        match value {
            super::Action::Validate => Self::Validate,
            super::Action::Abort => Self::Abort,
            super::Action::Resume => Self::Resume,
            super::Action::Pause => Self::Pause,
        }
    }
}
