use axum_extra::{TypedHeader, headers};

use crate::torrent::PendingTorrent;

impl PendingTorrent {
    pub async fn handle_request(
        &self,
        _file_start: u64,
        _file_size: u64,
        _range: Option<TypedHeader<headers::Range>>,
    ) {
        todo!()
    }
}
