use std::{io::SeekFrom, path::PathBuf};

use axum::{
    body::StreamBody,
    headers::{ContentType, Range},
    http::{HeaderName, HeaderValue},
    response::AppendHeaders,
    TypedHeader,
};
use bytes::Bytes;
use reqwest::{header, StatusCode};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncSeekExt},
};
use tokio_util::codec::{BytesCodec, FramedRead};

use crate::library::LibraryItem;

pub trait ServeContent {
    /// Serve video file
    async fn serve_video(
        &self,
        range: Range,
    ) -> (
        StatusCode,
        AppendHeaders<[(HeaderName, HeaderValue); 6]>,
        StreamBody<FramedRead<tokio::io::Take<File>, BytesCodec>>,
    );

    /// Serve previews
    async fn serve_previews(
        &self,
        number: i32,
    ) -> Result<(TypedHeader<ContentType>, Bytes), StatusCode>;

    /// Serve subtitles
    async fn serve_subs(&self, lang: Option<String>) -> Result<String, StatusCode>;
}

impl<T: LibraryItem> ServeContent for T {
    async fn serve_video(
        &self,
        range: Range,
    ) -> (
        StatusCode,
        AppendHeaders<[(HeaderName, HeaderValue); 6]>,
        StreamBody<FramedRead<tokio::io::Take<File>, BytesCodec>>,
    ) {
        let mut file = tokio::fs::File::open(&self.source_path()).await.unwrap();
        let file_size = file.metadata().await.unwrap().len();
        let (start, end) = range.iter().next().expect("at least one tuple");
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

        return (
            StatusCode::PARTIAL_CONTENT,
            AppendHeaders([
                (
                    header::CONTENT_RANGE,
                    HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end - 1, file_size))
                        .unwrap(),
                ),
                (header::CONTENT_LENGTH, HeaderValue::from(end - start)),
                (
                    header::CACHE_CONTROL,
                    HeaderValue::from_str("public, max-age=0").unwrap(),
                ),
                (
                    header::ACCESS_CONTROL_ALLOW_ORIGIN,
                    HeaderValue::from_str("*").unwrap(),
                ),
                (
                    header::ACCEPT_RANGES,
                    HeaderValue::from_str("bytes").unwrap(),
                ),
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_str("video/x-matroska").unwrap(),
                ),
            ]),
            stream_of_bytes.into(),
        );
    }

    async fn serve_previews(
        &self,
        number: i32,
    ) -> Result<(TypedHeader<ContentType>, Bytes), StatusCode> {
        let path = PathBuf::from(self.previews_path().to_str().unwrap());
        let mut previews_dir = tokio::fs::read_dir(path).await.unwrap();

        while let Some(file) = previews_dir.next_entry().await.unwrap() {
            let file_path = file.path();
            let file_number: i32 = file_path
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .parse()
                .expect("file to contain only numbers");
            if file_number == number {
                let bytes: Bytes = tokio::fs::read(file_path).await.unwrap().into();
                return Ok((TypedHeader(ContentType::jpeg()), bytes));
            }
        }
        return Err(StatusCode::NO_CONTENT);
    }

    async fn serve_subs(&self, lang: Option<String>) -> Result<String, StatusCode> {
        get_subtitles(self.subtitles_path(), lang)
            .await
            .ok_or(StatusCode::NO_CONTENT)
    }
}

async fn get_subtitles(path: PathBuf, lang: Option<String>) -> Option<String> {
    let mut subs_dir = tokio::fs::read_dir(path).await.unwrap();
    let mut subs: Option<String> = None;
    while let Some(file) = subs_dir.next_entry().await.unwrap() {
        let file_path = file.path();
        let file_name = file_path.file_stem().unwrap().to_str().unwrap();

        subs = match &lang {
            Some(lang) => {
                if file_name == lang {
                    Some(tokio::fs::read_to_string(file.path()).await.unwrap())
                } else {
                    continue;
                }
            }
            None => {
                if &file_name == &"unknown" || &file_name == &"eng" {
                    Some(tokio::fs::read_to_string(file_path).await.unwrap())
                } else {
                    continue;
                }
            }
        };
    }
    return subs;
}
pub trait ServePreviews {}
