use std::{io::SeekFrom, path::PathBuf};

use bytes::{Bytes, BytesMut};
use tokio::{
    fs,
    io::{self, AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
};

use crate::utils::LengthCalculator;

/// My attpmpt to store only parts of piece that are not in enabled files in .parts file
#[allow(unused)]
mod unstable {
    use std::io::SeekFrom;

    use bytes::BytesMut;
    use tokio::{
        fs,
        io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    };

    use crate::{DownloadParams, OutputFile, storage::ReadyPiece};

    fn file_bounds(files: &[File]) -> Box<[(usize, usize)]> {
        files.iter().map(|v| (v.start_piece, v.end_piece)).collect()
    }

    #[derive(Debug, Clone, Copy)]
    enum BorderSide {
        Left,
        Right,
    }

    #[derive(Debug, Clone)]
    struct Slot {
        left_file: usize,
        piece: usize,
        piece_offset: u64,
        offset: u64,
        length: u64,
        side: BorderSide,
    }

    impl Slot {
        pub fn right_file_idx(&self) -> usize {
            self.left_file + 1
        }
    }

    #[derive(Debug)]
    struct File {
        start_byte: u64,
        end_byte: u64,
        start_piece: usize,
        end_piece: usize,
    }

    impl File {
        pub fn from_output_files(output_files: &[OutputFile], piece_length: u64) -> Vec<File> {
            let mut offset = 0;
            let mut out = Vec::new();
            for file in output_files {
                let length = file.length();
                let start = offset;
                let end = start + length;

                let start_piece = (start / piece_length) as usize;
                let end_piece = ((end - 1) / piece_length) as usize;

                out.push(Self {
                    start_byte: start,
                    end_byte: end,
                    start_piece,
                    end_piece,
                });
                offset += length;
            }

            out
        }
    }

    /// ### Rules of border pieces
    /// We put border piece in parts file only when all conditions met:
    /// 1. Overlapping neighbor file is disabled
    /// 2. Current bitfield does not contain this piece (this piece is not already in parts file)
    ///
    /// We should restructure it when:
    /// - One of the disabled files gets enabled.
    /// In that case we move piece data in newly enabled output file and remove border piece from parts
    /// file
    /// - Added piece that shared between files where one of the files is disabled
    ///
    /// Border piece exists in parts file when:
    /// Bitfield contains border piece and one of the neighbor files is disabled
    ///
    /// ### Active or enabled files?
    /// Using active(files that are already created) will save some space compared only enabled
    #[derive(Debug)]
    #[allow(non_camel_case_types)]
    pub struct PartsFile_unstable {
        file: fs::File,
        slots: Vec<Slot>,
        piece_length: u64,
        file_bounds: Box<[(usize, usize)]>,
        created_files: Box<[bool]>,
    }

    async fn created_files(files: &[OutputFile]) -> Box<[bool]> {
        let mut out = Vec::with_capacity(files.len());
        for file in files {
            out.push(fs::try_exists(file.path()).await.unwrap_or(false));
        }
        out.into_boxed_slice()
    }

    async fn active_files(files: &[OutputFile]) -> Box<[bool]> {
        let mut out = Vec::with_capacity(files.len());
        for file in files {
            out.push(fs::try_exists(file.path()).await.unwrap_or(false));
        }
        out.into_boxed_slice()
    }

    impl PartsFile_unstable {
        pub async fn open(params: &DownloadParams) -> anyhow::Result<Self> {
            let enabled_files: Vec<_> = params.files.iter().map(|f| !f.is_disabled()).collect();
            let info = &params.info;
            let bf = &params.bitfield;
            let location = params
                .save_location
                .join(format!(".{}.parts", info.hex_hash()));
            let files = info.output_files("");
            let created_files = created_files(&files).await;
            let file = fs::OpenOptions::new()
                .write(true)
                .read(true)
                .create(true)
                .open(&location)
                .await?;
            let metadata = file.metadata().await?;
            let piece_length = info.piece_length as u64;
            let files = File::from_output_files(&files, piece_length);
            let file_bounds = file_bounds(&files);
            debug_assert_eq!(files.len(), file_bounds.len());
            debug_assert_eq!(files.len(), enabled_files.len());

            let mut slots: Vec<Slot> = Vec::new();

            for (i, ((_, file_end), (next_start, _))) in
                file_bounds.windows(2).map(|v| (v[0], v[1])).enumerate()
            {
                if file_end != next_start {
                    println!("Skipping aligned files: {} {}", i, i + 1);
                    // skip if files are aligned
                    continue;
                }
                if !bf.has(file_end) {
                    println!("We don't have border piece: {file_end}");
                    continue;
                }
                if enabled_files[i] ^ enabled_files[i + 1] {
                    let side = if enabled_files[i] {
                        BorderSide::Right
                    } else {
                        BorderSide::Left
                    };

                    let border_byte = files[i].end_byte;
                    let piece_start = file_end as u64 * piece_length;
                    let piece_end = piece_start + piece_length;
                    let length = match side {
                        BorderSide::Left => border_byte - piece_start,
                        BorderSide::Right => piece_end - border_byte,
                    };
                    let piece_offset = match side {
                        BorderSide::Left => 0,
                        BorderSide::Right => border_byte - piece_start,
                    };
                    let offset = slots.iter().fold(0, |acc, s| acc + s.length);
                    // let offset = slots.last().map_or(0, |v| v.offset + v.length);
                    slots.push(Slot {
                        left_file: i,
                        piece: file_end,
                        piece_offset,
                        offset,
                        length,
                        side,
                    });
                }
            }

            debug_assert_eq!(metadata.len(), slots.iter().map(|v| v.length).sum::<u64>());

            Ok(Self {
                file,
                slots,
                piece_length,
                file_bounds,
                created_files,
            })
        }

        pub async fn write_piece(
            &mut self,
            piece_i: usize,
            piece: &ReadyPiece,
        ) -> anyhow::Result<()> {
            let mut part_offset = 0;
            let Some(slot) = self.slots.iter().find(|s| {
                part_offset += s.length;
                s.piece == piece_i
            }) else {
                anyhow::bail!("slot for piece {piece_i} is not found")
            };

            let position = SeekFrom::Start(part_offset);
            self.file.seek(position).await?;
            // todo: precalculate capacity
            let mut buf = Vec::new();
            self.file.read_to_end(&mut buf).await?;
            self.file.seek(position).await?;
            let piece_start = slot.piece_offset as usize;
            let piece_end = piece_start + slot.length as usize;
            piece
                .write_to(&mut self.file, piece_start..piece_end)
                .await?;
            self.file.write_all(&buf).await?;

            Ok(())
        }

        pub async fn read_part(
            &mut self,
            piece_i: usize,
            bytes: &mut BytesMut,
        ) -> anyhow::Result<()> {
            let Some(slot) = self.slots.iter().find(|s| s.piece == piece_i) else {
                anyhow::bail!("Could not find slot for piece {piece_i}");
            };
            self.file.seek(SeekFrom::Start(slot.offset)).await?;
            let piece_start = slot.piece_offset as usize;
            let piece_end = piece_start + slot.length as usize;
            self.file
                .read_exact(&mut bytes[piece_start..piece_end])
                .await?;
            Ok(())
        }
    }
}

/// TODO: Come back and use GAT with lifetime after this issue is resolved.
/// https://github.com/rust-lang/rust/issues/100013
/// Or compile with RUSTFLAGS=-Zhigher-ranked-assumptions
/// This will allow `open_io` to borrow GAT from self and remove immutability from test code
pub trait PartsResource: Send {
    type Item: io::AsyncRead + io::AsyncWrite + io::AsyncSeek + Unpin + Send;

    fn open_io(&self) -> impl Future<Output = std::io::Result<Self::Item>> + Send;
    fn len(io: &Self::Item) -> impl Future<Output = std::io::Result<u64>> + Send
    where
        Self: Sized;
}

impl PartsResource for PartsPath {
    type Item = fs::File;

    async fn open_io(&self) -> std::io::Result<Self::Item> {
        tracing::debug!(path = %self.0.display(), "Opening .parts file");
        fs::OpenOptions::new()
            .write(true)
            .read(true)
            .create(true)
            .open(&self.0)
            .await
    }

    async fn len(io: &Self::Item) -> std::io::Result<u64> {
        io.metadata().await.map(|m| m.len())
    }
}

#[derive(Debug)]
pub struct PartsPath(PathBuf);

impl PartsPath {
    pub fn new(save_location: &std::path::Path, hex_hash: &str) -> Self {
        Self(save_location.join(format!(".{}.parts", hex_hash)))
    }
}

/// Simple implementation of parts file
/// Layout of this file is [4 bytes piece index + full piece]
#[derive(Debug)]
pub struct PartsFile<T> {
    pieces: Vec<usize>,
    io: T,
    piece_length_measurer: LengthCalculator,
}

impl<T> PartsFile<T>
where
    T: PartsResource + Send,
{
    pub fn init(
        measurer: LengthCalculator,
        io: T,
    ) -> impl Future<Output = anyhow::Result<PartsFile<T>>> + Send {
        async move {
            let mut pieces = Vec::new();
            match io.open_io().await {
                Ok(mut file) => {
                    let len = T::len(&file).await?;
                    let mut position = 0;
                    while position < len {
                        file.seek(SeekFrom::Start(position)).await?;
                        let piece = file.read_u32().await? as usize;
                        let piece_length = measurer.piece_length(piece) as u64;
                        pieces.push(piece);
                        position += 4 + piece_length;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => Err(e)?,
            }

            tracing::debug!("Initiated .parts file with {} parts", pieces.len());

            Ok(Self {
                pieces,
                io,
                piece_length_measurer: measurer,
            })
        }
    }

    pub async fn write_piece(&mut self, piece_i: usize, piece: &[Bytes]) -> anyhow::Result<()> {
        debug_assert_eq!(
            self.piece_length_measurer.piece_length(piece_i),
            piece.iter().map(|p| p.len() as u32).sum::<u32>(),
            "piece {piece_i} has unexpected length that will ruin alignment of .parts file",
        );

        if self.pieces.contains(&piece_i) {
            tracing::error!("Attempt to write duplicate piece {piece_i} into .parts file");
            return Ok(());
        }
        let mut file = self.io.open_io().await?;
        tracing::debug!("Writing piece {piece_i} in .parts file");
        file.seek(SeekFrom::End(0)).await?;
        file.write_u32(piece_i as u32).await?;
        for block in piece {
            file.write_all(block).await?;
        }
        file.flush().await?;
        self.pieces.push(piece_i);
        Ok(())
    }

    #[cfg(test)]
    pub fn get_ref(&self) -> &T {
        &self.io
    }

    pub async fn read_piece(&mut self, piece_i: usize) -> anyhow::Result<Bytes> {
        let mut read_position = 0;
        for &piece in &self.pieces {
            let piece_length = self.piece_length_measurer.piece_length(piece);
            if piece == piece_i {
                tracing::debug!("Read piece {piece_i} from .parts file");
                let mut file = self.io.open_io().await?;
                file.seek(SeekFrom::Start(read_position)).await?;
                let idx = file.read_u32().await?;
                anyhow::ensure!(idx == piece_i as u32);
                let mut piece = BytesMut::zeroed(piece_length as usize);
                file.read_exact(&mut piece).await?;
                return Ok(piece.into());
            } else {
                read_position += 4 + piece_length as u64;
            }
        }
        Err(anyhow::anyhow!("piece {piece_i} is not found"))
    }
}

#[cfg(test)]
mod tests {
    const LEN: LengthCalculator = LengthCalculator::new(11, 2);

    use crate::storage::{memory_store::AsyncInMemoryBlock, split_bytes};

    use super::*;

    #[tokio::test]
    async fn parts_read_write_init() {
        let mut parts = PartsFile::init(LEN, AsyncInMemoryBlock::default())
            .await
            .unwrap();
        let inner = parts.get_ref();
        let mut parts_length = inner.len();
        assert_eq!(inner.len(), 0);

        let arr0 = [0, 1];
        let piece = Bytes::copy_from_slice(&arr0);
        parts.write_piece(0, &split_bytes(piece)).await.unwrap();
        parts_length += 4 + arr0.len();

        let inner = parts.get_ref();
        assert_eq!(inner.len(), parts_length);

        let arr2 = [3, 4];
        let piece = Bytes::copy_from_slice(&arr2);
        parts.write_piece(2, &split_bytes(piece)).await.unwrap();
        parts_length += 4 + arr2.len();
        let inner = parts.get_ref();
        assert_eq!(inner.len(), parts_length);

        let arr_last = [9];
        let piece = Bytes::copy_from_slice(&arr_last);
        let last_piece = parts.piece_length_measurer.total_pieces - 1;
        parts
            .write_piece(last_piece, &split_bytes(piece))
            .await
            .unwrap();
        parts_length += 4 + arr_last.len();
        let inner = parts.get_ref();
        assert_eq!(inner.len(), parts_length);

        assert_eq!(arr0.as_slice(), parts.read_piece(0).await.unwrap());
        assert_eq!(arr2.as_slice(), parts.read_piece(2).await.unwrap());
        assert_eq!(
            arr_last.as_slice(),
            parts.read_piece(last_piece).await.unwrap()
        );

        parts
            .read_piece(1)
            .await
            .expect_err("piece should not be found");

        let inner = parts.io;
        let mut reconstructed_parts = PartsFile::init(LEN, inner).await.unwrap();
        assert_eq!(reconstructed_parts.pieces, [0, 2, 5]);

        assert_eq!(
            arr0.as_slice(),
            reconstructed_parts.read_piece(0).await.unwrap()
        );
        assert_eq!(
            arr2.as_slice(),
            reconstructed_parts.read_piece(2).await.unwrap()
        );
        assert_eq!(
            arr_last.as_slice(),
            reconstructed_parts.read_piece(last_piece).await.unwrap()
        );
    }
}
