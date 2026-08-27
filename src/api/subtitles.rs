use std::path::PathBuf;

use axum::{
    extract::{Multipart, State},
    response::IntoResponse,
};
use axum_extra::{headers, response::FileStream};
use tokio_stream::StreamExt;
use tokio_util::io::ReaderStream;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::{
    AppError,
    api::{Json, NumberQuery, Path, Query},
    app_state::AppState,
    db::{self, Db, DbActions},
    library::assets::{self, FileAsset},
};

/// Pull subtitle from video file using its track number
#[utoipa::path(
    get,
    path = "/video/{id}/pull_subtitle",
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
async fn pull_video_subtitle(
    Path(video_id): Path<i64>,
    Query(number): Query<NumberQuery>,
    State(state): State<AppState>,
) -> crate::Result<String> {
    state
        .pull_subtitle_from_video(video_id, number.number)
        .await
}

/// Multipart subtitles api data type
// Not used in actuall implementation
#[allow(dead_code)]
#[derive(Debug, utoipa::ToSchema)]
pub struct MultipartSubtitles {
    pub language: Option<String>,
    #[schema(format = Binary, value_type = String, content_media_type = "application/octet-stream")]
    pub subtitles: bytes::Bytes,
}

/// Upload subtitles on the server
#[utoipa::path(
    post,
    path = "/video/{id}/upload_subtitles",
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
async fn upload_subtitles(
    Path(video_id): Path<i64>,
    State(db): State<Db>,
    mut multipart: Multipart,
) -> crate::Result<()> {
    let mut language = None;
    let mut file_stem = String::new();
    let id = db
        .insert_subtitles(&db::DbSubtitles {
            id: None,
            language: None,
            file_stem: String::new(),
            external_path: None,
            video_id,
        })
        .await?;

    let subtitles_asset = assets::SubtitleAsset::new(video_id, id);

    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name() {
            Some("language") => {
                language = field.text().await.ok();
            }
            Some("subtitles") => {
                file_stem = field.file_name().map(Into::into).unwrap_or_default();
                use std::io::Error;
                let mut stream = field.map(|data| data.map_err(Error::other));
                let output_path = subtitles_asset.path();
                if let Some(parent) = output_path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                crate::ffmpeg::convert_and_save_srt(&output_path, &mut stream).await?;
            }
            _ => {}
        }
    }
    if let Err(e) = db
        .update_subtitles(db::DbSubtitles {
            id: Some(id),
            language,
            file_stem,
            external_path: None,
            video_id,
        })
        .await
    {
        tracing::error!("Failed to commit updated subtitles: {e}");
        if let Err(e) = subtitles_asset.delete_file().await {
            tracing::error!("Failed to clean up subtitles file: {e}");
        };
        return Err(e.into());
    };

    Ok(())
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
    path = "/video/{id}/reference_subtitles",
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
async fn reference_external_subtitles(
    Path(video_id): Path<i64>,
    State(db): State<Db>,
    Json(reference): Json<SubtitlesReferencePayload>,
) -> crate::Result<()> {
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
    path = "/subtitles/{id}",
    params(
        ("id", description = "subtitles id"),
    ),
    responses(
        (status = 200, description = "Subtitles are successfully deleted"),
        (status = 404, description = "Subtitles are not found", body = AppError),
    ),
    tag = "Subtitles",
)]
async fn delete_subtitles(Path(id): Path<i64>, State(db): State<Db>) -> crate::Result<()> {
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
    path = "/subtitles/{id}",
    params(
        ("id", description = "subtitles id"),
    ),
    responses(
        (status = 200, description = "Subtitles stream", body = String),
        (status = 404, description = "Subtitles are not found", body = AppError),
    ),
    tag = "Subtitles",
)]
async fn get_subtitles(
    Path(id): Path<i64>,
    State(db): State<Db>,
) -> crate::Result<impl IntoResponse> {
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

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(delete_subtitles, get_subtitles))
        .routes(routes!(pull_video_subtitle))
        .routes(routes!(reference_external_subtitles))
        .routes(routes!(upload_subtitles))
}
