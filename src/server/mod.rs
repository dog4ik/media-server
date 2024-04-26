use std::path::Path;

use crate::metadata::{ContentType, MetadataProvider};
use axum::response::IntoResponse;
use axum_extra::{headers::Range, TypedHeader};
use serde::Deserialize;

pub mod admin_api;
pub mod content;
pub mod public_api;

#[derive(Deserialize)]
pub struct PageQuery {
    pub page: Option<usize>,
}

#[derive(Deserialize)]
pub struct IdQuery {
    pub id: i64,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub search: String,
}

#[derive(Deserialize)]
pub struct ContentTypeQuery {
    pub content_type: ContentType,
}

#[derive(Deserialize)]
pub struct ProviderQuery {
    pub provider: MetadataProvider,
}

#[derive(Deserialize)]
pub struct VariantQuery {
    pub variant: String,
}

#[derive(Deserialize)]
pub struct StringIdQuery {
    pub id: String,
}

#[derive(Deserialize)]
pub struct SeasonQuery {
    pub season: usize,
}

#[derive(Deserialize)]
pub struct EpisodeQuery {
    pub episode: usize,
}

#[derive(Deserialize)]
pub struct NumberQuery {
    pub number: usize,
}

#[derive(Deserialize)]
pub struct LanguageQuery {
    pub lang: Option<String>,
}

#[derive(Deserialize)]
pub struct TakeParam {
    pub take: Option<usize>,
}
async fn serve_video(
    path: impl AsRef<Path>,
    range: Option<TypedHeader<Range>>,
) -> impl IntoResponse {
    use std::io::SeekFrom;

    use axum::{body::Body, http::HeaderMap};
    use reqwest::{header, StatusCode};
    use tokio::fs;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    use tokio_util::codec::{BytesCodec, FramedRead};

    let mut file = fs::File::open(path).await.unwrap();
    let metadata = file.metadata().await.unwrap();
    let file_size = metadata.len();
    let range = range.map(|h| h.0).unwrap_or(Range::bytes(0..).unwrap());
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

    let chunk_size = end - start + 1;
    file.seek(SeekFrom::Start(start)).await.unwrap();
    let stream_of_bytes = FramedRead::new(file.take(chunk_size), BytesCodec::new());
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from(end - start),
    );
    // headers.insert(
    //     header::CONTENT_TYPE,
    //     header::HeaderValue::from_static(self.guess_mime_type()),
    // );
    headers.insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("public, max-age=0"),
    );
    headers.insert(
        header::CONTENT_RANGE,
        header::HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end - 1, file_size))
            .unwrap(),
    );

    return (
        StatusCode::PARTIAL_CONTENT,
        headers,
        Body::from_stream(stream_of_bytes),
    );
}
