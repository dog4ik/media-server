use tokio::fs;
use tokio::io::{AsyncRead, AsyncSeek, AsyncWrite};

#[cfg(test)]
use crate::storage::memory_store::AsyncInMemoryBlock;
use crate::storage::{FileHandles, StorageFile};

/// Store of write/read/seek compatible items that are accessed in Storage
pub trait StorageSink {
    type Sink: AsyncWrite + AsyncRead + AsyncSeek + Unpin;
    async fn open<'a>(
        &'a mut self,
        idx: usize,
        file: &StorageFile,
    ) -> std::io::Result<&'a mut Self::Sink>;
}

impl StorageSink for FileHandles {
    type Sink = fs::File;

    async fn open<'a>(
        &'a mut self,
        idx: usize,
        file: &StorageFile,
    ) -> std::io::Result<&'a mut Self::Sink> {
        // Ahh rust
        if self.0.get_mut(&idx).is_some() {
            return Ok(self.0.get_mut(&idx).unwrap());
        }

        if let Some(parent) = file.path.parent() {
            fs::create_dir_all(parent).await?;
        }
        tracing::debug!("Creating file handle: {}", file.path.display());
        let file_handle = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&file.path)
            .await?;
        file_handle.set_len(file.length).await?;
        self.0.put(idx, file_handle);
        Ok(self.0.get_mut(&idx).unwrap())
    }
}

#[cfg(test)]
impl StorageSink for Vec<AsyncInMemoryBlock> {
    type Sink = AsyncInMemoryBlock;

    async fn open<'a>(
        &'a mut self,
        idx: usize,
        file: &StorageFile,
    ) -> std::io::Result<&'a mut Self::Sink> {
        use tokio::io::AsyncWriteExt;

        let block = self.get_mut(idx).ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "in-memory file is not found",
        ))?;
        if block.is_empty() {
            block.write_all(&vec![0; file.length as usize]).await?;
        }
        Ok(block)
    }
}
