use std::{io::SeekFrom, path::Path};

use anyhow::ensure;
use bytes::{Bytes, BytesMut};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};

use crate::{
    peers::BitField,
    protocol::{Hashes, Info, OutputFile},
    utils::verify_sha1,
};

#[derive(Debug)]
pub struct TorrentStorage {
    pub output_files: Vec<OutputFile>,
    pub piece_size: u32,
    pub total_length: u64,
    pub pieces: Hashes,
}

#[derive(Debug)]
pub struct StorageHandle {
    pub sender: mpsc::Sender<StorePiece>,
    pub handle: JoinHandle<anyhow::Result<()>>,
}

#[derive(Debug)]
pub enum StorePiece {
    Preallocated {
        piece_i: usize,
        bytes: Bytes,
    },
    Reallocated {
        bitfield: BitField,
        piece_i: usize,
        bytes: Bytes,
    },
}

impl TorrentStorage {
    pub fn new(torrent: &Info, output_dir: impl AsRef<Path>) -> Self {
        Self {
            output_files: torrent.output_files(output_dir),
            piece_size: torrent.piece_length,
            total_length: torrent.total_size(),
            pieces: torrent.pieces.clone(),
        }
    }

    pub fn spawn(self) -> anyhow::Result<StorageHandle> {
        let (tx, rx) = mpsc::channel(100);
        let handle = tokio::spawn(self.work(rx));
        Ok(StorageHandle { sender: tx, handle })
    }

    pub async fn work(mut self, mut reciever: mpsc::Receiver<StorePiece>) -> anyhow::Result<()> {
        loop {
            tokio::select! {
                Some(data) = reciever.recv() => {
                    match data {
                        StorePiece::Preallocated { piece_i, bytes } => self.save_piece_preallocated(piece_i, bytes).await?,
                        StorePiece::Reallocated { bitfield, piece_i, bytes } => self.save_piece(&bitfield, piece_i, bytes).await?,
                    }
                }
            }
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

        let piece_start = piece_i as u64 * self.piece_size as u64;
        let piece_end = piece_start + piece_length as u64;

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
            let file_start = file_offset as u64;
            let file_end = (file_offset + file.length()) as u64;
            let file_range = file_start..file_end;
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length();
                continue;
            }

            let insert_offset = bitfield.pieces().fold(0, |acc, p| {
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

            // NOTE: Not cool
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

            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(file.path())
                .await?;

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

    /// saves piece preallocating all bytes in the file
    pub async fn save_piece_preallocated(
        &mut self,
        piece_i: usize,
        bytes: Bytes,
    ) -> anyhow::Result<()> {
        let piece_length = bytes.len() as u32;
        ensure!(piece_length == self.piece_length(piece_i));

        let piece_start = piece_i as u64 * self.piece_size as u64;
        let piece_end = piece_start + piece_length as u64;

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
            let file_start = file_offset as u64;
            let file_end = (file_offset + file.length()) as u64;
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length();
                continue;
            }

            // NOTE: Not cool
            if let Some(parent) = &file.path().parent() {
                fs::create_dir_all(parent).await?;
            }

            let insert_offset = file_start + piece_start;
            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .write(true)
                .open(file.path())
                .await?;
            file_handle.set_len(file.length()).await?;
            file_handle.seek(SeekFrom::Start(insert_offset)).await?;
            file_handle.write_all(&bytes).await?;
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

pub async fn verify_integrety(path: impl AsRef<Path>, info: &Info) -> anyhow::Result<()> {
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
            Err(e) => return Err(anyhow::anyhow!("worker paniced: {e}")),
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
                dbg!(&current_hash);
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
