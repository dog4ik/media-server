use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue},
    response::IntoResponse,
};
use axum_extra::{headers, TypedHeader};
use bytes::Bytes;
use reqwest::{header, StatusCode};
use tokio::sync::mpsc;
use torrent::ScheduleStrategy;

use crate::torrent::PendingTorrent;

impl PendingTorrent {
    pub async fn handle_request(
        &self,
        file_start: u64,
        file_size: u64,
        range: Option<TypedHeader<headers::Range>>,
    ) -> impl IntoResponse {
        let file_end = file_start + file_size;
        let range = range
            .map(|h| h.0)
            .unwrap_or(headers::Range::bytes(0..).unwrap());
        let (stream_tx, stream_rx) = mpsc::channel::<anyhow::Result<Bytes>>(5);
        let mut storage_handle = self.download_handle.storage.clone();
        let (start, end) = range
            .satisfiable_ranges(file_size)
            .next()
            .expect("at least one tuple");
        let start = match start {
            std::ops::Bound::Included(val) => val,
            std::ops::Bound::Excluded(val) => val,
            std::ops::Bound::Unbounded => 0,
        };

        let end = match end {
            std::ops::Bound::Included(val) => val,
            std::ops::Bound::Excluded(val) => val,
            std::ops::Bound::Unbounded => file_size,
        };
        let range = start + file_start..end + file_end;
        let piece_size = self.torrent_info.piece_length as usize;
        let mut current_piece = range.start / piece_size as u64;
        self.download_handle
            .set_strategy(ScheduleStrategy::Request(current_piece as usize))
            .await
            .unwrap();
        let stream = tokio_stream::wrappers::ReceiverStream::new(stream_rx);

        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from(end - start),
        );
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("video/matroska"),
        );
        headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
        headers.insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=0"),
        );
        headers.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end - 1, file_size)).unwrap(),
        );

        (
            StatusCode::PARTIAL_CONTENT,
            headers,
            Body::from_stream(stream),
        )
    }
}
