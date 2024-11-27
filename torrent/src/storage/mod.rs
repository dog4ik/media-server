use std::{io::SeekFrom, ops::Range, path::PathBuf, time::Instant};

use anyhow::{bail, ensure, Context};
use bytes::{Bytes, BytesMut};
use hash_verification::{Hasher, Payload, WorkResult};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    peers::BitField,
    protocol::{Hashes, Info, OutputFile},
    scheduler::BLOCK_LENGTH,
    utils::{verify_iter_sha1, verify_sha1},
};

pub mod hash_verification;

const HASHER_WORKERS: usize = 6;

pub struct ReadyPiece(Vec<Bytes>);

impl ReadyPiece {
    pub async fn write_to<T: AsyncWrite + Unpin>(
        &self,
        mut writer: T,
        range: Range<usize>,
    ) -> std::io::Result<()> {
        let block_length = BLOCK_LENGTH as usize;
        let start = range.start;
        let end = range.end;
        let start_idx = start / block_length;
        let end_idx = end.div_ceil(block_length);
        for i in start_idx..end_idx {
            let bytes = &self.0[i];
            let block_start = i * block_length;

            let relative_start = if i == start_idx {
                start - block_start
            } else {
                0
            };
            let relative_end = if i == end_idx - 1 {
                end - block_start
            } else {
                bytes.len() // Full block
            };
            writer
                .write_all(&bytes[relative_start..relative_end])
                .await?;
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.0.iter().map(|x| x.len()).sum()
    }
}

#[derive(Debug)]
pub struct FileHandles {
    pub opened_files: lru::LruCache<usize, fs::File>,
}

impl FileHandles {
    pub fn new() -> Self {
        use std::num::NonZeroUsize;
        Self {
            opened_files: lru::LruCache::new(NonZeroUsize::new(10).unwrap()),
        }
    }
}

#[derive(Debug)]
pub struct TorrentStorage {
    pub output_dir: PathBuf,
    pub output_files: Vec<OutputFile>,
    pub piece_size: u32,
    pub total_length: u64,
    pub pieces: Hashes,
    pub bitfield: BitField,
    pub enabled_files: BitField,
    // Cache of opened file handles
    pub file_handles: FileHandles,
    pub feedback_tx: mpsc::Sender<StorageFeedback>,
    pub hasher: hash_verification::Hasher,
}

#[derive(Debug, Clone)]
pub struct StorageHandle {
    pub message_tx: mpsc::Sender<StorageMessage>,
    pub cancellation_token: CancellationToken,
}

impl StorageHandle {
    pub async fn save_piece(&self, insert_piece: usize, blocks: Vec<Bytes>) {
        self.message_tx
            .send(StorageMessage::Save {
                piece_i: insert_piece,
                blocks,
            })
            .await
            .unwrap();
    }

    pub fn try_save_piece(&self, insert_piece: usize, blocks: Vec<Bytes>) -> anyhow::Result<()> {
        self.message_tx.try_send(StorageMessage::Save {
            piece_i: insert_piece,
            blocks,
        })?;
        Ok(())
    }
    pub async fn retrieve_piece(&self, piece_i: usize) {
        self.message_tx
            .send(StorageMessage::RetrievePiece { piece_i })
            .await
            .unwrap();
    }
    pub async fn retrieve_blocking(&self, piece_i: usize) -> Option<Bytes> {
        let (tx, rx) = oneshot::channel();
        self.message_tx
            .send(StorageMessage::RetrieveBlocking {
                piece_i,
                response: tx,
            })
            .await
            .unwrap();
        rx.await.unwrap()
    }
    pub async fn enable_file(&self, file_idx: usize) {
        self.message_tx
            .send(StorageMessage::EnableFile { file_idx })
            .await
            .unwrap();
    }
    pub async fn disable_file(&self, file_idx: usize) {
        self.message_tx
            .send(StorageMessage::DisableFile { file_idx })
            .await
            .unwrap();
    }
}

#[derive(Debug)]
pub enum StorageMessage {
    Save {
        piece_i: usize,
        blocks: Vec<Bytes>,
    },
    EnableFile {
        file_idx: usize,
    },
    DisableFile {
        file_idx: usize,
    },
    RetrievePiece {
        piece_i: usize,
    },
    RetrieveBlocking {
        piece_i: usize,
        response: oneshot::Sender<Option<Bytes>>,
    },
}

#[derive(Debug)]
pub enum StorageFeedback {
    Saved {
        piece_i: usize,
    },
    Failed {
        piece_i: usize,
    },
    Data {
        piece_i: usize,
        bytes: Option<Bytes>,
    },
}

impl TorrentStorage {
    pub fn new(
        feedback_tx: mpsc::Sender<StorageFeedback>,
        info: &Info,
        initial_bitfield: BitField,
        output_dir: PathBuf,
        enabled_files: &[usize],
    ) -> Self {
        let s = sysinfo::System::new();
        let workers = s
            .physical_core_count()
            .map_or(HASHER_WORKERS, |cores| cores / 2)
            .max(1);
        let output_files = info.output_files(&output_dir);
        let mut files_bitfield = BitField::empty(output_files.len());
        for enabled_idx in enabled_files {
            files_bitfield.add(*enabled_idx).unwrap();
        }
        let hasher = Hasher::new(workers);

        Self {
            feedback_tx,
            output_dir,
            output_files,
            piece_size: info.piece_length,
            total_length: info.total_size(),
            pieces: info.pieces.clone(),
            bitfield: initial_bitfield,
            enabled_files: files_bitfield,
            file_handles: FileHandles::new(),
            hasher,
        }
    }

    pub async fn spawn(
        mut self,
        tracker: &TaskTracker,
        cancellation_token: CancellationToken,
    ) -> anyhow::Result<StorageHandle> {
        let save_location_metadata = self.output_dir.metadata()?;
        if !save_location_metadata.is_dir() {
            return Err(anyhow::anyhow!(
                "Save directory must be a directory, got {:?}",
                save_location_metadata.file_type()
            ));
        }
        for file in &self.output_files {
            if fs::try_exists(file.path()).await.unwrap_or(true) {
                tracing::warn!("Output file already exists");
            }
            if let Some(parent) = file.path().parent() {
                fs::create_dir_all(parent)
                    .await
                    .context("Init paths for torrent files")?;
            }
        }
        let token = cancellation_token.clone();
        let (message_tx, mut message_rx) = mpsc::channel(200);
        tracker.spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = message_rx.recv() => self.handle_message(message).await,
                    work_result = self.hasher.recv() => self.handle_hasher_result(work_result).await,
                    _ = token.cancelled() => {
                        break;
                    }
                }
            }
        });
        Ok(StorageHandle {
            message_tx,
            cancellation_token,
        })
    }

    async fn handle_hasher_result(&mut self, result: WorkResult) {
        let piece_i = result.piece_i;
        if result.is_verified {
            self.bitfield.add(piece_i).unwrap();
            let start = Instant::now();
            let save_result = self.save_piece(piece_i, ReadyPiece(result.piece)).await;
            tracing::debug!(took = ?start.elapsed(), "Saved piece on the disk");
            match save_result {
                Ok(_) => {
                    let _ = self
                        .feedback_tx
                        .send(StorageFeedback::Saved { piece_i })
                        .await;
                }
                Err(_) => {
                    let _ = self
                        .feedback_tx
                        .send(StorageFeedback::Failed { piece_i })
                        .await;
                }
            }
        } else {
            let _ = self
                .feedback_tx
                .send(StorageFeedback::Failed { piece_i })
                .await;
        }
    }

    async fn handle_message(&mut self, message: StorageMessage) {
        match message {
            StorageMessage::Save { piece_i, blocks } => {
                self.pend_hash_validation(piece_i, ReadyPiece(blocks)).await;
            }
            StorageMessage::RetrieveBlocking { piece_i, response } => {
                let bytes = self.retrieve_piece(piece_i).await.ok();
                let _ = response.send(bytes);
            }
            StorageMessage::RetrievePiece { piece_i } => {
                // This will also block
                // TODO: verify hash using hasher
                let bytes = self.retrieve_piece(piece_i).await.ok();
                let _ = self
                    .feedback_tx
                    .send(StorageFeedback::Data { piece_i, bytes })
                    .await;
            }
            StorageMessage::EnableFile { file_idx } => {
                let _ = self.enabled_files.add(file_idx);
            }
            StorageMessage::DisableFile { file_idx } => {
                let _ = self.enabled_files.remove(file_idx);
            }
        };
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        crate::utils::piece_size(piece_i, self.piece_size, self.total_length) as u32
    }

    /// saves piece filling file with null bytes
    /// WARN: this will not validate piece hash
    pub async fn save_piece(&mut self, piece_i: usize, blocks: ReadyPiece) -> anyhow::Result<()> {
        let piece_length = blocks.len() as u32;
        ensure!(piece_length == self.piece_length(piece_i));

        let piece_start = piece_i as u64 * self.piece_size as u64;
        let piece_end = piece_start + piece_length as u64;

        let mut file_offset = 0;
        for (file_idx, file) in self.output_files.iter().enumerate() {
            let file_start = file_offset;
            let file_end = file_offset + file.length();
            if file_start > piece_end || file_end < piece_start || !self.enabled_files.has(file_idx)
            {
                file_offset += file.length();
                continue;
            }

            let insert_offset = piece_start.checked_sub(file_start).unwrap_or_default();
            let f = match self.file_handles.opened_files.get_mut(&file_idx) {
                Some(f) => f,
                None => {
                    tracing::debug!("Creating file handle: {}", file.path().display());
                    let file_handle = fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .open(file.path())
                        .await?;
                    file_handle.set_len(file.length()).await?;
                    self.file_handles.opened_files.put(file_idx, file_handle);
                    self.file_handles.opened_files.get_mut(&file_idx).unwrap()
                }
            };
            f.seek(SeekFrom::Start(insert_offset)).await?;

            let relative_start = file_start as isize - piece_start as isize;
            let relative_end = file_end as isize - piece_end as isize;

            let start = if relative_start > 0 {
                // start is behind file
                relative_start.abs()
            } else {
                // start is beyond file
                0
            } as usize;

            let end = if relative_end < 0 {
                // end is beyond file
                piece_length - relative_end.abs() as u32
            } else {
                // end is behind file
                piece_length
            } as usize;
            blocks.write_to(f, start..end).await?;
            file_offset += file.length();
        }
        Ok(())
    }

    /// retrieve piece from preallocated file
    pub async fn retrieve_piece(&mut self, piece_i: usize) -> anyhow::Result<Bytes> {
        if !self.bitfield.has(piece_i) {
            bail!("Piece {piece_i} is not available");
        };
        let piece_length = self.piece_length(piece_i);
        let mut bytes = BytesMut::zeroed(piece_length as usize);

        let piece_start = piece_i as u64 * self.piece_size as u64;
        let piece_end = piece_start + piece_length as u64;

        let mut file_offset = 0;
        for (file_idx, file) in self.output_files.iter().enumerate() {
            let file_start = file_offset;
            let file_end = file_offset + file.length();
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length();
                continue;
            }

            let insert_offset = piece_start.checked_sub(file_start).unwrap_or_default();
            let f = match self.file_handles.opened_files.get_mut(&file_idx) {
                Some(f) => f,
                None => {
                    tracing::debug!("Creating file handle: {}", file.path().display());
                    let file_handle = fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .open(file.path())
                        .await?;
                    file_handle.set_len(file.length()).await?;
                    self.file_handles.opened_files.put(file_idx, file_handle);
                    self.file_handles.opened_files.get_mut(&file_idx).unwrap()
                }
            };
            f.seek(SeekFrom::Start(insert_offset)).await?;
            let range_start = if file_start > piece_start {
                (file_start - piece_start) as usize
            } else {
                0
            };
            let range_end = if file_end < piece_end {
                (piece_end - file_end) as usize
            } else {
                piece_length as usize
            };
            f.read_exact(&mut bytes[range_start..range_end]).await?;
            file_offset += file.length();
        }
        let bytes = bytes.freeze();
        let hash = self.pieces.get_hash(piece_i).unwrap();
        if !verify_sha1(hash, &bytes) {
            panic!("Failed to verify hash of retrieved piece");
        };
        Ok(bytes)
    }

    pub async fn recheck_pieces(&mut self) -> anyhow::Result<()> {
        let bitfield = self.bitfield.clone();
        for i in bitfield.pieces() {
            let bytes = self.retrieve_piece(i).await?;
            if !verify_sha1(&self.pieces.0[i], &bytes) {
                anyhow::bail!("Failed to verify hash of the piece: {}", i);
            }
        }
        Ok(())
    }

    pub async fn pend_hash_validation(&mut self, piece_i: usize, data: ReadyPiece) {
        let hash = self.pieces.0[piece_i];
        let payload = Payload {
            hash,
            piece_i,
            data: data.0,
        };
        self.hasher.pend_job(payload).await
    }
}
