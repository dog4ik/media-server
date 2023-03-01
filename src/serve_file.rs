use axum::body::Body;
use axum::http::{HeaderMap, Response, StatusCode};
use tokio::fs;
use tokio::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::codec::{BytesCodec, FramedRead};
pub async fn serve_file(req_header: HeaderMap) -> Response<Body> {
    let mut file = fs::File::open("video.mkv").await.unwrap();
    let file_size = fs::metadata("video.mkv").await.unwrap().len();
    let content_range_header = req_header.get("range");
    let response = match content_range_header {
        Some(range_header) => {
            //parsing header
            let range_header = range_header.to_str().unwrap();
            let mut ranges = range_header.split('=').skip(1);
            let range = ranges.next().unwrap().to_owned();
            let parts: Vec<&str> = range.split('-').collect();
            let start = parts[0].parse().unwrap();
            let end = if let Some(s) = parts.get(1) {
                s.parse().unwrap_or(file_size - 1)
            } else {
                file_size - 1
            };

            let chunk_size = end - start + 1;
            file.seek(SeekFrom::Start(start)).await.unwrap();
            let stream_of_bytes = FramedRead::new(file.take(chunk_size), BytesCodec::new());

            Response::builder()
                .header(
                    "Content-Range",
                    format!("bytes {}-{}/{}", start, end, file_size),
                )
                .header("Content-Length", chunk_size.to_string())
                .header("Accept-Ranges", "bytes")
                .header("Content-Type", "video/x-matroska")
                .status(StatusCode::PARTIAL_CONTENT)
                .body(Body::wrap_stream(stream_of_bytes))
                .unwrap()
        }
        None => {
            let size = 40_000;
            println!("sending opening response");

            let stream_of_bytes = FramedRead::new(file.take(size as u64), BytesCodec::new());

            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "video/x-matroska")
                .header("Content-Length", size.to_string())
                .header("Accept-Ranges", "bytes")
                .body(Body::wrap_stream(stream_of_bytes))
                .unwrap()
        }
    };

    response
}
