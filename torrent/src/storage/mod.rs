use std::{io::SeekFrom, ops::Range, path::PathBuf, time::Instant};

use anyhow::Context;
use bytes::{Bytes, BytesMut};
use hash_verification::Hasher;
use parts::PartsFile;
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};
use tokio_util::task::TaskTracker;

use crate::{
    DownloadParams, Priority, bitfield::BitField, length_calculator::LengthCalculator,
    protocol::OutputFile, scheduler::BLOCK_LENGTH, storage::revalidation::TorrentValidator,
};

mod error;
pub mod hash_verification;
#[cfg(test)]
mod memory_store;
pub mod parts;
pub mod revalidation;
mod sink;

pub use error::StorageError;

pub type Result<T> = std::result::Result<T, error::StorageError>;

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
            // This one panics when file gets enabled
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

/// Lru cache of file handles
#[derive(Debug)]
pub struct FileHandles(lru::LruCache<usize, fs::File>);

impl FileHandles {
    pub fn new() -> Self {
        use std::num::NonZeroUsize;
        Self(lru::LruCache::new(NonZeroUsize::new(3).unwrap()))
    }
}

#[derive(Debug, Clone)]
pub struct StorageFile {
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
pub struct TorrentStorage<T, P> {
    files: Box<[StorageFile]>,
    piece_length_measurer: LengthCalculator,
    bitfield: BitField,
    // Cache of opened file handles
    sinks: T,
    parts_file: PartsFile<P>,
}

#[derive(Debug, Clone)]
pub struct StorageHandle {
    pub message_tx: mpsc::Sender<StorageMessage>,
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

    pub async fn validate(&self) {
        self.message_tx
            .send(StorageMessage::Validate)
            .await
            .unwrap()
    }
}

#[derive(Debug)]
pub enum StorageMessage {
    Save { piece_i: usize, blocks: Vec<Bytes> },
    EnableFile { file_idx: usize },
    DisableFile { file_idx: usize },
    RetrievePiece { piece_i: usize },
    Validate,
}

#[derive(Debug)]
pub enum StorageFeedback {
    ValidationProgress { piece: usize, is_valid: bool },
    Saved { piece_i: usize },
    Data { piece_i: usize, bytes: Bytes },
    Error { piece_i: usize, error: StorageError },
}

impl<T: sink::StorageSink, P: parts::PartsResource> TorrentStorage<T, P> {
    pub fn new(sinks: T, parts_file: PartsFile<P>, torrent_params: &DownloadParams) -> Self {
        let info = &torrent_params.info;
        let output_dir = torrent_params.save_location.clone();
        let bitfield = torrent_params.bitfield.clone();
        let output_files = info.output_files(&output_dir);
        let files = StorageFile::new_files(&output_files, &torrent_params.files);
        let piece_length_measurer = LengthCalculator::new(info.total_size(), info.piece_length);

        Self {
            files,
            bitfield,
            sinks,
            parts_file,
            piece_length_measurer,
        }
    }

    pub fn bitfield(&self) -> &BitField {
        &self.bitfield
    }

    pub fn base_piece_length(&self) -> u64 {
        self.piece_length_measurer.piece_length as u64
    }

    pub async fn enable_file(&mut self, file_idx: usize) {
        let file = &mut self.files[file_idx];
        file.is_enabled = true;
        let file = file.clone();
        let file_offset = file.offset;
        let start_piece = (file_offset / self.base_piece_length()) as usize;
        if let Ok(bytes) = self.parts_file.read_piece(start_piece).await {
            let _ = self
                .save_piece(start_piece, ReadyPiece(split_bytes(bytes)))
                .await;
        }

        let end_piece = ((file.end() - 1) / self.base_piece_length()) as usize;
        if let Ok(bytes) = self.parts_file.read_piece(end_piece).await {
            let _ = self
                .save_piece(end_piece, ReadyPiece(split_bytes(bytes)))
                .await;
        }
    }

    /// saves piece filling file with null bytes
    /// WARN: this will not validate piece hash
    pub async fn save_piece(&mut self, piece_i: usize, blocks: ReadyPiece) -> Result<()> {
        let piece_length = blocks.len() as u64;
        debug_assert_eq!(
            piece_length as u32,
            self.piece_length_measurer.piece_length(piece_i)
        );

        let piece_start = piece_i as u64 * self.piece_length_measurer.piece_length as u64;
        let piece_end = piece_start + piece_length;

        for (file_idx, file) in self.files.iter().enumerate() {
            let file_start = file.offset;
            let file_end = file.end();
            if file_start >= piece_end || file_end <= piece_start {
                continue;
            }

            let file_end_piece = file.end_piece(self.base_piece_length());
            let file_start_piece = file.start_piece(self.base_piece_length());
            if !file.is_enabled && (piece_i == file_end_piece || piece_i == file_start_piece) {
                let border_next = self.files.get(file_idx + 1).is_some_and(|next| {
                    next.start_piece(self.base_piece_length()) == file_end_piece
                });
                let border_prev = file_idx
                    .checked_sub(1)
                    .and_then(|i| self.files.get(i))
                    .is_some_and(|prev| {
                        prev.end_piece(self.base_piece_length()) == file_start_piece
                    });
                if border_next || border_prev {
                    if let Err(e) = self.parts_file.write_piece(piece_i, &blocks.0).await {
                        tracing::error!("Failed to write piece {piece_i} to the parts file: {e}");
                    };
                }
                continue;
            }

            let f = self.sinks.open(file_idx, &file).await?;
            let insert_offset = piece_start.saturating_sub(file_start);
            f.seek(SeekFrom::Start(insert_offset)).await?;

            let start = if piece_start < file_start {
                (file_start - piece_start) as usize
            } else {
                0
            };
            let end = if file_end < piece_end {
                (piece_length - (piece_end - file_end)) as usize
            } else {
                piece_length as usize
            };

            blocks.write_to(f, start..end).await?;
        }

        self.bitfield.add(piece_i).unwrap();
        Ok(())
    }

    /// retrieve piece from preallocated file
    pub async fn retrieve_piece(&mut self, piece_i: usize) -> Result<Bytes> {
        if !self.bitfield.has(piece_i) {
            return Err(error::StorageError::MissingPiece);
        };
        if let Ok(piece) = self.parts_file.read_piece(piece_i).await {
            return Ok(piece);
        }

        let piece_length = self.piece_length_measurer.piece_length(piece_i) as u64;
        let mut bytes = BytesMut::zeroed(piece_length as usize);

        let piece_start = piece_i as u64 * self.base_piece_length();
        let piece_end = piece_start + piece_length;

        for (file_idx, file) in self.files.iter().enumerate() {
            let file_start = file.offset;
            let file_end = file.end();
            if file_start >= piece_end || file_end <= piece_start {
                continue;
            }

            let read_offset = piece_start.saturating_sub(file_start);
            let f = self.sinks.open(file_idx, &file).await?;
            f.seek(SeekFrom::Start(read_offset)).await?;
            let range_start = if piece_start < file_start {
                (file_start - piece_start) as usize
            } else {
                0
            };
            let range_end = if file_end < piece_end {
                (piece_length - (piece_end - file_end)) as usize
            } else {
                piece_length as usize
            };
            f.read_exact(&mut bytes[range_start..range_end]).await?;
        }
        let bytes = bytes.freeze();
        Ok(bytes)
    }
}

pub async fn spawn(
    download_params: &DownloadParams,
    tracker: &TaskTracker,
    tx: mpsc::Sender<StorageFeedback>,
    parts: PartsFile<parts::PartsPath>,
) -> anyhow::Result<StorageHandle> {
    let (message_tx, mut message_rx) = mpsc::channel(1800);
    let mut hasher = Hasher::new(download_params.info.pieces.clone());
    let file_handles = FileHandles::new();
    let mut storage = TorrentStorage::new(file_handles, parts, download_params);
    tracker.spawn(async move {
        loop {
            tokio::select! {
                message = message_rx.recv() => match message {
                    Some(message) => {
                        match message {
                            StorageMessage::Save { piece_i, blocks } => {
                                hasher.pend_job(piece_i, blocks);
                            }
                            StorageMessage::RetrievePiece { piece_i } => {
                                match storage.retrieve_piece(piece_i).await {
                                    Ok(bytes) => {
                                        let _ = tx.send(StorageFeedback::Data { piece_i, bytes }).await;
                                    },
                                    Err(error)  => {
                                        let _ = tx.send(StorageFeedback::Error { piece_i, error }).await;
                                    },
                                };
                            }
                            StorageMessage::EnableFile { file_idx } => storage.enable_file(file_idx).await,
                            StorageMessage::DisableFile { file_idx } => {
                                storage.files[file_idx].is_enabled = false;
                            }
                            StorageMessage::Validate => {
                                tracing::trace!("Received validation command");
                                let mut revalidator = TorrentValidator { storage: &mut storage, hasher: &mut hasher };
                                {
                                    let tx = tx.clone();
                                    revalidator.revalidate(async move |piece, is_valid| tx.send(StorageFeedback::ValidationProgress { piece,  is_valid }).await.context("download is available")).await;
                                }
                            }
                        };
                    },
                    None => {
                        tracing::debug!("Stopping storage worker (download channel closed)");
                        break;
                    }
                },
                Some(result) = hasher.join_next() => {
                    let piece_i = result.piece_i;
                    if result.is_verified {
                        let is_old = storage.bitfield.has(piece_i);
                        let start = Instant::now();
                        let save_result = storage.save_piece(piece_i, ReadyPiece(result.blocks)).await;
                        tracing::trace!(took = ?start.elapsed(), "Saved piece {piece_i} on the disk");
                        match save_result {
                            Ok(_) => {
                                // Stupid hack to avoid sending saved message when the file gets enabled.
                                if !is_old {
                                    let _ = tx.send(StorageFeedback::Saved { piece_i }).await;
                                }
                            }
                            Err(error) => {
                                tracing::warn!("Failed to save piece {piece_i}: {error}");
                                let _ = tx.send(StorageFeedback::Error { piece_i, error }).await;
                            }
                        }
                    } else {
                        let _ = tx.send(StorageFeedback::Error {
                            piece_i,
                            error: error::StorageError::Hash,
                        })
                        .await;
                    }
            },
            }
        }
    });
    Ok(StorageHandle { message_tx })
}

fn split_bytes(bytes: Bytes) -> Vec<Bytes> {
    let block_length = BLOCK_LENGTH as usize;
    let amount = bytes.len().div_ceil(block_length);
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

#[cfg(test)]
mod tests {
    use super::split_bytes;
    use crate::{
        StorageError,
        length_calculator::LengthCalculator,
        scheduler::BLOCK_LENGTH,
        storage::{
            StorageFile, TorrentStorage, memory_store::AsyncInMemoryBlock, parts::PartsFile,
        },
    };
    use bytes::Bytes;

    #[derive(Debug)]
    struct StorageTester<'a> {
        piece_length: u32,
        content: &'a [u8],
        storage: TorrentStorage<Vec<AsyncInMemoryBlock>, AsyncInMemoryBlock>,
    }

    #[derive(Debug)]
    struct TestBuilder<'a> {
        piece_length: u32,
        target_content: &'a [u8],
        files: Vec<StorageFile>,
    }

    impl<'a> TestBuilder<'a> {
        pub fn new(piece_length: u32, target_content: &'a [u8]) -> Self {
            Self {
                piece_length,
                target_content,
                files: Vec::new(),
            }
        }

        pub fn add_file(self, length: u64) -> Self {
            self.file(length, true)
        }

        pub fn add_disabled_file(self, length: u64) -> Self {
            self.file(length, false)
        }

        fn file(mut self, length: u64, is_enabled: bool) -> Self {
            let offset = self.files.iter().map(|f| f.length).sum();
            self.files.push(StorageFile {
                offset,
                length,
                path: Default::default(),
                is_enabled,
            });
            self
        }

        pub async fn build(self) -> StorageTester<'a> {
            assert_eq!(
                self.files.iter().map(|f| f.length as usize).sum::<usize>(),
                self.target_content.len(),
                "files do not match content"
            );
            let measurer =
                LengthCalculator::new(self.target_content.len() as u64, self.piece_length);
            let parts_file = PartsFile::init(measurer.clone(), AsyncInMemoryBlock::default())
                .await
                .unwrap();
            let bitfield = crate::BitField::empty(
                self.target_content
                    .len()
                    .div_ceil(self.piece_length as usize),
            );
            let internal_files = (0..self.files.len())
                .map(|_| AsyncInMemoryBlock::default())
                .collect();
            let storage = TorrentStorage {
                files: self.files.into_boxed_slice(),
                piece_length_measurer: measurer.clone(),
                bitfield,
                sinks: internal_files,
                parts_file,
            };
            StorageTester {
                piece_length: self.piece_length,
                content: self.target_content,
                storage,
            }
        }
    }

    impl<'a> StorageTester<'a> {
        pub async fn checked_insert(&mut self, piece: usize) -> super::Result<()> {
            let start = piece * self.piece_length as usize;
            let end = start + self.storage.piece_length_measurer.piece_length(piece) as usize;
            let data_slice = &self.content[start..end];
            self.storage
                .save_piece(
                    piece,
                    super::ReadyPiece(split_bytes(Bytes::copy_from_slice(data_slice))),
                )
                .await?;
            assert!(
                self.storage.bitfield().has(piece),
                "bitfild does contain just inserted piece"
            );

            let data = self.storage.retrieve_piece(piece).await?;

            assert_eq!(
                data_slice, data,
                "inserted and retrieved data is not the same"
            );

            Ok(())
        }
    }

    #[tokio::test]
    async fn each_piece_is_file() {
        let contents: Vec<_> = (0..8).map(|v| v).collect();

        let mut t = TestBuilder::new(2, &contents)
            .add_file(2)
            .add_file(2)
            .add_file(2)
            .add_file(2)
            .build()
            .await;

        t.checked_insert(0).await.unwrap();
        t.checked_insert(2).await.unwrap();

        assert_eq!(t.storage.sinks[0], [0, 1]);
        assert_eq!(t.storage.sinks[1], []);
        assert_eq!(t.storage.sinks[2], [4, 5]);
        assert_eq!(t.storage.sinks[3], &[]);
        assert_eq!(t.storage.parts_file.get_ref().len(), 0)
    }

    #[tokio::test]
    async fn parts_file_populates_file() {
        let contents: Vec<_> = (0..5).map(|v| v).collect();

        let mut t = TestBuilder::new(3, &contents)
            .add_file(2)
            .add_disabled_file(2)
            .add_file(1)
            .build()
            .await;

        t.checked_insert(0).await.unwrap();
        let expected_parts: Vec<_> = 0_u32.to_be_bytes().into_iter().chain([0, 1, 2]).collect();
        assert_eq!(*t.storage.parts_file.get_ref(), expected_parts);

        t.storage.enable_file(1).await;
        assert_eq!(t.storage.sinks[1], [2, 0]);
    }

    #[tokio::test]
    async fn out_of_bounds_access_errors() {
        use std::assert_matches;
        let contents: Vec<_> = (0..5).map(|v| v).collect();

        let mut t = TestBuilder::new(3, &contents).add_file(5).build().await;

        t.checked_insert(0).await.unwrap();
        t.checked_insert(1).await.unwrap();
        assert!(
            t.storage.retrieve_piece(1).await.is_ok(),
            "in bound piece should succeed",
        );
        assert_matches!(
            t.storage.retrieve_piece(2).await,
            Err(StorageError::MissingPiece),
            "out of bounds retrieve should fail with error"
        );
    }

    #[test]
    fn test_bytes_splitting_one() {
        let arr = [0, 1, 2];
        let bytes = Bytes::copy_from_slice(&arr);
        let split = split_bytes(bytes);
        assert_eq!(split.len(), 1);
        assert_eq!(split[0], arr.as_slice());
    }

    #[test]
    fn test_bytes_splitting_common() {
        let bytes = [0; BLOCK_LENGTH as usize * 3 + 778];
        let split = split_bytes(Bytes::copy_from_slice(&bytes));
        assert_eq!(split.len(), 4);
        for block in &split[..3] {
            assert_eq!(block.len(), BLOCK_LENGTH as usize);
        }
        assert_eq!(split[3].len(), 778);
    }
}
