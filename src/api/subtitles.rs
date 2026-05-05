use std::path::PathBuf;

use anyhow::Context;
use axum::{
    extract::{Multipart, State},
    response::IntoResponse,
};
use axum_extra::{headers, response::FileStream};
use tokio_stream::StreamExt;
use tokio_util::io::ReaderStream;

use crate::{
    api::{Json, NumberQuery, Path, Query},
    app_state::{AppError, AppState},
    db::{self, Db, DbActions},
    library::assets::{self, FileAsset},
};

/// Pull subtitle from video file using its track number
#[utoipa::path(
    get,
    path = "/api/video/{id}/pull_subtitle",
    params(
        ("id", description = "video id"),
        NumberQuery,
    ),
    responses(
        (status = 200, description = "Subtitles", body = String),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Subtitles",
)]
pub async fn pull_video_subtitle(
    Path(video_id): Path<i64>,
    Query(number): Query<NumberQuery>,
    State(state): State<AppState>,
) -> Result<String, AppError> {
    state
        .pull_subtitle_from_video(video_id, number.number)
        .await
}

#[derive(Debug, utoipa::ToSchema)]
pub struct MultipartSubtitles {
    pub language: Option<String>,
    #[schema(format = Binary, value_type = String, content_media_type = "application/octet-stream")]
    pub subtitles: bytes::Bytes,
}

impl MultipartSubtitles {
    pub async fn from_multipart(multipart: &mut Multipart) -> anyhow::Result<Self> {
        let mut language = None;
        let mut subtitles = None;
        while let Ok(Some(field)) = multipart.next_field().await {
            if let Some("language") = field.name() {
                language = field.text().await.ok();
                continue;
            }
            let data = field.bytes().await?;
            subtitles = Some(data);
        }

        Ok(Self {
            subtitles: subtitles.context("get subtitles field")?,
            language,
        })
    }
}

/// Upload subtitles on the server
#[utoipa::path(
    post,
    path = "/api/video/{id}/upload_subtitles",
    params(
        ("id", description = "video id"),
    ),
    request_body(content = inline(MultipartSubtitles), content_type = "multipart/form-data"),
    responses(
        (status = 200),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Subtitles",
)]
pub async fn upload_subtitles(
    Path(video_id): Path<i64>,
    State(db): State<Db>,
    mut multipart: Multipart,
) -> Result<(), AppError> {
    let mut language = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some("language") = field.name() {
            language = field.text().await.ok();
            continue;
        }
        if let Some("subtitles") = field.name() {
            let file_stem = field.file_name().map(Into::into).unwrap_or_default();
            let db_subtitles = db::DbSubtitles {
                id: None,
                file_stem,
                external_path: None,
                language,
                video_id,
            };
            let mut tx = db.begin().await?;
            let subtitles_id = tx.insert_subtitles(&db_subtitles).await?;
            let subtitles_asset = assets::SubtitleAsset::new(video_id, subtitles_id);

            use std::io::{Error, ErrorKind};
            let mut stream = field.map(|data| data.map_err(|e| Error::new(ErrorKind::Other, e)));
            let output_path = subtitles_asset.path();
            if let Some(parent) = output_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            crate::ffmpeg::convert_and_save_srt(&output_path, &mut stream).await?;

            if tx.commit().await.is_err() {
                tracing::error!("Failed to commit subtitles transaction");
                if let Err(e) = subtitles_asset.delete_file().await {
                    tracing::error!(
                        path = %output_path.display(),
                        "Failed to clean up subtitles file: {e}"
                    );
                };
            };
            return Ok(());
        }
    }

    Err(AppError::bad_request(
        "multipart does not contain required subtitles field",
    ))
}

#[derive(Debug, serde::Deserialize, utoipa::ToSchema)]
pub struct SubtitlesReferencePayload {
    language: Option<String>,
    path: String,
}

/// Create subtitles entry using path reference.
///
/// This types of subtitles are just references to user files and not stored in server assets
/// directory.
///
/// TODO:
/// Read more about subtitles references here
#[utoipa::path(
    post,
    path = "/api/video/{id}/reference_subtitles",
    params(
        ("id", description = "video id"),
    ),
    request_body(content = SubtitlesReferencePayload),
    responses(
        (status = 200, description = "Subtitles are referenced successfully"),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Subtitles",
)]
pub async fn reference_external_subtitles(
    Path(video_id): Path<i64>,
    State(db): State<Db>,
    Json(reference): Json<SubtitlesReferencePayload>,
) -> Result<(), AppError> {
    if !reference.path.ends_with(".srt") {
        tracing::trace!(path = reference.path, "Rejecting subtitles reference path");
        return Err(AppError::bad_request("only .srt files can be referenced"));
    }
    let file_stem: String = std::path::Path::new(&reference.path)
        .file_stem()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();

    let db_subtitles = db::DbSubtitles {
        id: None,
        language: reference.language,
        file_stem,
        external_path: Some(reference.path),
        video_id,
    };
    db.insert_subtitles(&db_subtitles).await?;
    Ok(())
}

/// Delete subtitles on the server
///
/// Note that if subtitles are referenced it will not delete referenced file
#[utoipa::path(
    delete,
    path = "/api/subtitles/{id}",
    params(
        ("id", description = "subtitles id"),
    ),
    responses(
        (status = 200, description = "Subtitles are successfully deleted"),
        (status = 404, description = "Subtitles are not found", body = AppError),
    ),
    tag = "Subtitles",
)]
pub async fn delete_subtitles(Path(id): Path<i64>, State(db): State<Db>) -> Result<(), AppError> {
    let removed_subs = sqlx::query!(
        "DELETE FROM subtitles WHERE id = ? RETURNING video_id, external_path",
        id
    )
    .fetch_one(&db.pool)
    .await?;

    // if subtitles are not referenced delete the asset
    if removed_subs.external_path.is_none() {
        let video_id = removed_subs.video_id;
        let subtitles_asset = assets::SubtitleAsset::new(video_id, id);
        subtitles_asset.delete_file().await.inspect_err(|e| {
            tracing::error!(id, video_id, "Failed to deleted subtitles asset: {e}");
        })?;
        tracing::info!(id, video_id, "Deleted subtitles asset");
    }

    Ok(())
}

/// Get subtitles in text format
#[utoipa::path(
    get,
    path = "/api/subtitles/{id}",
    params(
        ("id", description = "subtitles id"),
    ),
    responses(
        (status = 200, description = "Subtitles stream", body = String),
        (status = 404, description = "Subtitles are not found", body = AppError),
    ),
    tag = "Subtitles",
)]
pub async fn get_subtitles(
    Path(id): Path<i64>,
    State(db): State<Db>,
) -> Result<impl IntoResponse, AppError> {
    let (video_id, external_path) = sqlx::query!(
        "SELECT video_id, external_path FROM subtitles WHERE id = ?",
        id
    )
    .fetch_one(&db.pool)
    .await
    .map(|r| (r.video_id, r.external_path.map(PathBuf::from)))?;

    match external_path {
        Some(p) => Ok(FileStream::<ReaderStream<tokio::fs::File>>::from_path(p)
            .await?
            .into_response()),
        None => Ok(assets::SubtitleAsset::new(video_id, id)
            .into_response(headers::ContentType::text(), None)
            .await?
            .into_response()),
    }
}
