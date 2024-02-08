use std::{
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    ops::Range,
    path::Path,
    time::Duration,
};

use anyhow::{anyhow, bail, ensure};
use bytes::{Bytes, BytesMut};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, Semaphore},
    task::JoinSet,
    time::{timeout, Instant},
};
use uuid::Uuid;

use crate::{
    file::Hashes,
    peers::{BitField, Peer, PeerError, PeerErrorCause, PeerIPC, PeerMessage},
    torrent::Torrent,
    utils::verify_sha1,
};

const BLOCK_SIZE: u32 = 1024 * 16;

const LISTENER_PORT: u16 = 6881;

const MAX_UPLOAD_PEERS: usize = 5;

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

    pub fn get_missing_block(&self, max_length: u32) -> Option<Block> {
        if self.is_full() {
            return None;
        }
        let mut cursor = 0;
        for (offset, bytes) in self.blocks.iter().rev() {
            if *offset < cursor {
                let block = Block {
                    piece: self.index,
                    offset: cursor,
                    length: std::cmp::min(max_length, offset - cursor),
                };
                return Some(block);
            } else {
                cursor = *offset + bytes.len() as u32
            }
        }
        if cursor != self.length {
            let block = Block {
                piece: self.index,
                offset: cursor,
                length: std::cmp::min(max_length, self.length - cursor),
            };
            Some(block)
        } else {
            None
        }
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
pub struct TorrentStorage {
    pub file: File,
    pub piece_size: u64,
    pub total_length: u64,
    pub pieces: Hashes,
    pub pending_pieces: HashMap<usize, Piece>,
    pub bitfield: BitField,
    pub assignment: LinearAssignment,
    pub failed_blocks: Vec<Block>,
}

impl TorrentStorage {
    pub fn new(output_path: impl AsRef<Path>, torrent: &Torrent) -> Self {
        use std::fs;
        let torrent_file = &torrent.file;
        let output_path = output_path.as_ref().join(&torrent.file.info.name);

        let output_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(output_path)
            .expect("file can be opened");

        let bitfield = BitField::empty(torrent_file.info.pieces.len());

        let assignment = LinearAssignment {
            block_queue: Vec::new(),
            current_piece: 0,
            piece_length: torrent_file.info.piece_length,
            pieces: torrent_file.info.pieces.clone(),
            total_length: torrent_file.info.total_size(),
            block_size: BLOCK_SIZE,
        };

        Self {
            pending_pieces: HashMap::new(),
            pieces: torrent_file.info.pieces.clone(),
            file: output_file,
            piece_size: torrent_file.info.piece_length,
            bitfield,
            total_length: torrent_file.info.total_size(),
            assignment,
            failed_blocks: Vec::new(),
        }
    }

    pub fn piece_length(&self, piece_i: usize) -> u32 {
        piece_size(
            piece_i as u32,
            self.assignment.piece_length as u32,
            self.pieces.len() as u32,
            self.total_length as u32,
        )
    }

    pub fn get_work(&mut self, bitfield: &BitField) -> Option<Block> {
        for (i, block) in self.failed_blocks.iter_mut().enumerate() {
            if bitfield.has(block.piece as usize) {
                return Some(self.failed_blocks.swap_remove(i));
            }
        }
        self.assignment.assign_next()
    }

    pub fn save_piece(&mut self, piece_i: usize) -> anyhow::Result<()> {
        let piece = self
            .pending_pieces
            .remove(&piece_i)
            .expect("piece always exist");
        let mut offset = 0;
        let piece_i = piece.index as usize;
        tracing::trace!("saving piece to the disk ({})", piece_i);
        for piece in self.bitfield.pieces() {
            let p_len = self.piece_length(piece);
            if piece < piece_i {
                offset += p_len;
            } else {
                break;
            };
        }
        let hash = self.pieces.get_hash(piece.index as usize).unwrap();
        let bytes = piece.as_bytes().unwrap();
        ensure!(verify_sha1(hash, &bytes));
        self.file.write_at(&bytes, offset as u64)?;
        self.bitfield.add(piece_i).unwrap();
        Ok(())
    }

    /// Save block and return the piece index if it completed
    pub fn store_block(&mut self, block: Block, bytes: Bytes) -> anyhow::Result<Option<u32>> {
        ensure!(block.length as usize == bytes.len());

        let index = block.piece as usize;
        if let Some(piece) = self.pending_pieces.get_mut(&index) {
            piece.blocks.push((block.offset, bytes));
        } else {
            let length = self.piece_length(block.piece as usize);
            let piece = Piece {
                index: block.piece,
                length,
                blocks: vec![(block.offset, bytes)],
            };
            self.pending_pieces.insert(index, piece);
        };

        let piece = self.pending_pieces.get(&index).unwrap();
        if piece.is_full() {
            self.save_piece(index)?;
        }

        Ok(None)
    }

    pub fn retrieve_piece(&self, piece_i: usize) -> Option<Bytes> {
        let mut offset = 0;
        for piece in self.bitfield.pieces() {
            if piece <= piece_i {
                offset += 1;
            }
            if piece == piece_i {
                break;
            }
        }
        let mut buf = BytesMut::with_capacity(self.piece_length(piece_i) as usize);
        self.file.read_at(&mut buf, offset).ok()?;
        Some(buf.into())
    }
}

/// Describes what block is going to be downloaded next
pub trait Assignment {
    fn assign_next(&mut self) -> Option<Block>;
}

#[derive(Debug)]
pub struct ActivePeer {
    pub command: mpsc::Sender<PeerCommand>,
    pub bitfield: BitField,
    pub choke_status: ChokeStatus,
    pub peer_choke_status: ChokeStatus,
    /// Pending blocks are used if peer panics or chokes, also it indicates that peer is busy
    pub pending_blocks: Vec<Block>,
}

impl ActivePeer {
    pub fn new(command: mpsc::Sender<PeerCommand>, bitfield: BitField) -> Self {
        let choke_status = ChokeStatus::default();
        Self {
            command,
            bitfield,
            peer_choke_status: choke_status.clone(),
            choke_status,
            pending_blocks: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct LinearAssignment {
    current_piece: u32,
    piece_length: u32,
    block_queue: Vec<Block>,
    total_length: u64,
    pieces: Hashes,
    block_size: u32,
}

pub fn piece_size(piece_i: u32, piece_length: u32, total_pieces: u32, total_length: u32) -> u32 {
    if piece_i == total_pieces - 1 {
        let md = total_length % piece_length;
        if md == 0 {
            piece_length
        } else {
            md
        }
    } else {
        piece_length
    }
}

impl Assignment for LinearAssignment {
    fn assign_next(&mut self) -> Option<Block> {
        let piece_length = piece_size(
            self.current_piece,
            self.piece_length as u32,
            self.pieces.len() as u32,
            self.total_length as u32,
        );
        if let Some(block) = self.block_queue.pop() {
            return Some(block);
        } else {
            if self.current_piece as usize == self.pieces.len() {
                return None;
            }
            let mut blocks = Vec::new();
            let mut cursor = 0;
            while cursor != piece_length {
                let block_length = if cursor + self.block_size > piece_length {
                    let md = piece_length % self.block_size;
                    if md == 0 {
                        self.block_size
                    } else {
                        md
                    }
                } else {
                    self.block_size
                };
                let new_block = Block {
                    piece: self.current_piece,
                    offset: cursor,
                    length: block_length,
                };
                blocks.insert(0, new_block);
                cursor += block_length;
            }
            self.current_piece += 1;
            self.block_queue = blocks;
            return self.block_queue.pop();
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BlockLength {
    Small,
    Medium,
    Long,
}

#[derive(Debug, Clone)]
pub struct PeerStats {
    last_update: Instant,
}

#[derive(Debug, Clone)]
pub struct ChokeStatus {
    choked: bool,
    choked_time: Instant,
}

impl Default for ChokeStatus {
    fn default() -> Self {
        Self {
            choked: true,
            choked_time: Instant::now(),
        }
    }
}

impl ChokeStatus {
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

    pub fn choke_duration(&self) -> Duration {
        self.choked_time.elapsed()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Block {
    pub piece: u32,
    pub offset: u32,
    pub length: u32,
}

impl Block {
    pub fn next_block(&self, block_size: u32, piece_length: u32) -> Block {
        // check if new offset
        let new_offset = self.offset + self.length;
        if self.offset == piece_length {
            return Self {
                piece: self.piece,
                offset: piece_length,
                length: 0,
            };
        }
        let length = if new_offset >= piece_length {
            new_offset - piece_length
        } else {
            std::cmp::min(block_size, piece_length)
        };
        Self {
            piece: self.piece,
            offset: new_offset,
            length,
        }
    }

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

#[derive(Debug)]
pub struct Download {
    pub info_hash: [u8; 20],
    pub max_peers: usize,
    pub piece_length: u32,
    pub available_peers: Vec<SocketAddrV4>,
    pub peers_handles: JoinSet<(Uuid, Result<(), PeerError>)>,
    pub status_rx: mpsc::Receiver<PeerStatus>,
    pub status_tx: mpsc::Sender<PeerStatus>,
    pub listener: TcpListener,
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
            available_peers,
            peers_handles: active_peers,
            status_rx,
            status_tx,
            listener,
            scheduler,
        })
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
                peer = self.peers_handles.join_next() => self.handle_peer_join(peer),
                Some(status) = self.status_rx.recv() => {
                    if self.handle_peer_status(status).await {
                        return Ok(())
                    };
                },
                Ok((socket, ip)) = self.listener.accept() => self.handle_new_peer(socket, ip).await.unwrap(),
                    _ = optimistic_unchoke_interval.tick() => {
                },
            }
        }
    }

    async fn handle_new_peer(
        &mut self,
        socket: TcpStream,
        ip: SocketAddr,
    ) -> Result<(), PeerError> {
        let (tx, rx) = mpsc::channel(100);
        let ipc = PeerIPC {
            status_tx: self.status_tx.clone(),
            commands_rx: rx,
        };
        if let Ok(mut peer) = Peer::new(socket, self.info_hash, ipc).await {
            peer.send_peer_msg(PeerMessage::Bitfield {
                payload: self.scheduler.bitfield.clone(),
            })
            .await?;
            let active_peer = ActivePeer::new(tx, peer.bitfield);
            self.scheduler.active_peers.insert(peer.uuid, active_peer);
        };
        Ok(())
    }

    async fn handle_peer_status(&mut self, status: PeerStatus) -> bool {
        match status.message_type {
            MessageType::Request { response, block } => {
                let _ = response.send(self.scheduler.retrieve_piece(block.piece as usize).await);
            }
            MessageType::Choked => {
                self.scheduler.choke_peer(status.peer_id);
            }
            MessageType::Unchoked => self.scheduler.handle_peer_unchoke(status.peer_id).await,
            MessageType::Data { bytes, block } => {
                let _ = self
                    .scheduler
                    .save_block(status.peer_id, block, bytes)
                    .await;
                let _ = self.scheduler.schedule_next_block().await;
                if self.scheduler.is_torrent_finished() {
                    return true;
                };
            }
            MessageType::Have { piece } => {
                if let Some(peer) = self.scheduler.active_peers.get_mut(&status.peer_id) {
                    peer.bitfield.add(piece as usize).unwrap();
                }
            }
            MessageType::UnknownFailure => {
                let peer = self.scheduler.active_peers.get(&status.peer_id).unwrap();
                // scheduler will handle this peer after it exits
                peer.command.send(PeerCommand::Abort).await.unwrap();
            }
        }
        false
    }

    fn handle_peer_join(
        &mut self,
        join_res: Option<Result<(Uuid, Result<(), PeerError>), tokio::task::JoinError>>,
    ) {
        let Some(join_res) = join_res else {
            // active_peers are empty
            todo!("handle empty active peers")
        };
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
}

#[derive(Debug, Clone)]
pub enum PeerCommand {
    Start { block: Block },
    Have { piece: u32 },
    Interested,
    Abort,
}

#[derive(Debug)]
pub struct PeerStatus {
    pub peer_id: Uuid,
    pub message_type: MessageType,
}

#[derive(Debug)]
pub enum MessageType {
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
    Have {
        piece: u32,
    },
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use tracing_test::traced_test;

    use crate::{
        download::{Assignment, Download, LinearAssignment},
        file::Hashes,
        torrent::Torrent,
    };

    use super::Piece;

    #[tokio::test]
    #[traced_test]
    async fn download_all() {
        let torrent = TorrentFile::from_path("torrents/book.torrent").unwrap();
        tracing::debug!("Tested torrent have {} pieces", torrent.info.pieces.len());
        let peers = torrent.announce().await.unwrap().peers;
        let mut download = Download::new(torrent, peers, 5).await.unwrap();
        download.concurrent_download().await.unwrap();
    }

    #[tokio::test]
    async fn download_all() {
        let torrent = Torrent::new("torrents/codecrafters.torrent").await.unwrap();
        dbg!(torrent.file.info.pieces.len());
        let mut download = Download::new(torrent, 5).await.unwrap();
        download.concurrent_download().await.unwrap();
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
    fn piece_state() {
        let mut piece = Piece {
            index: 0,
            length: 5,
            blocks: Vec::new(),
        };
        // [_, _, _, _, _]
        assert!(!piece.is_full());
        piece.add_block(0, Bytes::from_static(&[0; 1])).unwrap();
        // [0, _, _, _, _]
        assert!(!piece.is_full());
        let missing = piece.get_missing_block(2).unwrap();
        assert_eq!(missing.offset, 1);
        assert_eq!(missing.length, 2);
        assert!(piece.add_block(0, Bytes::from_static(&[1; 2])).is_err());
        piece.add_block(2, Bytes::from_static(&[2; 2])).unwrap();
        // [0, _, 2, 2, _]
        let missing = piece.get_missing_block(2).unwrap();
        assert_eq!(missing.offset, 4);
        assert_eq!(missing.length, 1);
        assert!(piece.add_block(2, Bytes::from_static(&[1; 2])).is_err());
        piece.add_block(1, Bytes::from_static(&[1; 1])).unwrap();
        // [0, 1, 2, 2, _]
        assert!(!piece.is_full());
        piece.add_block(4, Bytes::from_static(&[3; 1])).unwrap();
        // [0, 1, 2, 2, 3]
        assert!(piece.add_block(0, Bytes::from_static(&[1; 2])).is_err());
        assert!(piece.get_missing_block(2).is_none());
        assert!(piece.is_full());
        assert_eq!(&[0, 1, 2, 2, 3], piece.as_bytes().unwrap().as_ref());
    }

    #[test]
    #[traced_test]
    fn tracing_test() {
        tracing::info!("Tracing is working");
    }
}
