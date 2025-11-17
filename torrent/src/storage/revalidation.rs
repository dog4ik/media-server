use crate::storage::{
    TorrentStorage, hash_verification::Hasher, parts::PartsResource, sink::StorageSink,
};

#[derive(Debug)]
pub struct TorrentValidator<'a, T, P> {
    pub storage: &'a mut TorrentStorage<T, P>,
    pub hasher: &'a mut Hasher,
}

impl<S: StorageSink, P: PartsResource> TorrentValidator<'_, S, P> {
    pub async fn revalidate<T>(&mut self, mut on_progress: T)
    where
        T: AsyncFnMut(usize, bool) -> anyhow::Result<()> + 'static,
    {
        let mut current_piece = 0;
        let total_pieces = self.hasher.hashes.len();
        const CONCURRENCY: usize = 10;
        tracing::debug!(
            total_pieces,
            concurrency = CONCURRENCY,
            "Started torrent validation"
        );

        while current_piece < total_pieces {
            if self.hasher.len() < CONCURRENCY {
                if let Ok(bytes) = self.storage.retrieve_piece(current_piece).await {
                    self.hasher.pend_job(current_piece, vec![bytes]);
                } else {
                    self.storage.bitfield.remove(current_piece).unwrap();
                    if on_progress(current_piece, false).await.is_err() {
                        tracing::warn!("Download disconnected during validation");
                        return;
                    };
                    current_piece += 1;
                };
            } else {
                while let Some(res) = self.hasher.join_next().await {
                    if res.is_verified {
                        self.storage.bitfield.add(res.piece_i).unwrap();
                    } else {
                        self.storage.bitfield.remove(res.piece_i).unwrap();
                    }
                    if on_progress(res.piece_i, res.is_verified).await.is_err() {
                        tracing::warn!("Download disconnected during validation");
                        return;
                    };
                    current_piece += 1;
                }
            }
        }
    }
}
