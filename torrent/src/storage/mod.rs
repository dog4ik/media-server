use std::{io::SeekFrom, ops::Range, path::PathBuf, time::Instant};

use anyhow::{bail, ensure, Context};
use bytes::{Bytes, BytesMut};
use hash_verification::{Hasher, Payload, WorkResult};
use parts::PartsFile;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    peers::BitField,
    protocol::{Hashes, OutputFile},
    scheduler::BLOCK_LENGTH,
    DownloadParams, Priority,
};

mod hash_verification;
pub mod parts;

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
struct FileHandles {
    opened_files: lru::LruCache<usize, fs::File>,
}

impl FileHandles {
    pub fn new() -> Self {
        use std::num::NonZeroUsize;
        Self {
            opened_files: lru::LruCache::new(NonZeroUsize::new(10).unwrap()),
        }
    }
}

#[derive(Debug, Clone)]
struct StorageFile {
    offset: u64,
    length: u64,
    path: PathBuf,
    is_enabled: bool,
}

impl StorageFile {
    pub fn new_files(output_files: &[OutputFile], priorities: &[Priority]) -> Box<[Self]> {
        debug_assert_eq!(output_files.len(), priorities.len());
        let mut offset = 0;
        let mut out = Vec::with_capacity(output_files.len());
        for (i, file) in output_files.iter().enumerate() {
            let length = file.length();
            out.push(StorageFile {
                offset,
                length,
                path: file.path().to_owned(),
                is_enabled: !priorities[i].is_disabled(),
            });
            offset += length;
        }
        out.into_boxed_slice()
    }

    pub fn end(&self) -> u64 {
        self.offset + self.length
    }

    pub fn start_piece(&self, piece_length: u64) -> usize {
        (self.offset / piece_length) as usize
    }

    pub fn end_piece(&self, piece_length: u64) -> usize {
        ((self.end() - 1) / piece_length) as usize
    }
}

#[derive(Debug)]
pub struct TorrentStorage {
    output_dir: PathBuf,
    files: Box<[StorageFile]>,
    piece_length: u64,
    total_length: u64,
    pieces: Hashes,
    bitfield: BitField,
    // Cache of opened file handles
    file_handles: FileHandles,
    feedback_tx: mpsc::Sender<StorageFeedback>,
    hasher: hash_verification::Hasher,
    parts_file: PartsFile,
}

#[derive(Debug, Clone)]
pub struct StorageHandle {
    pub message_tx: mpsc::Sender<StorageMessage>,
    #[allow(unused)]
    pub cancellation_token: CancellationToken,
}

impl StorageHandle {
    pub fn try_save_piece(&self, insert_piece: usize, blocks: Vec<Bytes>) -> anyhow::Result<()> {
        self.message_tx.try_send(StorageMessage::Save {
            piece_i: insert_piece,
            blocks,
        })?;
        Ok(())
    }
    pub fn try_retrieve_piece(&self, piece_i: usize) -> anyhow::Result<()> {
        self.message_tx
            .try_send(StorageMessage::RetrievePiece { piece_i })?;
        Ok(())
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
    //pub async fn validate_hash(&self) {
    //    self.message_tx
    //        .send(StorageMessage::Validate)
    //        .await
    //        .unwrap()
    //}
}

#[derive(Debug)]
pub enum StorageMessage {
    Save { piece_i: usize, blocks: Vec<Bytes> },
    EnableFile { file_idx: usize },
    DisableFile { file_idx: usize },
    RetrievePiece { piece_i: usize },
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
        parts_file: PartsFile,
        torrent_params: DownloadParams,
    ) -> Self {
        let info = torrent_params.info;
        let output_dir = torrent_params.save_location;
        let bitfield = torrent_params.bitfield;
        let s = sysinfo::System::new();
        let workers = s
            .physical_core_count()
            .map_or(HASHER_WORKERS, |cores| cores / 2)
            .max(1);
        let output_files = info.output_files(&output_dir);
        let files = StorageFile::new_files(&output_files, &torrent_params.files);
        let hasher = Hasher::new(workers);

        Self {
            feedback_tx,
            output_dir,
            files,
            piece_length: info.piece_length as u64,
            total_length: info.total_size(),
            pieces: info.pieces.clone(),
            bitfield,
            file_handles: FileHandles::new(),
            parts_file,
            hasher,
        }
    }

    pub async fn spawn(
        mut self,
        tracker: &TaskTracker,
        cancellation_token: CancellationToken,
    ) -> anyhow::Result<StorageHandle> {
        let save_location_metadata = fs::metadata(&self.output_dir)
            .await
            .context("save directory metadata")?;
        if !save_location_metadata.is_dir() {
            return Err(anyhow::anyhow!(
                "Save directory must be a directory, got {:?}",
                save_location_metadata.file_type()
            ));
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
            tracing::trace!(took = ?start.elapsed(), "Saved piece {piece_i} on the disk");
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
            StorageMessage::RetrievePiece { piece_i } => {
                let bytes = self.retrieve_piece(piece_i).await.ok();
                let _ = self
                    .feedback_tx
                    .send(StorageFeedback::Data { piece_i, bytes })
                    .await;
            }
            StorageMessage::EnableFile { file_idx } => self.enable_file(file_idx).await,
            StorageMessage::DisableFile { file_idx } => {
                self.files[file_idx].is_enabled = false;
            }
        };
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u64 {
        crate::utils::piece_size(piece_i, self.piece_length as u32, self.total_length)
    }

    pub async fn enable_file(&mut self, file_idx: usize) {
        let file = &mut self.files[file_idx];
        file.is_enabled = true;
        let file = file.clone();
        let file_offset = file.offset;
        let start_piece = (file_offset / self.piece_length) as usize;
        if let Ok(bytes) = self.parts_file.read_piece(start_piece).await {
            self.pend_hash_validation(start_piece, ReadyPiece(split_bytes(bytes)))
                .await;
        }

        let end_piece = ((file.end() - 1) / self.piece_length) as usize;
        if let Ok(bytes) = self.parts_file.read_piece(end_piece).await {
            self.pend_hash_validation(end_piece, ReadyPiece(split_bytes(bytes)))
                .await;
        }
    }

    /// saves piece filling file with null bytes
    /// WARN: this will not validate piece hash
    pub async fn save_piece(&mut self, piece_i: usize, blocks: ReadyPiece) -> anyhow::Result<()> {
        let piece_length = blocks.len() as u64;
        ensure!(piece_length == self.piece_length(piece_i));

        let piece_start = piece_i as u64 * self.piece_length;
        let piece_end = piece_start + piece_length;

        for (file_idx, file) in self.files.iter().enumerate() {
            let file_start = file.offset;
            let file_end = file.end();
            if file_start > piece_end || file_end < piece_start {
                continue;
            }

            let file_end_piece = file.end_piece(self.piece_length);
            let file_start_piece = file.start_piece(self.piece_length);
            if !file.is_enabled && (piece_i == file_end_piece || piece_i == file_start_piece) {
                let border_next = self
                    .files
                    .get(file_idx + 1)
                    .is_some_and(|next| next.start_piece(self.piece_length) == file_end_piece);
                let border_prev = file_idx
                    .checked_sub(1)
                    .and_then(|i| self.files.get(i))
                    .is_some_and(|prev| prev.end_piece(self.piece_length) == file_start_piece);
                if border_next || border_prev {
                    if piece_i as u64 == self.total_length / self.piece_length {
                        tracing::error!("Skipping the last piece to avoid .parts aligning issues");
                        continue;
                    };
                    if let Err(e) = self.parts_file.write_piece(piece_i, &blocks.0).await {
                        tracing::error!("Failed to write piece {piece_i} to the parts file: {e}");
                    };
                }
                continue;
            }

            let insert_offset = piece_start.saturating_sub(file_start);
            let f = match self.file_handles.opened_files.get_mut(&file_idx) {
                Some(f) => f,
                None => {
                    if let Some(parent) = file.path.parent() {
                        fs::create_dir_all(parent).await?;
                    }
                    tracing::debug!("Creating file handle: {}", file.path.display());
                    let file_handle = fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .open(&file.path)
                        .await?;
                    file_handle.set_len(file.length).await?;
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
                piece_length - relative_end.abs() as u64
            } else {
                // end is behind file
                piece_length
            } as usize;
            blocks.write_to(f, start..end).await?;
        }
        Ok(())
    }

    /// retrieve piece from preallocated file
    pub async fn retrieve_piece(&mut self, piece_i: usize) -> anyhow::Result<Bytes> {
        if !self.bitfield.has(piece_i) {
            bail!("Piece {piece_i} is not available");
        };
        if let Ok(piece) = self.parts_file.read_piece(piece_i).await {
            return Ok(piece);
        }

        let piece_length = self.piece_length(piece_i);
        let mut bytes = BytesMut::zeroed(piece_length as usize);

        let piece_start = piece_i as u64 * self.piece_length as u64;
        let piece_end = piece_start + piece_length;

        for (file_idx, file) in self.files.iter().enumerate() {
            let file_start = file.offset;
            let file_end = file.end();
            if file_start > piece_end || file_end < piece_start {
                continue;
            }

            let read_offset = piece_start.saturating_sub(file_start);
            let f = match self.file_handles.opened_files.get_mut(&file_idx) {
                Some(f) => f,
                None => {
                    tracing::debug!("Creating file handle: {}", file.path.display());
                    let file_handle = fs::OpenOptions::new().read(true).open(&file.path).await?;
                    self.file_handles.opened_files.put(file_idx, file_handle);
                    self.file_handles.opened_files.get_mut(&file_idx).unwrap()
                }
            };
            f.seek(SeekFrom::Start(read_offset)).await?;
            let range_start = if piece_start < file_start {
                (file_start - piece_start) as usize
            } else {
                0
            };
            let range_end = if file_end < piece_end {
                (piece_length as u64 - (piece_end - file_end)) as usize
            } else {
                piece_length as usize
            };
            f.read_exact(&mut bytes[range_start..range_end]).await?;
        }
        let bytes = bytes.freeze();
        Ok(bytes)
    }

    pub async fn revalidate(&mut self) -> anyhow::Result<BitField> {
        let mut bitfield = BitField::empty(self.pieces.len());
        let mut current_piece = 0;
        let mut verified_pieces = 0;
        let total_pieces = self.pieces.len();
        let s = sysinfo::System::new();
        let workers = s.physical_core_count().unwrap_or(4);
        let mut hasher = Hasher::new(workers);
        const CONCURRENCY: usize = 50;
        for _ in 0..CONCURRENCY {
            let bytes = self.retrieve_piece(current_piece).await?;
            let payload = Payload {
                hash: self.pieces[current_piece],
                piece_i: current_piece,
                data: vec![bytes],
            };
            hasher.pend_job(payload).await;
            current_piece += 1;
            if current_piece >= total_pieces {
                break;
            }
        }
        loop {
            let res = hasher.recv().await;
            verified_pieces += 1;
            if res.is_verified {
                bitfield.add(res.piece_i).unwrap();
            }

            if verified_pieces >= total_pieces {
                break;
            }

            if current_piece < total_pieces {
                let bytes = self.retrieve_piece(current_piece).await?;
                let payload = Payload {
                    hash: self.pieces[current_piece],
                    piece_i: current_piece,
                    data: vec![bytes],
                };
                current_piece += 1;
                hasher.pend_job(payload).await;
            }
        }
        Ok(bitfield)
    }

    async fn pend_hash_validation(&mut self, piece_i: usize, data: ReadyPiece) {
        let hash = self.pieces.0[piece_i];
        let payload = Payload {
            hash,
            piece_i,
            data: data.0,
        };
        self.hasher.pend_job(payload).await
    }
}

fn split_bytes(bytes: Bytes) -> Vec<Bytes> {
    let block_length = BLOCK_LENGTH as usize;
    let amount = bytes.len() / block_length;
    let mut parts = Vec::with_capacity(amount);
    for i in 0..amount {
        let start = i * block_length;
        if i == amount - 1 {
            parts.push(bytes.slice(start..));
            break;
        };
        let end = start + block_length;
        parts.push(bytes.slice(start..end));
    }
    parts
}
