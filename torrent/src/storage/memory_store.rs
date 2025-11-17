use std::{
    fmt::Debug,
    io::Cursor,
    ops::DerefMut,
    sync::{Arc, Mutex},
    task::Poll,
};

use crate::storage::parts::PartsResource;

/// Shared in-memory storage
#[derive(Default, Clone)]
pub struct AsyncInMemoryBlock(pub Arc<Mutex<Cursor<Vec<u8>>>>);

impl Debug for AsyncInMemoryBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AsyncInMemoryBlock")
            .field(&self.0.lock().unwrap())
            .finish()
    }
}

impl AsyncInMemoryBlock {
    pub fn len(&self) -> usize {
        self.0.lock().unwrap().get_ref().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl PartialEq<[u8]> for AsyncInMemoryBlock {
    fn eq(&self, other: &[u8]) -> bool {
        self.0.lock().unwrap().get_ref().as_slice() == other
    }
}

impl PartialEq<Vec<u8>> for AsyncInMemoryBlock {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.0.lock().unwrap().get_ref().as_slice() == other
    }
}

impl PartialEq<AsyncInMemoryBlock> for &[u8] {
    fn eq(&self, other: &AsyncInMemoryBlock) -> bool {
        other.0.lock().unwrap().get_ref().as_slice() == *self
    }
}

impl<const N: usize> PartialEq<[u8; N]> for AsyncInMemoryBlock {
    fn eq(&self, other: &[u8; N]) -> bool {
        self.0.lock().unwrap().get_ref().as_slice() == other
    }
}

impl<'a, T: ?Sized> PartialEq<&'a T> for AsyncInMemoryBlock
where
    AsyncInMemoryBlock: PartialEq<T>,
{
    fn eq(&self, other: &&'a T) -> bool {
        *self == **other
    }
}

impl tokio::io::AsyncWrite for AsyncInMemoryBlock {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let mut inner = self.0.lock().unwrap();
        let r = inner.deref_mut();
        tokio::pin!(r);
        tokio::io::AsyncWrite::poll_write(r, cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let mut inner = self.0.lock().unwrap();
        let r = inner.deref_mut();
        tokio::pin!(r);
        tokio::io::AsyncWrite::poll_flush(r, cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let mut inner = self.0.lock().unwrap();
        let r = inner.deref_mut();
        tokio::pin!(r);
        tokio::io::AsyncWrite::poll_shutdown(r, cx)
    }
}

impl tokio::io::AsyncSeek for AsyncInMemoryBlock {
    fn start_seek(
        self: std::pin::Pin<&mut Self>,
        position: std::io::SeekFrom,
    ) -> std::io::Result<()> {
        let mut inner = self.0.lock().unwrap();
        let r = inner.deref_mut();
        tokio::pin!(r);
        tokio::io::AsyncSeek::start_seek(r, position)
    }

    fn poll_complete(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::io::Result<u64>> {
        let mut borrow = self.0.lock().unwrap();
        let r = borrow.deref_mut();
        tokio::pin!(r);
        tokio::io::AsyncSeek::poll_complete(r, cx)
    }
}

impl tokio::io::AsyncRead for AsyncInMemoryBlock {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let mut inner = self.0.lock().unwrap();
        let r = inner.deref_mut();
        tokio::pin!(r);
        tokio::io::AsyncRead::poll_read(r, cx, buf)
    }
}

impl PartsResource for AsyncInMemoryBlock {
    type Item = AsyncInMemoryBlock;

    fn open_io(&self) -> impl Future<Output = std::io::Result<Self::Item>> + Send {
        async { Ok(self.clone()) }
    }

    fn len(io: &Self::Item) -> impl Future<Output = std::io::Result<u64>> + Send
    where
        Self: Sized,
    {
        async { Ok(io.len() as u64) }
    }
}
