use std::io::SeekFrom;

use bytes::{Bytes, BytesMut};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use crate::{
    download::piece_size,
    file::{Hashes, OutputFile, TorrentFile},
    peers::BitField,
    utils::verify_sha1,
};

trait SyncBitfield {
    async fn save(&mut self, bitfield: BitField) -> anyhow::Result<()>;
}

#[derive(Debug)]
pub struct FileBitfield {
    file: fs::File,
}

impl FileBitfield {
    pub async fn from_hex_hash(hash: &str) -> anyhow::Result<Self> {
        let file = fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(format!(".{}", hash))
            .await?;
        Ok(Self { file })
    }
}

impl SyncBitfield for FileBitfield {
    async fn save(&mut self, bitfield: BitField) -> anyhow::Result<()> {
        self.file.set_len(0).await?;
        self.file.write_all(&bitfield.0).await?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct TorrentStorage {
    pub output_files: Vec<OutputFile>,
    pub piece_size: u32,
    pub total_length: u64,
    pub pieces: Hashes,
}

impl TorrentStorage {
    pub fn new(torrent: &TorrentFile) -> Self {
        Self {
            output_files: torrent.info.all_files(),
            piece_size: torrent.info.piece_length,
            total_length: torrent.info.total_size(),
            pieces: torrent.info.pieces.clone(),
        }
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        piece_size(
            piece_i as u32,
            self.piece_size as u32,
            self.pieces.len() as u32,
            self.total_length as u32,
        )
    }

    pub async fn save_piece(
        &mut self,
        bitfield: &BitField,
        piece_i: usize,
        bytes: Bytes,
    ) -> anyhow::Result<()> {
        let piece_length = bytes.len() as u32;
        let piece_start = piece_i as u32 * self.piece_size;
        let piece_end = piece_start + piece_length;

        let hash = self.pieces.get_hash(piece_i).unwrap();
        // BUG: for some unknown reason piece with index 0 is always fail
        // be careful of race condition when writing the file
        if verify_sha1(hash, &bytes) {
            tracing::trace!("Successfuly verified hash of piece {}", piece_i);
        } else {
            let msg = format!("Failed to verify hash of piece {}", piece_i);
            tracing::trace!("{}", msg);
            panic!("{}", msg);
        };

        let mut file_offset = 0;
        for file in &self.output_files {
            let file_start = file_offset as u32;
            let file_end = (file_offset + file.length) as u32;
            let file_range = file_start..file_end;
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length;
                continue;
            }

            let insert_offset = bitfield.pieces().fold(0, |acc, p| {
                if p > piece_i {
                    return acc;
                }
                let p_len = self.piece_length(p);
                let p_start = p_len * p as u32;
                let p_end = p_start + p_len;
                let contains_start = file_range.contains(&p_start);
                let contains_end = file_range.contains(&p_end);
                if contains_start && contains_end {
                    return acc + p_len;
                } else if contains_start {
                    return acc + (file_start - p_end) as u32;
                } else if contains_end {
                    unreachable!(
                        "contains_end is not possible without contains_start, iterated piece must be ahead of saved piece"
                    );
                } else {
                    return acc
                }
            });
            tracing::debug!(
                "Saving piece {} in file {} with offset {}",
                piece_i,
                &file.path.display(),
                insert_offset
            );

            if let Some(parent) = &file.path.parent() {
                fs::create_dir_all(parent).await?;
            }
            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(&file.path)
                .await?;
            file_handle
                .seek(SeekFrom::Start(insert_offset.into()))
                .await?;

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
                // end is beyound file
                piece_length - relative_end.abs() as u32
            } else {
                // end is behind file
                piece_length
            } as usize;

            tracing::debug!("start/end {}/{}", start, end);
            file_handle.write_all(&bytes[start..end]).await?;
            file_offset += file.length;
        }
        Ok(())
    }

    pub async fn retrieve_piece(&self, bitfield: &BitField, piece_i: usize) -> Option<Bytes> {
        let piece_length = self.piece_length(piece_i);
        let piece_start = piece_i as u32 * self.piece_size;
        let piece_end = piece_start + piece_length;
        let mut file_offset = 0;
        let mut out = BytesMut::with_capacity(piece_length as usize);
        for file in &self.output_files {
            let file_start = file_offset as u32;
            let file_end = (file_offset + file.length) as u32;
            let file_range = file_start..file_end;
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length;
                continue;
            }

            let read_offset = bitfield.pieces().fold(0, |acc, p| {
                let p_len = self.piece_length(p);
                let p_start = self.piece_size * p as u32;
                let p_end = p_start + p_len;
                if p > piece_i {
                    return acc;
                }
                let contains_start = file_range.contains(&p_start);
                let contains_end = file_range.contains(&p_end);
                if contains_start && contains_end {
                    return acc + p_len;
                } else if contains_start {
                    return acc + (file_range.start - p_end) as u32;
                } else if contains_end {
                    unreachable!(
                        "contains_end is not possible without contains_start, iterated piece must be ahead of saved piece"
                    );
                } else {
                    return acc;
                }
            });
            let mut file_handle = fs::OpenOptions::new()
                .write(true)
                .open(&file.path)
                .await
                .ok()?;
            let file_size = file_handle.metadata().await.unwrap().len();
            let cursor_position = file_handle
                .seek(SeekFrom::Start(read_offset.into()))
                .await
                .ok()?;
            let mut buf = BytesMut::with_capacity(std::cmp::min(
                piece_length.into(),
                file_size - cursor_position,
            ) as usize);
            file_handle.read_exact(&mut buf).await.ok()?;
            out.extend_from_slice(&buf);
            file_offset += file.length;
        }
        if out.len() == piece_length as usize {
            tracing::debug!("Retrieved piece {}", piece_i);
            return Some(out.into());
        } else {
            None
        }
    }
}
