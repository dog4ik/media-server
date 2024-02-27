use std::io::SeekFrom;

use anyhow::ensure;
use bytes::{Bytes, BytesMut};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use crate::{
    file::{Hashes, Info, OutputFile},
    peers::BitField,
    utils::verify_sha1,
};

trait SyncBitfield {
    async fn save(&mut self, bitfield: BitField) -> anyhow::Result<()>;
}

#[derive(Debug)]
pub struct TorrentStorage {
    pub output_files: Vec<OutputFile>,
    pub piece_size: u32,
    pub total_length: u64,
    pub pieces: Hashes,
}

impl TorrentStorage {
    pub fn new(torrent: &Info) -> Self {
        Self {
            output_files: torrent.all_files(),
            piece_size: torrent.piece_length,
            total_length: torrent.total_size(),
            pieces: torrent.pieces.clone(),
        }
    }

    /// Helper function to get piece length with consideration of the last piece
    fn piece_length(&self, piece_i: usize) -> u32 {
        crate::utils::piece_size(
            piece_i,
            self.piece_size as usize,
            self.total_length as usize,
        ) as u32
    }

    pub async fn save_piece(
        &mut self,
        bitfield: &BitField,
        piece_i: usize,
        bytes: Bytes,
    ) -> anyhow::Result<()> {
        let piece_length = bytes.len() as u32;
        ensure!(piece_length == self.piece_length(piece_i));

        let piece_start = piece_i as u32 * self.piece_size;
        let piece_end = piece_start + piece_length;

        let hash = self.pieces.get_hash(piece_i).unwrap();
        if verify_sha1(hash, &bytes) {
            tracing::trace!("Successfuly verified hash of piece {}", piece_i);
        } else {
            let msg = format!("Failed to verify hash of piece {}", piece_i);
            tracing::error!("{}", msg);
            return Err(anyhow::anyhow!("{}", msg));
        };
        dbg!(piece_i);

        let mut file_offset = 0;
        for file in &self.output_files {
            let file_start = file_offset as u32;
            let file_end = (file_offset + file.length()) as u32;
            let file_range = file_start..file_end;
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length();
                continue;
            }

            let insert_offset = bitfield.pieces().fold(0, |acc, p| {
                if p > piece_i {
                    return acc;
                }
                let p_len = self.piece_length(p);
                let p_start = self.piece_size * p as u32;
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

            if let Some(parent) = &file.path().parent() {
                fs::create_dir_all(parent).await?;
            }

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

            println!();
            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(&file.path())
                .await?;

            println!();
            file_handle
                .seek(SeekFrom::Start(insert_offset.into()))
                .await?;

            let mut buffer = Vec::new();
            file_handle.read_to_end(&mut buffer).await.unwrap();

            file_handle
                .seek(SeekFrom::Start(insert_offset.into()))
                .await
                .unwrap();

            file_handle.write_all(&bytes[start..end]).await.unwrap();
            file_handle.write_all(&buffer).await.unwrap();

            file_offset += file.length();
        }

        Ok(())
    }

    pub async fn retrieve_piece(&self, bitfield: &BitField, piece_i: usize) -> Option<Bytes> {
        let piece_length = self.piece_length(piece_i);
        println!("Piece {} was requested by peer", piece_i);

        let piece_start = piece_i as u32 * self.piece_size;
        let piece_end = piece_start + piece_length;

        let hash = self.pieces.get_hash(piece_i).unwrap();

        let mut file_offset = 0;
        let mut out = BytesMut::with_capacity(piece_length as usize);
        for file in &self.output_files {
            let file_start = file_offset as u32;
            let file_end = (file_offset + file.length()) as u32;
            let file_range = file_start..file_end;
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length();
                continue;
            }

            let read_offset = bitfield.pieces().fold(0, |acc, p| {
                if p > piece_i {
                    return acc;
                }
                let p_len = self.piece_length(p);
                let p_start = self.piece_size * p as u32;
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
                "Reading piece {} in file {} with offset {}",
                piece_i,
                &file.path().display(),
                read_offset
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
                .open(&file.path())
                .await
                .ok()?;

            file_handle
                .seek(SeekFrom::Start(read_offset.into()))
                .await
                .ok()?;

            file_handle.read_exact(&mut out[start..end]).await.unwrap();

            file_offset += file.length();
        }
        let out: Bytes = out.into();
        if verify_sha1(hash, &out) {
            return Some(out);
        }
        panic!("Failed to verify hash of downloaded piece {}", piece_i);
    }
}
