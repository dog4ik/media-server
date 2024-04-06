use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Display,
    net::SocketAddrV4,
    ops::Range,
    path::Path,
    time::Duration,
};

use anyhow::{anyhow, bail, ensure};
use bytes::{Bytes, BytesMut};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
    time::{timeout, Instant},
};
use uuid::Uuid;

use crate::{
    file::Info,
    peers::{BitField, Peer, PeerError, PeerIPC},
    scheduler::{Scheduler, MAX_PENDING_BLOCKS},
    NewPeer,
};

/// Piece representation where all blocks are sorted
#[derive(Debug, Clone)]
pub struct Piece {
    pub index: u32,
    pub length: u32,
    pub blocks: Vec<(u32, Bytes)>,
}

impl Piece {
    pub fn empty(index: u32, length: u32) -> Self {
        Self {
            index,
            length,
            blocks: Vec::new(),
        }
    }

    pub fn is_full(&self) -> bool {
        self.blocks
            .iter()
            .map(|(_, bytes)| bytes.len() as u32)
            .sum::<u32>()
            == self.length
    }

    pub fn add_block(&mut self, offset: u32, bytes: Bytes) -> anyhow::Result<()> {
        let end_new_block = offset + bytes.len() as u32;
        if end_new_block > self.length {
            bail!("block is bigger then length of the piece")
        }
        if self.blocks.len() == 0 {
            self.blocks.push((offset, bytes));
            return Ok(());
        }
        for (existing_offset, existing_bytes) in &self.blocks {
            let end_existing_block = existing_offset + existing_bytes.len() as u32;

            if offset < end_existing_block && end_new_block > *existing_offset {
                bail!("block conflicts with existing blocks");
            }
        }

        self.blocks
            .iter()
            .position(|(existing_offset, _)| *existing_offset <= offset)
            .map(|index| {
                self.blocks.insert(index, (offset, bytes));
            })
            .ok_or(anyhow!("failed to insert block"))
    }

    pub fn as_bytes(mut self) -> anyhow::Result<Bytes> {
        let length = self.blocks.iter().map(|(_, bytes)| bytes.len()).sum();
        let mut bytes = BytesMut::with_capacity(length);
        self.blocks.sort_by_key(|(offset, _)| *offset);
        for (_, block_bytes) in &self.blocks {
            bytes.extend_from_slice(&block_bytes);
        }
        ensure!(bytes.len() as u32 == self.length);
        Ok(bytes.into())
    }
}

#[derive(Debug)]
pub struct ActivePeer {
    pub ip: SocketAddrV4,
    pub command: mpsc::Sender<PeerCommand>,
    pub bitfield: BitField,
    /// Our status towards peer
    pub out_status: Status,
    /// Peer's status towards us
    pub in_status: Status,
    /// Pending blocks are used if peer panics or chokes, also it indicates that peer is busy
    pub pending_blocks: Vec<Block>,
    /// Amount of bytes downloaded from peer
    pub downloaded: u64,
    /// Amount of bytes uploaded to peer
    pub uploaded: u64,
    /// History of peer's performance
    pub performance_history: VecDeque<Performance>,
}

impl ActivePeer {
    pub fn new(command: mpsc::Sender<PeerCommand>, peer: &Peer) -> Self {
        let choke_status = Status::default();
        Self {
            command,
            ip: peer.ip(),
            bitfield: peer.bitfield.clone(),
            in_status: choke_status.clone(),
            out_status: choke_status,
            pending_blocks: Vec::new(),
            downloaded: 0,
            uploaded: 0,
            performance_history: VecDeque::with_capacity(20),
        }
    }

    pub fn update_performance(&mut self) {
        if self.performance_history.len() >= 20 {
            self.performance_history.pop_front();
        };

        self.performance_history.push_front(Performance {
            downloaded: self.downloaded,
            uploaded: self.uploaded,
            time: Instant::now(),
        });
    }

    /// Peer's download speed in bytes per second
    pub fn download_speed(&self) -> usize {
        let Some(last) = self.performance_history.back() else {
            return 0;
        };
        let elapsed = last.time.elapsed().as_secs_f64();
        let downloaded = (self.downloaded - last.downloaded) as f64;
        (downloaded / elapsed).round() as usize
    }

    /// Peer's upload speed in bytes per second
    pub fn upload_speed(&self) -> usize {
        todo!()
    }

    pub async fn out_choke(&mut self) {
        self.command.try_send(PeerCommand::Choke).unwrap();
        self.out_status.choke();
    }

    pub async fn out_unchoke(&mut self) {
        self.command.try_send(PeerCommand::Unchoke).unwrap();
        self.out_status.choke();
    }

    pub fn in_choke(&mut self) {
        self.in_status.choke();
    }

    pub fn in_unchoke(&mut self) {
        self.in_status.unchoke();
    }
}

#[derive(Debug, Clone)]
pub struct Status {
    choked: bool,
    choked_time: Instant,
    interested: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            choked: true,
            choked_time: Instant::now(),
            interested: false,
        }
    }
}

impl Status {
    pub fn choke(&mut self) {
        self.choked = true;
        self.choked_time = Instant::now();
    }

    pub fn unchoke(&mut self) {
        self.choked = false;
    }

    pub fn is_choked(&self) -> bool {
        self.choked
    }

    pub fn is_interested(&self) -> bool {
        self.interested
    }

    /// Get duration of being choked returing 0 Duration if currently choked
    pub fn choke_duration(&self) -> Duration {
        if self.is_choked() {
            Duration::ZERO
        } else {
            self.choked_time.elapsed()
        }
    }

    pub fn interest(&mut self) {
        self.interested = true;
    }

    pub fn uninterest(&mut self) {
        self.interested = false;
    }
}

#[derive(Debug, Clone)]
pub struct Performance {
    pub downloaded: u64,
    pub uploaded: u64,
    pub time: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    pub piece: u32,
    pub offset: u32,
    pub length: u32,
}

impl Block {
    pub fn range(&self) -> Range<u32> {
        self.offset..self.offset + self.length
    }

    pub fn empty(size: u32) -> Self {
        Self {
            piece: 0,
            offset: 0,
            length: size,
        }
    }
}

/// Glue between active peers and scheduler
#[derive(Debug)]
pub struct Download {
    pub info_hash: [u8; 20],
    pub max_peers: usize,
    pub piece_length: u32,
    pub peers_handles: JoinSet<(Uuid, Result<(), PeerError>)>,
    pub status_rx: mpsc::Receiver<PeerStatus>,
    pub status_tx: mpsc::Sender<PeerStatus>,
    pub new_peers: mpsc::Receiver<NewPeer>,
    pub new_peers_join_set: JoinSet<Result<Peer, SocketAddrV4>>,
    pub pending_new_peers_ips: HashSet<SocketAddrV4>,
    pub scheduler: Scheduler,
}

impl Download {
    pub async fn new(
        output: impl AsRef<Path>,
        t: Info,
        new_peers: mpsc::Receiver<NewPeer>,
    ) -> Self {
        let info_hash = t.hash();
        let piece_length = t.piece_length;
        let active_peers = JoinSet::new();
        let (status_tx, status_rx) = mpsc::channel(1000);
        let commands = HashMap::new();

        let scheduler = Scheduler::new(output, t, commands);

        Self {
            max_peers: 200,
            new_peers,
            new_peers_join_set: JoinSet::new(),
            pending_new_peers_ips: HashSet::new(),
            info_hash,
            piece_length,
            peers_handles: active_peers,
            status_rx,
            status_tx,
            scheduler,
        }
    }

    pub async fn start(mut self) -> anyhow::Result<()> {
        let mut optimistic_unchoke_interval = tokio::time::interval(Duration::from_secs(30));
        let mut choke_interval = tokio::time::interval(Duration::from_secs(10));

        // immidiate tick
        optimistic_unchoke_interval.tick().await;
        choke_interval.tick().await;

        self.scheduler.start().await;

        loop {
            tokio::select! {
                Some(peer) = self.peers_handles.join_next() => self.handle_peer_join(peer),
                Some(status) = self.status_rx.recv() => {
                    if self.handle_peer_status(status).await {
                        return Ok(())
                    };
                },
                Some(new_peer) = self.new_peers.recv() => {
                    match new_peer {
                        NewPeer::ListenerOrigin(peer) => self.handle_new_peer(peer).await,
                        NewPeer::TrackerOrigin(ip) => self.handle_tracker_peer(ip),
                    };
                },
                Some(Ok(peer)) = self.new_peers_join_set.join_next() => {
                    let ip = match peer {
                        Ok(peer) => {
                            let ip = peer.ip();
                            self.handle_new_peer(peer).await;
                            ip
                        },
                        Err(ip) => ip,
                    };
                    self.pending_new_peers_ips.remove(&ip);
                },
                _ = optimistic_unchoke_interval.tick() => self.handle_optimistic_unchoke().await,
                _ = choke_interval.tick() => self.handle_choke_interval().await,
                else => {
                    break Err(anyhow!("Select branch"));
                }
            }
        }
    }

    fn handle_tracker_peer(&mut self, ip: SocketAddrV4) {
        if self.pending_new_peers_ips.insert(ip) {
            let info_hash = self.info_hash.clone();
            self.new_peers_join_set.spawn(async move {
                let timeout_duration = Duration::from_millis(500);
                match timeout(timeout_duration, Peer::new_from_ip(ip, info_hash)).await {
                    Ok(Ok(peer)) => Ok(peer),
                    Ok(Err(e)) => {
                        tracing::trace!("Peer with ip {} errored: {}", ip, e);
                        Err(ip)
                    }
                    Err(_) => {
                        tracing::trace!("Peer with ip {} timed out", ip);
                        Err(ip)
                    }
                }
            });
        } else {
            tracing::trace!("Recieved duplicate peer with ip {}", ip);
        }
    }

    async fn handle_new_peer(&mut self, peer: Peer) {
        let (tx, rx) = mpsc::channel(100);
        let ipc = PeerIPC {
            status_tx: self.status_tx.clone(),
            commands_rx: rx,
        };
        let active_peer = ActivePeer::new(tx, &peer);
        self.scheduler.add_peer(active_peer, peer.uuid).await;
        self.peers_handles.spawn(peer.download(ipc));
    }

    async fn handle_peer_status(&mut self, status: PeerStatus) -> bool {
        match status.message_type {
            PeerStatusMessage::Request { response, block } => {
                let _ = response.send(self.scheduler.retrieve_piece(block.piece as usize).await);
            }
            PeerStatusMessage::Choked => {
                self.scheduler.handle_peer_choke(status.peer_id).await;
            }
            PeerStatusMessage::Unchoked => self.scheduler.handle_peer_unchoke(status.peer_id).await,
            PeerStatusMessage::Data { bytes, block } => {
                let span = tracing::span!(tracing::Level::DEBUG, "Handle peer data");
                let _span = span.enter();

                let _ = self
                    .scheduler
                    .save_block(status.peer_id, block, bytes)
                    .await;
                let is_full = self
                    .scheduler
                    .get_peer(&status.peer_id)
                    .map(|p| p.pending_blocks.len() >= MAX_PENDING_BLOCKS)
                    .unwrap();
                if is_full {
                    tracing::warn!("Scheduling is not required");
                } else {
                    let _ = self.scheduler.schedule(&status.peer_id).await;
                };
                if self.scheduler.is_torrent_finished() {
                    tracing::info!("Finished downloading torrent");
                    return true;
                };
            }
            PeerStatusMessage::Have { piece } => {
                if let Some(peer) = self.scheduler.active_peers.get_mut(&status.peer_id) {
                    peer.bitfield.add(piece as usize).unwrap();
                    // self.scheduler.schedule(&status.peer_id).await;
                }
            }
            PeerStatusMessage::UnknownFailure => {
                let peer = self.scheduler.active_peers.get(&status.peer_id).unwrap();
                // scheduler will handle this peer after it exits
                peer.command.try_send(PeerCommand::Abort).unwrap();
            }
            PeerStatusMessage::Afk => {
                let peer = self.scheduler.active_peers.get(&status.peer_id).unwrap();
                if !peer.pending_blocks.is_empty() {
                    tracing::debug!(
                        "Peer with id {} is AFK while having pending blocks",
                        status.peer_id
                    );
                    self.scheduler.choke_peer(status.peer_id).await;
                }
            }
        }
        false
    }

    fn handle_peer_join(
        &mut self,
        join_res: Result<(Uuid, Result<(), PeerError>), tokio::task::JoinError>,
    ) {
        if let Ok((peer_id, Err(peer_err))) = &join_res {
            tracing::warn!(
                "Peer with id: {} joined with error: {:?} {}",
                peer_id,
                peer_err.error_type,
                peer_err.msg
            );
        }

        // remove peer from scheduler or propagate panic
        if let Ok((peer_id, _)) = join_res {
            self.scheduler.remove_peer(peer_id);
        } else {
            panic!("Peer process paniced");
        }
    }

    async fn handle_choke_interval(&mut self) {
        println!("Choke interval");
    }

    async fn handle_optimistic_unchoke(&mut self) {
        println!("Optimistic unchoke interval");
    }
}

#[derive(Debug, Clone)]
pub enum PeerCommand {
    Start { block: Block },
    Have { piece: u32 },
    Interested,
    Abort,
    Choke,
    Unchoke,
    NotInterested,
}

#[derive(Debug)]
pub struct PeerStatus {
    pub peer_id: Uuid,
    pub message_type: PeerStatusMessage,
}

#[derive(Debug)]
pub enum PeerStatusMessage {
    Request {
        response: oneshot::Sender<Option<Bytes>>,
        block: Block,
    },
    Choked,
    Unchoked,
    Data {
        bytes: Bytes,
        block: Block,
    },
    UnknownFailure,
    Afk,
    Have {
        piece: u32,
    },
}

impl Display for PeerStatusMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PeerStatusMessage::Request { block, .. } => {
                write!(f, "Request for piece: {}", block.piece)
            }
            PeerStatusMessage::Choked => write!(f, "Choked"),
            PeerStatusMessage::Unchoked => write!(f, "Unchoked"),
            PeerStatusMessage::Data { block, .. } => write!(f, "Data for piece: {}", block.piece),
            PeerStatusMessage::UnknownFailure => write!(f, "UnknownFailure"),
            PeerStatusMessage::Afk => write!(f, "Afk"),
            PeerStatusMessage::Have { piece } => write!(f, "Have piece {}", piece),
        }
    }
}

#[cfg(test)]
mod tests {
    use tracing_test::traced_test;

    use crate::Torrent;

    #[tokio::test]
    #[traced_test]
    async fn download_all() {
        let torrent = Torrent::from_file("torrents/book.torrent").unwrap();
        tracing::debug!("Tested torrent have {} pieces", torrent.info.pieces.len());
        // torrent.download("").await.unwrap();
    }

    #[tokio::test]
    #[traced_test]
    async fn codecrafters_download() {
        let torrent = Torrent::from_file("torrents/codecrafters.torrent").unwrap();
        tracing::debug!("Tested torrent have {} pieces", torrent.info.pieces.len());
        // torrent.download("").await.unwrap();
        let downloaded = std::fs::read("sample.txt").unwrap();
        let original = std::fs::read("codecrafters_original.txt").unwrap();
        tracing::debug!(
            "downloaded/original len: {}/{}",
            downloaded.len(),
            original.len()
        );
        assert!(downloaded.len().abs_diff(original.len()) <= 1);
    }

    #[test]
    #[traced_test]
    fn tracing_test() {
        tracing::info!("Tracing is working");
    }
}
