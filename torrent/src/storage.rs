use std::{
    io::SeekFrom,
    ops::Range,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context};
use bytes::{Bytes, BytesMut};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot, watch},
    task::JoinSet,
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use crate::{
    peers::BitField,
    protocol::{Hashes, Info, OutputFile},
    scheduler::BLOCK_LENGTH,
    utils::{verify_iter_sha1, verify_sha1},
};

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
pub struct TorrentStorage {
    pub output_dir: PathBuf,
    pub output_files: Vec<OutputFile>,
    pub piece_size: u32,
    pub total_length: u64,
    pub pieces: Hashes,
    pub bitfield: BitField,
    pub enabled_files: BitField,
}

#[derive(Debug, Clone)]
pub struct StorageHandle {
    pub piece_tx: mpsc::Sender<StorageMessage>,
    pub bitfield: watch::Receiver<BitField>,
    pub cancellation_token: CancellationToken,
}

impl StorageHandle {
    pub async fn save_piece(
        &self,
        insert_piece: usize,
        blocks: Vec<Bytes>,
        response: mpsc::Sender<StorageFeedback>,
    ) {
        self.piece_tx
            .send(StorageMessage::Save {
                piece_i: insert_piece,
                blocks,
                response,
            })
            .await
            .unwrap();
    }

    pub fn try_save_piece(
        &self,
        insert_piece: usize,
        blocks: Vec<Bytes>,
        response: mpsc::Sender<StorageFeedback>,
    ) -> anyhow::Result<()> {
        self.piece_tx.try_send(StorageMessage::Save {
            piece_i: insert_piece,
            blocks,
            response,
        })?;
        Ok(())
    }
    pub async fn retrieve_piece(&self, piece_i: usize, response: mpsc::Sender<StorageFeedback>) {
        self.piece_tx
            .send(StorageMessage::RetrievePiece { piece_i, response })
            .await
            .unwrap();
    }
    pub async fn retrieve_blocking(&self, piece_i: usize) -> Option<Bytes> {
        let (tx, rx) = oneshot::channel();
        self.piece_tx
            .send(StorageMessage::RetrieveBlocking {
                piece_i,
                response: tx,
            })
            .await
            .unwrap();
        rx.await.unwrap()
    }
    pub async fn enable_file(&self, file_idx: usize) {
        self.piece_tx
            .send(StorageMessage::EnableFile { file_idx })
            .await
            .unwrap();
    }
    pub async fn disable_file(&self, file_idx: usize) {
        self.piece_tx
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
        response: mpsc::Sender<StorageFeedback>,
    },
    EnableFile {
        file_idx: usize,
    },
    DisableFile {
        file_idx: usize,
    },
    RetrievePiece {
        piece_i: usize,
        response: mpsc::Sender<StorageFeedback>,
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
    pub fn new(info: &Info, output_dir: impl AsRef<Path>, enabled_files: &[usize]) -> Self {
        let output_files = info.output_files(&output_dir);
        let mut files_bitfield = BitField::empty(output_files.len());
        for enabled_idx in enabled_files {
            files_bitfield.add(*enabled_idx).unwrap();
        }

        Self {
            output_dir: output_dir.as_ref().to_path_buf(),
            output_files,
            piece_size: info.piece_length,
            total_length: info.total_size(),
            pieces: info.pieces.clone(),
            bitfield: BitField::empty(info.pieces.len()),
            enabled_files: files_bitfield,
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
        let (piece_tx, mut piece_rx) = mpsc::channel(1000);
        let (mut state_tx, state_rx) = watch::channel(self.bitfield.clone());
        let token = cancellation_token.clone();
        tracker.spawn(async move {
            loop {
                tokio::select! {
                    Some(message) = piece_rx.recv() => self.handle_message(message, &mut state_tx).await,
                    _ = token.cancelled() => {
                        break;
                    }
                }
            }
        });
        Ok(StorageHandle {
            piece_tx,
            bitfield: state_rx,
            cancellation_token,
        })
    }

    async fn handle_message(
        &mut self,
        message: StorageMessage,
        state: &mut watch::Sender<BitField>,
    ) {
        match message {
            StorageMessage::Save {
                piece_i,
                blocks,
                response,
            } => {
                let save_result = self
                    .save_piece_preallocated(piece_i, ReadyPiece(blocks))
                    .await;
                match save_result {
                    Ok(_) => {
                        let _ = response.send(StorageFeedback::Saved { piece_i }).await;
                        self.bitfield.add(piece_i).unwrap();
                        state.send_modify(|old| old.add(piece_i).unwrap())
                    }
                    Err(_) => {
                        let _ = response.send(StorageFeedback::Failed { piece_i }).await;
                    }
                }
            }
            StorageMessage::RetrieveBlocking { piece_i, response } => {
                let bytes = self.retrieve_piece(piece_i).await.ok();
                let _ = response.send(bytes);
            }
            StorageMessage::RetrievePiece { piece_i, response } => {
                let bytes = self.retrieve_piece(piece_i).await.ok();
                let _ = response
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

    /// Save piece reallocating every time "gap" occurs
    pub async fn save_piece(&mut self, piece_i: usize, bytes: Bytes) -> anyhow::Result<()> {
        let piece_length = bytes.len() as u32;
        ensure!(piece_length == self.piece_length(piece_i));

        let piece_start = piece_i as u64 * self.piece_size as u64;
        let piece_end = piece_start + piece_length as u64;

        let hash = self.pieces.get_hash(piece_i).unwrap();
        if !verify_sha1(hash, &bytes) {
            let msg = format!("Failed to verify hash of piece {}", piece_i);
            tracing::error!(msg);
            return Err(anyhow::anyhow!(msg));
        };

        let mut file_offset = 0;
        for file in &self.output_files {
            let file_start = file_offset;
            let file_end = file_offset + file.length();
            let file_range = file_start..file_end;
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length();
                continue;
            }

            let insert_offset = self.bitfield.pieces().fold(0, |acc, p| {
                if p > piece_i {
                    return acc;
                }
                let p_len = self.piece_length(p) as u64;
                let p_start = self.piece_size as u64 * p as u64;
                let p_end = p_start + p_len;
                let contains_start = file_range.contains(&p_start);
                let contains_end = file_range.contains(&p_end);
                if contains_start && contains_end {
                    acc + p_len
                } else if contains_start {
                    acc + file_start - p_end
                } else if contains_end {
                    acc + p_end - file_start
                } else {
                    acc
                }
            });
            tracing::debug!(
                "Saving piece {} in file {} with offset {}",
                piece_i,
                &file.path().display(),
                insert_offset
            );

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

            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(file.path())
                .await?;

            file_handle.seek(SeekFrom::Start(insert_offset)).await?;

            let mut buffer = Vec::new();
            file_handle.read_to_end(&mut buffer).await.unwrap();

            file_handle
                .seek(SeekFrom::Start(insert_offset))
                .await
                .unwrap();

            file_handle.write_all(&bytes[start..end]).await.unwrap();
            file_handle.write_all(&buffer).await.unwrap();

            file_offset += file.length();
        }

        Ok(())
    }

    /// saves piece preallocating all the bytes in the file with null byte
    pub async fn save_piece_preallocated(
        &mut self,
        piece_i: usize,
        blocks: ReadyPiece,
    ) -> anyhow::Result<()> {
        let piece_length = blocks.len() as u32;
        ensure!(piece_length == self.piece_length(piece_i));

        let piece_start = piece_i as u64 * self.piece_size as u64;
        let piece_end = piece_start + piece_length as u64;

        let hash = self.pieces.get_hash(piece_i).unwrap();
        if !verify_iter_sha1(hash, blocks.0.iter()) {
            let msg = format!("Failed to verify hash of piece {}", piece_i);
            tracing::error!(msg);
            return Err(anyhow::anyhow!(msg));
        };

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
            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(file.path())
                .await?;
            file_handle.set_len(file.length()).await?;
            file_handle.seek(SeekFrom::Start(insert_offset)).await?;

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
            blocks.write_to(&mut file_handle, start..end).await?;
            //file_handle.write_all(&bytes[start..end]).await?;
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
        for file in &self.output_files {
            let file_start = file_offset;
            let file_end = file_offset + file.length();
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length();
                continue;
            }

            let insert_offset = piece_start.checked_sub(file_start).unwrap_or_default();
            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .read(true)
                .open(file.path())
                .await?;
            file_handle.seek(SeekFrom::Start(insert_offset)).await?;
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
            file_handle
                .read_exact(&mut bytes[range_start..range_end])
                .await?;
            file_offset += file.length();
        }
        let bytes = bytes.freeze();
        let hash = self.pieces.get_hash(piece_i).unwrap();
        if !verify_sha1(hash, &bytes) {
            panic!("Failed to verify hash of downloaded piece");
        };
        Ok(bytes)
    }
}

pub async fn verify_integrity(path: impl AsRef<Path>, info: &Info) -> anyhow::Result<()> {
    let workers_amount = 5;
    let chunk_size = std::cmp::max(info.pieces.len() / workers_amount, 1);

    let mut worker_set = JoinSet::new();
    for (i, hashes) in info.pieces.chunks(chunk_size).map(Vec::from).enumerate() {
        worker_set.spawn(verify_worker(
            i,
            hashes,
            info.output_files(&path),
            info.piece_length as usize,
            chunk_size,
        ));
    }
    while let Some(result) = worker_set.join_next().await {
        match result {
            Ok(Err(e)) => return Err(anyhow::anyhow!("failed to verify file: {e}")),
            Ok(_) => continue,
            Err(e) => return Err(anyhow::anyhow!("worker panicked: {e}")),
        }
    }
    Ok(())
}

async fn verify_worker(
    idx: usize,
    hashes: Vec<[u8; 20]>,
    output_files: Vec<OutputFile>,
    piece_length: usize,
    chunk_size: usize,
) -> anyhow::Result<()> {
    let start_piece = idx * chunk_size;
    let start_byte = start_piece as u64 * piece_length as u64;
    let end_byte = start_byte + piece_length as u64;
    let mut file_offset = 0;
    let mut hashes = hashes.into_iter();
    let mut buffer = BytesMut::with_capacity(piece_length);
    let Some(mut current_hash) = hashes.next() else {
        return Ok(());
    };
    for file in output_files {
        let file_start = file_offset;
        let file_end = file_offset + file.length();
        if file_start > end_byte || file_end < start_byte {
            file_offset += file.length();
            continue;
        }

        let handle = fs::File::open(file.path()).await?;
        let mut buf_reader = BufReader::new(handle);
        if file_start < start_byte {
            buf_reader
                .seek(SeekFrom::Start(start_byte - file_start))
                .await
                .unwrap();
        }

        while buf_reader.read_buf(&mut buffer).await? != 0 {
            if buffer.len() == buffer.capacity() {
                if !verify_sha1(current_hash, &buffer) {
                    println!("{:?}", buffer.as_ref());
                    return Err(anyhow::anyhow!("Hash verification failed"));
                };
                let Some(next_hash) = hashes.next() else {
                    return Ok(());
                };
                current_hash = next_hash;
                buffer.clear();
            }
        }
        file_offset += file.length();
        println!("EOF");
        continue;
    }

    Ok(())
}
