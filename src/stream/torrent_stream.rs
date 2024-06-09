use std::ops::Range;

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue},
    response::IntoResponse,
};
use axum_extra::{headers, TypedHeader};
use bytes::Bytes;
use reqwest::{header, StatusCode};
use tokio::sync::mpsc;
use torrent::scheduler::ScheduleStrategy;

use crate::torrent::TorrentDownload;

impl TorrentDownload {
    pub async fn handle_request(
        &self,
        file_range: Range<u64>,
        range: Option<TypedHeader<headers::Range>>,
    ) -> impl IntoResponse {
        let file_size = file_range.end - file_range.start;
        let file_start = file_range.start;
        let file_end = file_range.end;
        let range = range.map(|h| h.0).unwrap_or(headers::Range::bytes(0..).unwrap());
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
        let range = start + file_range.start..end + file_range.end;
        let piece_size = self.piece_size as usize;
        let mut current_piece = range.start / self.piece_size as u64;
        self.download_handle
            .set_strategy(ScheduleStrategy::PieceRequest {
                piece: current_piece as usize,
            })
            .await
            .unwrap();
        tokio::spawn(async move {
            while let Ok(_) = storage_handle.bitfield.changed().await {
                let have = {
                    let bf = storage_handle.bitfield.borrow_and_update();
                    bf.has(current_piece as usize)
                };
                if have {
                    tracing::info!("Retrieving piece: {}", current_piece);
                    let bytes = storage_handle
                        .retrieve_blocking(current_piece as usize)
                        .await
                        .unwrap();
                    let piece_length = bytes.len() as u32;
                    let piece_start = current_piece * piece_size as u64;
                    let piece_end = piece_start + piece_length as u64;
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

                    if let Ok(_) = stream_tx.send(Ok(bytes.slice(start..end))).await {
                        current_piece += 1;
                    } else {
                        // channel closed
                        break;
                    }
                }
            }
        });
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

        return (
            StatusCode::PARTIAL_CONTENT,
            headers,
            Body::from_stream(stream),
        );
    }
}
