use std::path::PathBuf;

use axum::{body::Body, http::StatusCode, response::IntoResponse};
use axum_extra::{
    headers::{ContentType, Range},
    TypedHeader,
};
use bytes::Bytes;
use tokio::io::AsyncReadExt;

use crate::{
    app_state::AppError,
    library::{
        assets::{FileAsset, PreviewAsset, SubtitleAsset},
        Source,
    },
};

pub trait ServeContent {
    /// Serve video file
    #[allow(async_fn_in_trait)]
    async fn serve_video(&self, range: Option<TypedHeader<Range>>) -> impl IntoResponse;

    /// Serve previews
    #[allow(async_fn_in_trait)]
    async fn serve_previews(
        &self,
        number: usize,
    ) -> Result<(TypedHeader<ContentType>, Bytes), AppError>;

    /// Serve subtitles
    #[allow(async_fn_in_trait)]
    async fn serve_subs(&self, id: String) -> Result<String, AppError>;
}

// TODO: stream previews and subtitles from file
impl ServeContent for Source {
    async fn serve_video(&self, range: Option<TypedHeader<Range>>) -> impl IntoResponse {
        self.video.serve(range).await
    }

    async fn serve_previews(
        &self,
        number: usize,
    ) -> Result<(TypedHeader<ContentType>, Bytes), AppError> {
        let preview = PreviewAsset::new(self.id, number);
        let bytes = preview.read_to_bytes().await?;

        return Ok((TypedHeader(ContentType::jpeg()), bytes));
    }

    async fn serve_subs(&self, id: String) -> Result<String, AppError> {
        let subtitle = SubtitleAsset::new(self.id, id);
        let mut file = subtitle.open().await?;
        let mut response = String::new();
        file.read_to_string(&mut response).await?;
        Ok(response)
    }
}
