use tokio::fs;
use tokio::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::codec::{BytesCodec, FramedRead};
use warp::hyper::{header, Body, Response, StatusCode};

use crate::ShowFile;
pub async fn serve_file(
    episode: ShowFile,
    range: Option<String>,
) -> Result<Response<Body>, warp::Rejection> {
    let mut file = fs::File::open(&episode.video_path).await.unwrap();
    let file_size = fs::metadata(&episode.video_path).await.unwrap().len();
    let response = match range {
        Some(range_header) => {
            //parsing header
            println!("{:?}", range_header);

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
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, file_size),
                )
                .header(header::CONTENT_LENGTH, chunk_size.to_string())
                .header(header::CACHE_CONTROL, "public, max-age=0")
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_TYPE, "video/x-matroska")
                .status(StatusCode::PARTIAL_CONTENT)
                .body(Body::wrap_stream(stream_of_bytes))
                .unwrap()
        }
        None => {
            let size = 40_000;
            println!("sending opening response");

            let stream_of_bytes = FramedRead::new(file.take(size as u64), BytesCodec::new());

            Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, "video/x-matroska")
                .header(header::CONTENT_LENGTH, size.to_string())
                .header(header::ACCEPT_RANGES, "bytes")
                .body(Body::wrap_stream(stream_of_bytes))
                .unwrap()
        }
    };

    Ok(response)
}
