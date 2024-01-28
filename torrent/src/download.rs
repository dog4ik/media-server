use std::{
    collections::HashMap,
    fs::File,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    ops::Range,
    os::unix::fs::FileExt,
    path::Path,
};

use anyhow::{anyhow, bail, ensure};
use bytes::{Bytes, BytesMut};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use uuid::Uuid;

use crate::{
    file::{Hashes, TorrentFile},
    peers::{BitField, Peer, PeerError, PeerErrorCause, PeerIPC, PeerMessage},
    utils::verify_sha1,
};

const BLOCK_SIZE: u32 = 1024 * 16;

const LISTENER_PORT: u16 = 6881;

/// Piece representation where all blocks are sorted
#[derive(Debug)]
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
    pub fn new(output_path: impl AsRef<Path>, torrent: &TorrentFile) -> Self {
        use std::fs;
        let output_path = output_path.as_ref().join(&torrent.info.name);

        let output_file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(output_path)
            .expect("file can be opened");

        let bitfield = BitField::empty(torrent.info.pieces.len());

        let assignment = LinearAssignment {
            block_queue: Vec::new(),
            current_piece: 0,
            piece_length: torrent.info.piece_length,
            pieces: torrent.info.pieces.clone(),
            total_length: torrent.info.total_size(),
            block_size: BLOCK_SIZE,
        };

        Self {
            pending_pieces: HashMap::new(),
            pieces: torrent.info.pieces.clone(),
            file: output_file,
            piece_size: torrent.info.piece_length,
            bitfield,
            total_length: torrent.info.total_size(),
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
}

#[derive(Debug)]
pub struct LinearAssignment {
    current_piece: u32,
    piece_length: u64,
    block_queue: Vec<Block>,
    total_length: u64,
    pieces: Hashes,
    block_size: u32,
}

fn piece_size(piece_i: u32, piece_length: u32, total_pieces: u32, total_length: u32) -> u32 {
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
    pub piece_length: u64,
    pub available_peers: Vec<SocketAddrV4>,
    pub active_peers: JoinSet<Result<(), PeerError>>,
    pub commands: HashMap<Uuid, ActivePeer>,
    pub status_rx: mpsc::Receiver<PeerStatus>,
    pub status_tx: mpsc::Sender<PeerStatus>,
    pub listener: TcpListener,
    pub storage: TorrentStorage,
}

impl Download {
    pub async fn new(
        t: TorrentFile,
        peers_list: Vec<SocketAddrV4>,
        max_peers: usize,
    ) -> anyhow::Result<Self> {
        let info_hash = t.info.hash();
        let piece_length = t.info.piece_length;
        let listen_addr = SocketAddr::new(
            std::net::IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)),
            LISTENER_PORT,
        );
        let mut available_peers = Vec::new();
        let mut active_peers = JoinSet::new();
        let (status_tx, status_rx) = mpsc::channel(100);
        let mut commands = HashMap::new();

        let storage = TorrentStorage::new("", &t);

        for ip in peers_list {
            if active_peers.len() > max_peers {
                available_peers.push(ip);
                continue;
            }
            let (commands_tx, commands_rx) = mpsc::channel(100);
            let Ok(socket) = TcpStream::connect(&ip).await else {
                tracing::warn!("Failed to create connection with peer: {ip}");
                continue;
            };
            let ipc = PeerIPC {
                status_tx: status_tx.clone(),
                commands_rx,
            };
            let Ok(peer) = Peer::new(socket, info_hash, ipc).await else {
                tracing::warn!("Failed to handshake with peer: {ip}");
                continue;
            };
            let active_peer = ActivePeer {
                command: commands_tx,
                bitfield: peer.bitfield.clone(),
            };
            commands.insert(peer.uuid, active_peer);
            active_peers.spawn(peer.download());
        }

        let listener = TcpListener::bind(listen_addr).await?;

        Ok(Self {
            max_peers,
            info_hash,
            piece_length,
            available_peers,
            active_peers,
            status_rx,
            status_tx,
            listener,
            commands,
            storage,
        })
    }

    pub async fn concurrent_download(&mut self) -> anyhow::Result<()> {
        for peer in self.commands.values() {
            let block = self.storage.get_work(&peer.bitfield).unwrap();
            peer.command
                .send(PeerCommand::Start { block })
                .await
                .unwrap();
        }
        loop {
            tokio::select! {
                peer = self.active_peers.join_next() => self.handle_peer_join(peer),
                Some(status) = self.status_rx.recv() => {
                         if self.handle_peer_status(status).await {
                            return Ok(())
                        };
                    },
                Ok((socket, ip)) = self.listener.accept() =>  self.handle_new_peer(socket, ip).await.unwrap(),
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
                payload: self.storage.bitfield.clone(),
            })
            .await?;
            let active_peer = ActivePeer {
                command: tx,
                bitfield: peer.bitfield,
            };
            self.commands.insert(peer.uuid, active_peer);
        };
        Ok(())
    }

    async fn handle_peer_status(&mut self, status: PeerStatus) -> bool {
        match status.message_type {
            MessageType::Request { response, block } => {
                let _ = response.send(self.storage.retrieve_piece(block.piece as usize));
            }
            MessageType::Choked => {
                todo!("remove peer from available list")
            }
            MessageType::Unchoked => {
                todo!("add peer to available list")
            }
            MessageType::Data { bytes, block } => {
                let full_piece = self.storage.store_block(block, bytes).unwrap();
                if let Some(full_piece) = full_piece {
                    for (peer_id, peer) in &mut self.commands {
                        if *peer_id == status.peer_id {
                            continue;
                        }
                        peer.command
                            .send(PeerCommand::Have { piece: full_piece })
                            .await
                            .unwrap();
                    }
                }
                let peer = self.commands.get(&status.peer_id).unwrap();
                if let Some(new_block) = self.storage.get_work(&peer.bitfield) {
                    peer.command
                        .send(PeerCommand::Start { block: new_block })
                        .await
                        .unwrap();
                    // if we got piece then announce it to everyone
                    // let _ = value.command.send(PeerCommand::Have { piece });
                }
                if self.storage.bitfield.pieces().count() == self.storage.pieces.len() {
                    return true;
                };
            }
            MessageType::Have { piece } => {
                if let Some(peer) = self.commands.get_mut(&status.peer_id) {
                    peer.bitfield.add(piece as usize).unwrap();
                }
            }
            MessageType::UnknownFailure => {
                let peer = self.commands.get(&status.peer_id).unwrap();
                let _ = peer.command.send(PeerCommand::Abort);
                self.commands.remove(&status.peer_id);
                todo!("remove peer from global peer list")
            }
        }
        false
    }

    fn handle_peer_join(
        &mut self,
        join_res: Option<Result<Result<(), PeerError>, tokio::task::JoinError>>,
    ) {
        let Some(join_res) = join_res else {
            // active_peers are empty
            todo!("handle empy active peers")
        };
        if let Ok(Err(peer_err)) = join_res {
            match peer_err.error_type {
                PeerErrorCause::Timeout => todo!(),
                PeerErrorCause::Connection => todo!(),
                PeerErrorCause::PeerLogic => todo!(),
                PeerErrorCause::Unhandled => todo!(),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum PeerCommand {
    Start { block: Block },
    Have { piece: u32 },
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

    use crate::{
        download::{Assignment, Download, LinearAssignment},
        file::{Hashes, TorrentFile},
    };

    use super::Piece;

    #[test]
    fn assignment_liniar() {
        let block_size = 4;
        let piece_length = 6;
        let mut assignment = LinearAssignment {
            current_piece: 0,
            block_queue: Vec::new(),
            piece_length: piece_length as u64,
            total_length: 18,
            pieces: Hashes(vec![[0; 20], [1; 20], [2; 20]]),
            block_size,
        };
        assert_eq!(Some(0), assignment.assign_next().map(|p| p.piece));
        assert_eq!(Some(0), assignment.assign_next().map(|p| p.piece));
        assert_eq!(Some(1), assignment.assign_next().map(|p| p.piece));
        assert_eq!(Some(1), assignment.assign_next().map(|p| p.piece));
        assert_eq!(Some(2), assignment.assign_next().map(|p| p.piece));
        assert_eq!(Some(2), assignment.assign_next().map(|p| p.piece));
        assert_eq!(None, assignment.assign_next().map(|p| p.piece));
    }

    #[tokio::test]
    async fn download_all() {
        let torrent = TorrentFile::from_path("torrents/codecrafters.torrent").unwrap();
        dbg!(torrent.info.pieces.len());
        let peers = torrent.announce().await.unwrap().peers;
        let mut download = Download::new(torrent, peers, 5).await.unwrap();
        download.concurrent_download().await.unwrap();
        let downloaded = std::fs::read("sample.txt").unwrap();
        let original = std::fs::read("codecrafters_original.txt").unwrap();
        dbg!(downloaded.len(), original.len());
        assert!(downloaded.len().abs_diff(original.len()) <= 1);
    }

    #[test]
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
        dbg!(missing);
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
}
