use std::{io::SeekFrom, path::Path};

use bytes::{Bytes, BytesMut};
use tokio::{
    fs,
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader},
    task::JoinSet,
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
    pub fn new(torrent: &Info, output_dir: impl AsRef<Path>) -> Self {
        Self {
            output_files: torrent.output_files(output_dir),
            piece_size: torrent.piece_length,
            total_length: torrent.total_size(),
            pieces: torrent.pieces.clone(),
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
        ensure!(piece_length == self.piece_length(piece_i));

        let piece_start = piece_i as u64 * self.piece_size as u64;
        let piece_end = piece_start + piece_length as u64;

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
            let file_start = file_offset as u64;
            let file_end = (file_offset + file.length()) as u64;
            let file_range = file_start..file_end;
            if file_start > piece_end || file_end < piece_start {
                file_offset += file.length;
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

            // NOTE: Not cool
            if let Some(parent) = &file.path().parent() {
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

            let mut file_handle = fs::OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(&file.path())
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
