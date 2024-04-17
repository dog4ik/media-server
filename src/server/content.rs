use std::path::PathBuf;

use axum::{http::StatusCode, response::IntoResponse};
use axum_extra::{
    headers::{ContentType, Range},
    TypedHeader,
};
use bytes::Bytes;

use crate::library::Source;

pub trait ServeContent {
    /// Serve video file
    #[allow(async_fn_in_trait)]
    async fn serve_video(&self, range: Option<TypedHeader<Range>>) -> impl IntoResponse;

    /// Serve previews
    #[allow(async_fn_in_trait)]
    async fn serve_previews(
        &self,
        number: usize,
    ) -> Result<(TypedHeader<ContentType>, Bytes), StatusCode>;

    /// Serve subtitles
    #[allow(async_fn_in_trait)]
    async fn serve_subs(&self, lang: Option<String>) -> Result<String, StatusCode>;
}

impl ServeContent for Source {
    async fn serve_video(&self, range: Option<TypedHeader<Range>>) -> impl IntoResponse {
        self.origin.serve(range).await
    }

    async fn serve_previews(
        &self,
        number: usize,
    ) -> Result<(TypedHeader<ContentType>, Bytes), StatusCode> {
        let path = PathBuf::from(self.previews_path().to_str().unwrap());
        let mut previews_dir = tokio::fs::read_dir(path).await.unwrap();

        while let Some(file) = previews_dir.next_entry().await.unwrap() {
            let file_path = file.path();
            let file_number: usize = file_path
                .file_stem()
                .unwrap()
                .to_os_string()
                .into_string()
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
