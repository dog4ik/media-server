use std::convert::Infallible;
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::Context;
use axum::extract::{Multipart, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{
    Json,
    response::{
        Sse,
        sse::{Event, KeepAlive},
    },
};
use axum_extra::headers::Range;
use axum_extra::{TypedHeader, headers};
use base64::Engine;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::sync::oneshot;
use tokio_stream::{Stream, StreamExt};
use uuid::Uuid;

use super::{ContentTypeQuery, OptionalContentTypeQuery, ProviderQuery, StringIdQuery};
use super::{CursorQuery, IdQuery, NumberQuery, SearchQuery, TakeParam, VariantQuery};
use crate::app_state::AppError;
use crate::config::{
    self, APP_RESOURCES, Capabilities, ConfigurationApplyResult, SerializedSetting,
};
use crate::db::{self, DbActions};
use crate::db::{DbExternalId, DbHistory};
use crate::ffmpeg::{FFprobeAudioStream, FFprobeSubtitleStream, FFprobeVideoStream};
use crate::ffmpeg::{PreviewsJob, TranscodeJob};
use crate::ffmpeg_abi::{self, Audio, Subtitle, Track};
use crate::file_browser::{BrowseDirectory, BrowseFile, BrowseRootDirs, FileKey};
use crate::intro_detection::IntroJob;
use crate::library::TranscodePayload;
use crate::library::assets::{
    self, BackdropAsset, BackdropContentType, FileAsset, PosterAsset, PosterContentType,
    PreviewAsset, VariantAsset,
};
use crate::library::assets::{AssetDir, PreviewsDirAsset};
use crate::library::{
    AudioCodec, ContentIdentifier, Resolution, Source, SubtitlesCodec, VideoCodec,
};
use crate::metadata::{
    ContentType, EpisodeMetadata, MovieMetadata, SeasonMetadata, ShowMetadata,
    metadata_stack::MetadataProvidersStack,
};
use crate::metadata::{ExternalIdMetadata, MetadataProvider, MetadataSearchResult};
use crate::progress::{LibraryScanTask, Task, TaskError, TaskResource};
use crate::server::{OptionalTorrentIndexQuery, Path, Query};
use crate::torrent_index::{Torrent, TorrentIndexIdentifier};
use crate::{app_state::AppState, db::Db, progress::ProgressChannel};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetailedSubtitlesAsset {
    id: i64,
    language: Option<String>,
    #[schema(value_type = String)]
    path: PathBuf,
    is_external: bool,
    is_available: bool,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct DetailedVideo {
    pub id: i64,
    #[schema(value_type = String)]
    pub path: PathBuf,
    pub previews_count: usize,
    pub size: u64,
    #[schema(value_type = super::SerdeDuration)]
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
    pub subtitle_tracks: Vec<DetailedSubtitleTrack>,
    pub variants: Vec<DetailedVariant>,
    pub scan_date: String,
    pub history: Option<DbHistory>,
    pub intro: Option<Intro>,
    pub chapters: Vec<DetailedChapter>,
    pub subtitles: Vec<DetailedSubtitlesAsset>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedVariant {
    pub id: String,
    #[schema(value_type = String)]
    pub path: PathBuf,
    pub size: u64,
    #[schema(value_type = super::SerdeDuration)]
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedAudioTrack {
    pub is_default: bool,
    pub is_dub: bool,
    pub is_visual_impaired: bool,
    pub is_hearing_impaired: bool,
    pub sample_rate: String,
    pub channels: u16,
    pub profile_idc: i32,
    pub codec: AudioCodec,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedSubtitleTrack {
    pub is_default: bool,
    pub is_visual_impaired: bool,
    pub is_hearing_impaired: bool,
    pub is_text_format: bool,
    pub language: Option<String>,
    pub codec: SubtitlesCodec,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedVideoTrack {
    pub is_default: bool,
    pub resolution: Resolution,
    pub level: i32,
    pub profile_idc: i32,
    pub bitrate: usize,
    pub framerate: f64,
    pub codec: VideoCodec,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Intro {
    start_sec: i64,
    end_sec: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedChapter {
    pub start: std::time::Duration,
    pub end: std::time::Duration,
    pub title: Option<String>,
}

impl DetailedVideo {
    pub async fn new(db: Db, source: Source) -> anyhow::Result<Self> {
        let id = source.id;

        let db_video = sqlx::query!(
            r#"SELECT videos.scan_date, videos.path, history.time,
        history.id, history.update_time, history.is_finished,
        episode_intro.start_sec, episode_intro.end_sec
        FROM videos
        LEFT JOIN history ON history.video_id = videos.id
        LEFT JOIN episode_intro ON episode_intro.video_id = videos.id
        WHERE videos.id = ?;"#,
            id
        )
        .fetch_one(&db.pool)
        .await?;

        let db_subtitles = sqlx::query!(
            "select id, language, external_path from subtitles where video_id = ?",
            id,
        )
        .fetch_all(&db.pool)
        .await?;

        let mut subtitles = Vec::with_capacity(db_subtitles.len());

        for record in db_subtitles {
            if let Some(external_path) = record.external_path {
                let is_available = tokio::fs::try_exists(&external_path).await.unwrap_or(false);
                subtitles.push(DetailedSubtitlesAsset {
                    id: record.id,
                    language: record.language,
                    path: PathBuf::from(external_path),
                    is_external: true,
                    is_available,
                });
            } else {
                let asset = assets::SubtitleAsset::new(id, record.id);
                let is_available = tokio::fs::try_exists(asset.path()).await.unwrap_or(false);
                subtitles.push(DetailedSubtitlesAsset {
                    id: record.id,
                    language: record.language,
                    path: asset.path(),
                    is_external: false,
                    is_available,
                });
            }
        }

        let video_metadata = source.video.metadata().await?;

        let mut detailed_variants = Vec::with_capacity(source.variants.len());
        for variant in &source.variants {
            match DetailedVariant::from_video(variant).await {
                Ok(variant) => detailed_variants.push(variant),
                Err(err) => {
                    tracing::warn!(
                        "Failed to construct variant {}: {err}",
                        variant.path().display()
                    );
                    continue;
                }
            };
        }

        let date = db_video.scan_date.expect("scan date always defined");

        let history = db_video
            .time
            .zip(db_video.is_finished)
            .zip(db_video.update_time)
            .map(|((time, is_finished), update_time)| DbHistory {
                id: Some(db_video.id),
                time,
                is_finished,
                update_time,
                video_id: db_video.id,
            });

        let intro = db_video
            .start_sec
            .zip(db_video.end_sec)
            .map(|(start_sec, end_sec)| Intro { start_sec, end_sec });

        let previews_count = PreviewsDirAsset::new(id).previews_count();

        Ok(DetailedVideo {
            id,
            path: source.video.path().to_path_buf(),
            previews_count,
            size: source.video.file_size().await?,
            duration: video_metadata.duration(),
            variants: detailed_variants,
            scan_date: date.to_string(),
            video_tracks: video_metadata
                .video_streams()
                .into_iter()
                .map(Into::into)
                .collect(),
            audio_tracks: video_metadata
                .audio_streams()
                .into_iter()
                .map(Into::into)
                .collect(),
            subtitle_tracks: video_metadata
                .subtitle_streams()
                .into_iter()
                .map(Into::into)
                .collect(),
            chapters: video_metadata.chapters().iter().map(Into::into).collect(),
            history,
            intro,
            subtitles,
        })
    }
}

impl DetailedVideoTrack {
    pub fn from_video_stream(stream: FFprobeVideoStream<'_>, bitrate: usize) -> Self {
        DetailedVideoTrack {
            is_default: stream.is_default(),
            resolution: stream.resolution(),
            level: stream.level,
            profile_idc: stream.profile.parse().unwrap(),
            bitrate,
            framerate: stream.framerate(),
            codec: stream.codec(),
        }
    }
}

impl From<&Track<ffmpeg_abi::Video>> for DetailedVideoTrack {
    fn from(val: &Track<ffmpeg_abi::Video>) -> Self {
        Self {
            is_default: val.is_default(),
            resolution: val.stream.resolution(),
            level: val.stream.level,
            profile_idc: val.stream.profile,
            bitrate: val.stream.bit_rate,
            framerate: val.stream.avg_frame_rate as f64,
            codec: val.stream.codec.clone(),
        }
    }
}

impl From<FFprobeAudioStream<'_>> for DetailedAudioTrack {
    fn from(val: FFprobeAudioStream<'_>) -> Self {
        DetailedAudioTrack {
            is_default: val.disposition.default == 1,
            is_hearing_impaired: false,
            is_visual_impaired: false,
            is_dub: false,
            sample_rate: val.sample_rate.to_string(),
            channels: val.channels as u16,
            profile_idc: val.profile.unwrap().parse().unwrap(),
            codec: val.codec(),
            language: None,
        }
    }
}

impl From<&Track<Audio>> for DetailedAudioTrack {
    fn from(val: &Track<Audio>) -> Self {
        DetailedAudioTrack {
            is_default: val.is_default(),
            is_hearing_impaired: val.stream.is_hearing_impaired,
            is_visual_impaired: val.stream.is_visual_impaired,
            is_dub: val.stream.is_dub,
            sample_rate: val.stream.sample_rate.to_string(),
            channels: val.stream.channels,
            profile_idc: val.stream.profile_idc,
            codec: val.stream.codec.clone(),
            language: val.stream.language.clone(),
        }
    }
}

impl From<FFprobeSubtitleStream<'_>> for DetailedSubtitleTrack {
    fn from(val: FFprobeSubtitleStream<'_>) -> Self {
        DetailedSubtitleTrack {
            is_default: val.is_default(),
            is_hearing_impaired: false,
            is_visual_impaired: false,
            is_text_format: val.codec().supports_text(),
            language: val.language.map(|x| x.to_string()),
            codec: val.codec(),
        }
    }
}

impl From<&Track<Subtitle>> for DetailedSubtitleTrack {
    fn from(val: &Track<Subtitle>) -> Self {
        DetailedSubtitleTrack {
            is_default: val.is_default(),
            is_hearing_impaired: val.stream.is_hearing_impaired,
            is_visual_impaired: val.stream.is_visual_impaired,
            is_text_format: val.stream.codec.supports_text(),
            language: val.stream.language.clone(),
            codec: val.stream.codec.clone(),
        }
    }
}

impl From<&ffmpeg_abi::Chapter> for DetailedChapter {
    fn from(val: &ffmpeg_abi::Chapter) -> Self {
        DetailedChapter {
            start: val.start,
            end: val.end,
            title: val.title.clone(),
        }
    }
}

impl DetailedVariant {
    pub async fn from_video(video: &crate::library::Video) -> anyhow::Result<Self> {
        let id = video
            .path()
            .file_stem()
            .expect("file to have stem like {size}.{hash}")
            .to_string_lossy()
            .to_string();
        let metadata = video.metadata().await?;
        Ok(Self {
            id,
            size: video.file_size().await?,
            duration: metadata.duration(),
            video_tracks: metadata
                .video_streams()
                .into_iter()
                .map(Into::into)
                .collect(),
            audio_tracks: metadata
                .audio_streams()
                .into_iter()
                .map(Into::into)
                .collect(),
            path: video.path().to_path_buf(),
        })
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "content_type", rename_all = "lowercase")]
pub enum VideoContentMetadata {
    Episode {
        show: ShowMetadata,
        episode: EpisodeMetadata,
    },
    Movie {
        movie: MovieMetadata,
    },
}

/// Get metadata related to the video
#[utoipa::path(
    get,
    path = "/api/video/{id}/metadata",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200, description = "Metadata related to the video", body = VideoContentMetadata),
    ),
    tag = "Videos",
)]
pub async fn video_content_metadata(
    Path(video_id): Path<i64>,
    State(app_state): State<AppState>,
) -> Result<Json<VideoContentMetadata>, AppError> {
    let video = {
        let library = app_state.library.lock().unwrap();
        library
            .videos
            .get(&video_id)
            .ok_or(AppError::not_found("Video is not found"))?
            .clone()
    };
    let db = app_state.db;
    let video_metadata = video.source.video.metadata().await?;
    let duration = video_metadata.duration();
    let metadata = match video.identifier {
        ContentIdentifier::Show(_) => {
            let query = sqlx::query!(
                r#"SELECT episodes.id AS episode_id, seasons.show_id AS show_id FROM videos
            JOIN episodes ON episodes.id = videos.episode_id
            JOIN seasons ON seasons.id = episodes.season_id WHERE videos.id = ?;"#,
                video_id
            )
            .fetch_one(&db.pool)
            .await?;
            let episode_id = query.episode_id;
            let show_id = query.show_id;
            let episode_query = db.get_episode_by_id(episode_id);
            let show_query = db.get_show(show_id);
            let (episode, show) = tokio::join!(episode_query, show_query);
            let (mut episode, show) = (episode?, show?);
            episode.runtime = Some(duration);
            VideoContentMetadata::Episode { show, episode }
        }
        ContentIdentifier::Movie(_) => {
            let query = sqlx::query!(
                r#"SELECT movies.id FROM videos JOIN movies ON movies.id = videos.movie_id WHERE videos.id = ?;
                "#,
                video_id
            )
            .fetch_one(&db.pool)
            .await?;
            let mut movie = db.get_movie(query.id).await?;
            movie.runtime = Some(duration);
            VideoContentMetadata::Movie { movie }
        }
    };
    Ok(Json(metadata))
}

/// Get preview by video id
#[utoipa::path(
    get,
    path = "/api/video/{id}/previews/{number}",
    params(
        ("id", description = "video id"),
        ("number", description = "preview number"),
    ),
    responses(
        (status = 200, description = "Binary image", body = [u8]),
        (status = 304),
        (status = 404, description = "Preiew is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn previews(
    Path((video_id, number)): Path<(i64, usize)>,
    is_modified_since: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let preview_asset = PreviewAsset::new(video_id, number);
    let response = preview_asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_since)
        .await?;
    Ok(response)
}

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
    #[schema(content_media_type = "application/octet-stream", value_type = Vec<u8>)]
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
            let db_subtitles = db::DbSubtitles {
                id: None,
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
        return Err(AppError::bad_request("only .srt files can be referenced"));
    }
    let db_subtitles = db::DbSubtitles {
        id: None,
        language: reference.language,
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
    let mut tx = db.begin().await?;
    let removed_subs = sqlx::query!(
        "DELETE FROM subtitles WHERE id = ? RETURNING video_id, external_path",
        id
    )
    .fetch_one(&mut *tx)
    .await?;

    // if subtitles are not referenced delete the asset
    if removed_subs.external_path.is_none() {
        let subtitles_asset = assets::SubtitleAsset::new(removed_subs.video_id, id);
        subtitles_asset.delete_file().await?;
    }

    tx.commit().await?;
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
        (status = 200, description = "Subtitles stream", body = Vec<u8>),
        (status = 404, description = "Subtitles are not found", body = AppError),
    ),
    tag = "Subtitles",
)]
pub async fn get_subtitles(
    Path(id): Path<i64>,
    State(db): State<Db>,
) -> Result<impl IntoResponse, AppError> {
    let video_id = sqlx::query!("SELECT video_id FROM subtitles WHERE id = ?", id)
        .fetch_one(&db.pool)
        .await?
        .video_id;

    let response = assets::SubtitleAsset::new(video_id, id)
        .into_response(headers::ContentType::text(), None)
        .await?;
    Ok(response)
}

/// Video stream
#[utoipa::path(
    get,
    path = "/api/video/{id}/watch",
    params(
        ("id", description = "video id"),
        VariantQuery,
    ),
    responses(
        (status = 206, description = "Video progressive download stream", body = [u8], content_type = "video/*"),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn watch(
    Path(video_id): Path<i64>,
    variant: Query<VariantQuery>,
    State(state): State<AppState>,
    range: Option<TypedHeader<Range>>,
) -> Result<impl IntoResponse, AppError> {
    if let Query(VariantQuery {
        variant: Some(variant),
    }) = variant
    {
        let variant_asset = VariantAsset::new(video_id, variant);
        let video = variant_asset.video().await?;
        Ok(video.serve(range).await)
    } else {
        let AppState { library, .. } = state;
        let video = {
            let library = library.lock().unwrap();
            library
                .get_source(video_id)
                .map(|x| x.video.clone())
                .ok_or(AppError::not_found("Video not found"))?
        };
        Ok(video.serve(range).await)
    }
}

/// Watch episode video
#[utoipa::path(
    get,
    path = "/api/local_episode/{episode_id}/watch",
    params(
        ("episode_id", description = "episode id"),
        VariantQuery,
    ),
    responses(
        (status = 206, description = "Video progressive download stream", content_type = "video/*"),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn watch_episode(
    Path(episode_id): Path<i64>,
    variant: Query<VariantQuery>,
    State(state): State<AppState>,
    range: Option<TypedHeader<Range>>,
) -> Result<impl IntoResponse, AppError> {
    let video_id = sqlx::query!("SELECT id FROM videos WHERE episode_id = ?;", episode_id)
        .fetch_one(&state.db.pool)
        .await?
        .id;

    watch(Path(video_id), variant, State(state), range).await
}

/// Watch movie video
#[utoipa::path(
    get,
    path = "/api/local_movie/{movie_id}/watch",
    params(
        ("movie_id", description = "movie id"),
        VariantQuery,
    ),
    responses(
        (status = 206, description = "Movie video progressive download stream", content_type = "video/*"),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Movies",
)]
pub async fn watch_movie(
    Path(movie_id): Path<i64>,
    variant: Query<VariantQuery>,
    State(state): State<AppState>,
    range: Option<TypedHeader<Range>>,
) -> Result<impl IntoResponse, AppError> {
    let video_id = sqlx::query!("SELECT id FROM videos WHERE movie_id = ?;", movie_id)
        .fetch_one(&state.db.pool)
        .await?
        .id;
    watch(Path(video_id), variant, State(state), range).await
}

#[utoipa::path(
    get,
    path = "/api/local_shows",
    responses(
        (status = 200, description = "All local shows", body = Vec<ShowMetadata>),
    ),
    tag = "Shows",
)]
/// All local shows
pub async fn all_local_shows(State(db): State<Db>) -> Result<Json<Vec<ShowMetadata>>, AppError> {
    Ok(Json(db.pool.all_shows(None).await?))
}

#[utoipa::path(
    get,
    path = "/api/local_episode/{id}",
    params(
        ("id", description = "Local id"),
    ),
    responses(
        (status = 200, description = "Local episode", body = EpisodeMetadata),
        (status = 404, description = "Episode is not found", body = AppError),
    ),
    tag = "Shows",
)]
/// Local episode metadata by local episode id
pub async fn local_episode(
    Path(id): Path<i64>,
    State(db): State<Db>,
) -> Result<Json<EpisodeMetadata>, AppError> {
    Ok(Json(db.get_episode_by_id(id).await?))
}

#[utoipa::path(
    get,
    path = "/api/local_episode/by_video",
    params(
        IdQuery,
    ),
    responses(
        (status = 200, description = "Local episode", body = EpisodeMetadata),
    ),
    tag = "Shows",
)]
/// Get local episode metadata by video's id
pub async fn local_episode_by_video_id(
    Query(IdQuery { id }): Query<IdQuery>,
    State(db): State<Db>,
) -> Result<Json<EpisodeMetadata>, AppError> {
    let episode_id = sqlx::query!(
        r#"SELECT videos.episode_id as "episode_id!: i64"
    FROM videos WHERE id = ? AND videos.episode_id NOT NULL"#,
        id
    )
    .fetch_one(&db.pool)
    .await?;
    Ok(Json(db.get_episode_by_id(episode_id.episode_id).await?))
}

#[utoipa::path(
    get,
    path = "/api/local_movie/by_video",
    params(
        IdQuery,
    ),
    responses(
        (status = 200, description = "Local movie", body = MovieMetadata),
    ),
    tag = "Movies",
)]
/// Get local movie metadata by video's id
pub async fn local_movie_by_video_id(
    Query(IdQuery { id }): Query<IdQuery>,
    State(db): State<Db>,
) -> Result<Json<MovieMetadata>, AppError> {
    let movie_id = sqlx::query!(r#"SELECT movie_id as "movie_id!: i64" FROM videos WHERE id = ? AND videos.movie_id NOT NULL"#, id)
        .fetch_one(&db.pool)
        .await?;
    Ok(Json(db.get_movie(movie_id.movie_id).await?))
}

#[utoipa::path(
    get,
    path = "/api/local_movies",
    responses(
        (status = 200, description = "All local movies", body = Vec<MovieMetadata>),
    ),
    tag = "Movies",
)]
/// All local movies
pub async fn all_local_movies(State(db): State<Db>) -> Result<Json<Vec<MovieMetadata>>, AppError> {
    Ok(Json(db.all_movies(None).await?))
}

/// Map external to local id
#[utoipa::path(
    get,
    path = "/api/external_to_local/{id}",
    params(
        ("id", description = "External id"),
        ProviderQuery,
    ),
    responses(
        (status = 200, body = DbExternalId),
        (status = 404, description = "Local id is not found", body = AppError),
    ),
    tag = "Metadata",
)]
pub async fn external_to_local_id(
    Path(id): Path<String>,
    Query(provider): Query<ProviderQuery>,
    State(db): State<Db>,
) -> Result<Json<DbExternalId>, AppError> {
    let provider = provider.provider.to_string();
    let local_id = sqlx::query_as!(
        DbExternalId,
        "SELECT * FROM external_ids WHERE metadata_id = ? AND metadata_provider = ?",
        id,
        provider
    )
    .fetch_one(&db.pool)
    .await?;

    Ok(Json(local_id))
}

/// List external ids for desired content
#[utoipa::path(
    get,
    path = "/api/external_ids/{id}",
    params(
        ("id", description = "External id"),
        ProviderQuery,
        ContentTypeQuery,
    ),
    responses(
        (status = 200, description = "External ids", body = Vec<ExternalIdMetadata>),
    ),
    tag = "Metadata",
)]
pub async fn external_ids(
    State(providers): State<&'static MetadataProvidersStack>,
    Path(id): Path<String>,
    Query(ProviderQuery { provider }): Query<ProviderQuery>,
    Query(ContentTypeQuery { content_type }): Query<ContentTypeQuery>,
) -> Result<Json<Vec<ExternalIdMetadata>>, AppError> {
    let res = providers
        .get_external_ids(&id, content_type, provider)
        .await?;
    Ok(Json(res))
}

/// Videos for local content
#[utoipa::path(
    get,
    path = "/api/video/by_content",
    params(
        ContentTypeQuery,
        IdQuery,
    ),
    responses(
        (status = 200, description = "Videos for the content", body = Vec<DetailedVideo>),
        (status = 404, description = "Content is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn contents_video(
    Query(IdQuery { id }): Query<IdQuery>,
    Query(content_type): Query<ContentTypeQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<DetailedVideo>>, AppError> {
    #[derive(FromRow)]
    struct VideoId {
        id: i64,
    }

    let video_ids = match content_type.content_type {
        crate::metadata::ContentType::Movie => {
            sqlx::query_as!(VideoId, "SELECT id FROM videos WHERE movie_id = ?", id)
                .fetch_all(&state.db.pool)
                .await
        }
        crate::metadata::ContentType::Show => {
            sqlx::query_as!(VideoId, "SELECT id FROM videos WHERE episode_id = ?", id)
                .fetch_all(&state.db.pool)
                .await
        }
    }?;

    let mut out = Vec::with_capacity(video_ids.len());

    for VideoId { id } in video_ids {
        match get_video_by_id(Path(id), State(state.clone())).await {
            Ok(v) => out.push(v.0),
            Err(e) => {
                tracing::warn!("Failed to construct detailed video: {e}");
            }
        };
    }
    Ok(Json(out))
}

/// Get all videos that have transcoded variants
#[utoipa::path(
    get,
    path = "/api/variants",
    responses(
        (status = 200, body = Vec<DetailedVideo>),
    ),
    tag = "Videos",
)]
pub async fn get_all_variants(State(state): State<AppState>) -> Json<Vec<DetailedVideo>> {
    let videos: Vec<Source> = {
        let library = state.library.lock().unwrap();
        library
            .videos
            .values()
            .map(|v| &v.source)
            .filter(|s| !s.variants.is_empty())
            .cloned()
            .collect()
    };
    let mut summary = Vec::with_capacity(videos.len());
    for video in videos.into_iter() {
        match DetailedVideo::new(state.db.clone(), video).await {
            Ok(v) => summary.push(v),
            Err(e) => {
                tracing::error!("Failed to construct detailed video: {e}");
            }
        }
    }
    Json(summary)
}

/// Get video by id
#[utoipa::path(
    get,
    path = "/api/video/{id}",
    params(
        ("id", description = "Video id")
    ),
    responses(
        (status = 200, description = "Requested video", body = DetailedVideo),
    ),
    tag = "Videos",
)]
pub async fn get_video_by_id(
    Path(id): Path<i64>,
    State(state): State<AppState>,
) -> Result<Json<DetailedVideo>, AppError> {
    let AppState { library, db, .. } = state;
    let source = {
        let library = library.lock().unwrap();
        library
            .get_source(id)
            .cloned()
            .ok_or(AppError::not_found("Video is not found"))?
    };
    let detailed_video = DetailedVideo::new(db.clone(), source).await?;
    Ok(Json(detailed_video))
}

/// Get show by id and provider
#[utoipa::path(
    get,
    path = "/api/show/{id}",
    params(
        ("id", description = "Show id"),
        ProviderQuery,
    ),
    responses(
        (status = 200, description = "Requested show", body = ShowMetadata),
        (status = 404, body = AppError)
    ),
    tag = "Shows",
)]
pub async fn get_show(
    State(providers): State<&'static MetadataProvidersStack>,
    Query(ProviderQuery { provider }): Query<ProviderQuery>,
    Path(id): Path<String>,
) -> Result<Json<ShowMetadata>, AppError> {
    let res = providers.get_show(&id, provider).await?;
    Ok(Json(res))
}

/// Get movie by id and provider
#[utoipa::path(
    get,
    path = "/api/movie/{id}",
    params(
        ("id", description = "Movie id"),
        ProviderQuery,
    ),
    responses(
        (status = 200, description = "Requested movie", body = MovieMetadata),
        (status = 404, body = AppError)
    ),
    tag = "Movies",
)]
pub async fn get_movie(
    State(providers): State<&'static MetadataProvidersStack>,
    Query(ProviderQuery { provider }): Query<ProviderQuery>,
    Path(id): Path<String>,
) -> Result<Json<MovieMetadata>, AppError> {
    let res = providers.get_movie(&id, provider).await?;
    Ok(Json(res))
}

/// Get show poster
#[utoipa::path(
    get,
    path = "/api/show/{id}/poster",
    params(
        ("id", description = "Show id"),
    ),
    responses(
        (status = 200, content_type = "image/*"),
        (status = 304),
        (status = 404, body = AppError)
    ),
    tag = "Shows",
)]
pub async fn show_poster(
    Path(id): Path<i64>,
    is_modified_since: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = PosterAsset::new(id, PosterContentType::Show);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_since)
        .await?;
    Ok(response)
}

/// Get season poster
#[utoipa::path(
    get,
    path = "/api/season/{id}/poster",
    params(
        ("id", description = "Season id"),
    ),
    responses(
        (status = 200, content_type = "image/*"),
        (status = 304),
        (status = 404, body = AppError)
    ),
    tag = "Shows",
)]
pub async fn season_poster(
    Path(id): Path<i64>,
    is_modified_since: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = PosterAsset::new(id, PosterContentType::Season);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_since)
        .await?;
    Ok(response)
}

/// Get show backdrop image
#[utoipa::path(
    get,
    path = "/api/show/{id}/backdrop",
    params(
        ("id", description = "Show id"),
    ),
    responses(
        (status = 200, content_type = "image/*"),
        (status = 304),
        (status = 404, description = "Image not found", body = AppError)
    ),
    tag = "Shows",
)]
pub async fn show_backdrop(
    Path(id): Path<i64>,
    is_modified_since: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = BackdropAsset::new(id, BackdropContentType::Show);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_since)
        .await?;
    Ok(response)
}

/// Get movie poster
#[utoipa::path(
    get,
    path = "/api/movie/{id}/poster",
    params(
        ("id", description = "Movie id"),
    ),
    responses(
        (status = 200, content_type = "image/*"),
        (status = 304),
        (status = 404, body = AppError)
    ),
    tag = "Movies",
)]
pub async fn movie_poster(
    Path(id): Path<i64>,
    is_modified_since: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = PosterAsset::new(id, PosterContentType::Movie);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_since)
        .await?;
    Ok(response)
}

/// Get movie backdrop image
#[utoipa::path(
    get,
    path = "/api/movie/{id}/backdrop",
    params(
        ("id", description = "Movie id"),
    ),
    responses(
        (status = 200, content_type = "image/*"),
        (status = 304),
        (status = 404, body = AppError)
    ),
    tag = "Movies",
)]
pub async fn movie_backdrop(
    Path(id): Path<i64>,
    is_modified_since: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = BackdropAsset::new(id, BackdropContentType::Movie);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_since)
        .await?;
    Ok(response)
}

/// Get episode poster
#[utoipa::path(
    get,
    path = "/api/episode/{id}/poster",
    params(
        ("id", description = "Episode id"),
    ),
    responses(
        (status = 200, content_type = "image/*"),
        (status = 304),
        (status = 404, body = AppError)
    ),
    tag = "Shows",
)]
pub async fn episode_poster(
    Path(id): Path<i64>,
    is_modified_since: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = PosterAsset::new(id, PosterContentType::Episode);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_since)
        .await?;
    Ok(response)
}

/// Get season metadata
#[utoipa::path(
    get,
    path = "/api/show/{id}/{season}",
    params(
        ("id", description = "Show id"),
        ("season", description = "Season number"),
        ProviderQuery,
    ),
    responses(
        (status = 200, description = "Desired season metadata", body = SeasonMetadata),
        (status = 404, body = AppError)
    ),
    tag = "Shows",
)]
pub async fn get_season(
    State(providers): State<&'static MetadataProvidersStack>,
    Query(ProviderQuery { provider }): Query<ProviderQuery>,
    Path((show_id, season)): Path<(String, usize)>,
) -> Result<Json<SeasonMetadata>, AppError> {
    let res = providers.get_season(&show_id, season, provider).await?;
    Ok(Json(res))
}

/// Get episode metadata
#[utoipa::path(
    get,
    path = "/api/show/{id}/{season}/{episode}",
    params(
        ("id", description = "Show id"),
        ("season", description = "Season number"),
        ("episode", description = "Episode number"),
        ProviderQuery,
    ),
    responses(
        (status = 200, description = "Desired episode metadata", body = EpisodeMetadata),
        (status = 404, body = AppError)
    ),
    tag = "Shows",
)]
pub async fn get_episode(
    State(providers): State<&'static MetadataProvidersStack>,
    Query(ProviderQuery { provider }): Query<ProviderQuery>,
    Path((show_id, season, episode)): Path<(String, usize, usize)>,
) -> Result<Json<EpisodeMetadata>, AppError> {
    let res = providers
        .get_episode(&show_id, season, episode, provider)
        .await?;
    Ok(Json(res))
}

/// Search for torrent
#[utoipa::path(
    get,
    path = "/api/torrent/search",
    params(
        SearchQuery,
        OptionalContentTypeQuery,
        OptionalTorrentIndexQuery,
    ),
    responses(
        (status = 200, description = "Torrent search results", body = Vec<Torrent>),
    ),
    tag = "Torrent",
)]
pub async fn search_torrent(
    Query(SearchQuery { search }): Query<SearchQuery>,
    Query(content_type): Query<OptionalContentTypeQuery>,
    Query(OptionalTorrentIndexQuery { provider }): Query<OptionalTorrentIndexQuery>,
    State(providers): State<&'static MetadataProvidersStack>,
) -> Result<Json<Vec<Torrent>>, AppError> {
    if search.is_empty() {
        return Ok(Json(Vec::new()));
    }
    Ok(Json(match provider {
        Some(p) => {
            let lang: config::MetadataLanguage = config::CONFIG.get_value();
            let fetch_params = crate::metadata::FetchParams { lang: lang.0 };
            let provider = providers
                .torrent_index(p)
                .ok_or(AppError::not_found("Provider is not found"))?;
            match content_type.content_type {
                Some(ContentType::Show) => {
                    provider.search_show_torrent(&search, &fetch_params).await?
                }
                Some(ContentType::Movie) => {
                    provider
                        .search_movie_torrent(&search, &fetch_params)
                        .await?
                }
                None => provider.search_any_torrent(&search, &fetch_params).await?,
            }
        }
        None => {
            providers
                .get_torrents(&search, content_type.content_type)
                .await
        }
    }))
}

/// Get trending shows
#[utoipa::path(
    get,
    path = "/api/search/trending_shows",
    responses(
        (status = 200, description = "List of trending movies", body = Vec<ShowMetadata>),
    ),
    tag = "Search",
)]
pub async fn get_trending_shows(
    State(providers): State<&'static MetadataProvidersStack>,
) -> Result<Json<Vec<ShowMetadata>>, AppError> {
    let language: config::MetadataLanguage = config::CONFIG.get_value();
    let tmdb_api = providers
        .tmdb
        .ok_or(AppError::bad_request("tmdb provider is not available"))?;
    let res = tmdb_api.trending_shows(language.0).await?;
    Ok(Json(res.results.into_iter().map(Into::into).collect()))
}

/// Get trending movies
#[utoipa::path(
    get,
    path = "/api/search/trending_movies",
    responses(
        (status = 200, description = "List of trending shows", body = Vec<MovieMetadata>),
    ),
    tag = "Search",
)]
pub async fn get_trending_movies(
    State(providers): State<&'static MetadataProvidersStack>,
) -> Result<Json<Vec<MovieMetadata>>, AppError> {
    let language: config::MetadataLanguage = config::CONFIG.get_value();
    let tmdb_api = providers
        .tmdb
        .ok_or(AppError::bad_request("tmdb provider is not available"))?;
    let res = tmdb_api.trending_movies(language.0).await?;
    Ok(Json(res.results.into_iter().map(Into::into).collect()))
}

/// Search for content. Allows to search for all types of content at once
#[utoipa::path(
    get,
    path = "/api/search/content",
    params(
        SearchQuery,
    ),
    responses(
        (status = 200, description = "Content search results", body = Vec<MetadataSearchResult>),
    ),
    tag = "Search",
)]
pub async fn search_content(
    Query(query): Query<SearchQuery>,
    State(providers): State<&'static MetadataProvidersStack>,
) -> Result<Json<Vec<MetadataSearchResult>>, AppError> {
    if query.search.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let res = providers.multi_search(&query.search).await?;
    Ok(Json(res))
}

/// Get history for specific video
#[utoipa::path(
    get,
    path = "/api/history/{id}",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200, description = "History of desired video", body = Vec<DbHistory>),
        (status = 404),
    ),
    tag = "History",
)]
pub async fn video_history(
    Path(video_id): Path<i64>,
    State(db): State<Db>,
) -> Result<Json<DbHistory>, AppError> {
    let history = sqlx::query_as!(
        DbHistory,
        "SELECT * FROM history WHERE video_id = ?;",
        video_id
    )
    .fetch_one(&db.pool)
    .await?;
    Ok(Json(history))
}

/// Get all watch history of the default user. Limit defaults to 50 if not specified
#[utoipa::path(
    get,
    path = "/api/history",
    responses(
        (status = 200, description = "All history", body = CursoredResponse<DbHistory>),
    ),
    params(
        TakeParam,
        CursorQuery,
    ),
    tag = "History",
)]
pub async fn all_history(
    Query(TakeParam { take }): Query<TakeParam>,
    Query(CursorQuery { cursor }): Query<CursorQuery>,
    State(db): State<Db>,
) -> Result<Json<CursoredResponse<DbHistory>>, AppError> {
    let take = take.unwrap_or(50) as i64;
    let cursor: Option<i64> = cursor.map(|x| x.parse().unwrap());
    let history = match cursor {
        Some(cursor) => {
            sqlx::query_as!(
                DbHistory,
                "SELECT * FROM history WHERE update_time < datetime(?, 'unixepoch') ORDER BY update_time DESC LIMIT ?;",
                cursor,
                take,
            )
            .fetch_all(&db.pool)
            .await?
        }
        None => {
            sqlx::query_as!(
                DbHistory,
                "SELECT * FROM history ORDER BY update_time DESC LIMIT ?;",
                take,
            )
            .fetch_all(&db.pool)
            .await?
        }
    };
    let cursor = history.last().map(|x| {
        let date = x.update_time;
        date.unix_timestamp()
    });
    let response = CursoredResponse::new(history, cursor);
    Ok(Json(response))
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MovieHistory {
    pub movie: MovieMetadata,
    pub history: DbHistory,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ShowHistory {
    pub show_id: i64,
    pub episode: EpisodeMetadata,
    pub history: DbHistory,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HistoryEntry {
    Show { show: ShowHistory },
    Movie { movie: MovieHistory },
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ShowSuggestion {
    pub show_id: i64,
    pub episode: EpisodeMetadata,
    pub history: Option<DbHistory>,
}

/// Suggest to continue watching up to 3 movies based on history
#[utoipa::path(
    get,
    path = "/api/history/suggest/movies",
    responses(
        (status = 200, description = "Suggested movies", body = Vec<MovieHistory>),
    ),
    tag = "History",
)]
pub async fn suggest_movies(State(db): State<Db>) -> Result<Json<Vec<MovieHistory>>, AppError> {
    let history = sqlx::query!(
        r#"SELECT history.id AS history_id, history.time, history.is_finished, history.update_time,
        history.video_id AS video_id, movies.id AS movie_id FROM history
    JOIN videos ON videos.id = history.video_id
    JOIN movies ON movies.id = videos.movie_id WHERE history.is_finished = false
    ORDER BY history.update_time DESC LIMIT 3;"#
    )
    .fetch_all(&db.pool)
    .await?;

    let mut movie_suggestions = Vec::with_capacity(history.len());
    for entry in history {
        let Ok(movie_metadata) = db.get_movie(entry.movie_id).await else {
            tracing::error!("Failed to get movie connected to the history");
            continue;
        };
        movie_suggestions.push(MovieHistory {
            history: DbHistory {
                id: Some(entry.history_id),
                time: entry.time,
                is_finished: entry.is_finished,
                update_time: entry.update_time,
                video_id: entry.video_id,
            },
            movie: movie_metadata,
        });
    }
    Ok(Json(movie_suggestions))
}

/// Suggest to continue watching up to 3 shows based on history
#[utoipa::path(
    get,
    path = "/api/history/suggest/shows",
    responses(
        (status = 200, description = "Suggested shows", body = Vec<ShowSuggestion>),
    ),
    tag = "History",
)]
pub async fn suggest_shows(State(db): State<Db>) -> Result<Json<Vec<ShowSuggestion>>, AppError> {
    let history = sqlx::query!(
        r#"SELECT history.id AS history_id, history.time, history.is_finished, history.update_time,
        history.video_id AS video_id, episodes.number AS episode_number, seasons.show_id AS show_id,
        seasons.number AS season_number FROM history
    JOIN videos ON videos.id = history.video_id
    JOIN episodes ON episodes.id = videos.episode_id
    JOIN seasons ON seasons.id = episodes.season_id WHERE history.is_finished = false
    ORDER BY history.update_time DESC LIMIT 50;"#
    )
    .fetch_all(&db.pool)
    .await?;
    let mut show_suggestions: Vec<ShowSuggestion> = Vec::with_capacity(3);
    for entry in history {
        if show_suggestions
            .iter()
            .map(|x| x.show_id)
            .any(|id| id == entry.show_id)
        {
            continue;
        };
        let Ok(episode_metadata) = db
            .get_episode(
                entry.show_id,
                entry.season_number as usize,
                entry.episode_number as usize,
            )
            .await
        else {
            tracing::error!("Failed to get episode connected to the history");
            continue;
        };
        show_suggestions.push(ShowSuggestion {
            history: Some(DbHistory {
                id: Some(entry.history_id),
                time: entry.time,
                is_finished: entry.is_finished,
                update_time: entry.update_time,
                video_id: entry.video_id,
            }),
            show_id: entry.show_id,
            episode: episode_metadata,
        });

        if show_suggestions.len() == 3 {
            break;
        }
    }

    Ok(Json(show_suggestions))
}

/// Debug library files
pub async fn library_state(
    State(app_state): State<AppState>,
) -> Result<Json<serde_json::Value>, AppError> {
    let library = app_state.library.lock().unwrap();
    let map: serde_json::Map<String, serde_json::Value> = library
        .videos
        .iter()
        .map(|(id, v)| (id.to_string(), serde_json::to_value(&v.identifier).unwrap()))
        .collect();

    Ok(Json(map.into()))
}

/// Perform full library refresh
#[utoipa::path(
    post,
    path = "/api/scan",
    responses(
        (status = 202, description = "Scan is successfully started"),
        (status = 400, body = AppError, description = "Scan is already in progress"),
    ),
    tag = "Videos",
)]
pub async fn reconciliate_lib(State(app_state): State<AppState>) -> Result<StatusCode, AppError> {
    let tasks = app_state.tasks;
    let task_id = tasks.library_scan_tasks.start_task(LibraryScanTask, None)?;
    tokio::spawn(async move {
        match app_state.reconciliate_library().await {
            Ok(_) => {
                tasks.library_scan_tasks.finish_task(task_id);
            }
            Err(err) => {
                tracing::error!("Library reconcilliation task failed: {err}");
                tasks
                    .library_scan_tasks
                    .error_task(task_id, TaskError::Failure);
            }
        };
    });
    Ok(StatusCode::ACCEPTED)
}

/// Clear the database. For debug purposes only.
#[utoipa::path(
    delete,
    path = "/api/clear_db",
    responses(
        (status = 200, body = String),
    ),
    tag = "Configuration",
)]
pub async fn clear_db(State(app_state): State<AppState>) -> Result<String, StatusCode> {
    tracing::info!("Clearing database");
    app_state
        .db
        .pool
        .clear()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok("done".into())
}

#[derive(Debug, utoipa::ToSchema)]
pub struct CursoredResponse<T> {
    data: Vec<T>,
    cursor: Option<String>,
}

impl<T> CursoredResponse<T> {
    pub fn new(data: Vec<T>, cursor: Option<impl Display>) -> Self {
        let cursor = cursor.map(|x| x.to_string());
        Self { data, cursor }
    }
}

impl<T> Serialize for CursoredResponse<T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut out = serializer.serialize_struct("cursored_response", 2)?;
        out.serialize_field("data", &self.data)?;
        let encoded_cursor = self.cursor.as_ref().map(|cursor| {
            let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
            engine.encode(cursor)
        });
        out.serialize_field("cursor", &encoded_cursor)?;
        out.end()
    }
}

/// Remove video from library. WARN: It will actually delete video file
#[utoipa::path(
    delete,
    path = "/api/video/{id}",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn remove_video(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    state.remove_video(id).await
}

/// Delete episode from library. WARN: It will actually delete video files
#[utoipa::path(
    delete,
    path = "/api/local_episode/{id}",
    params(
        ("id", description = "Episode id"),
    ),
    responses(
        (status = 200),
        (status = 404, description = "Episode is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn delete_episode(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    state.delete_episode(id).await
}

/// Delete season from library. WARN: It will actually delete video files
#[utoipa::path(
    delete,
    path = "/api/local_season/{id}",
    params(
        ("id", description = "Season id"),
    ),
    responses(
        (status = 200, description = "Successfully deleted season"),
        (status = 404, description = "Season is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn delete_season(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    state.delete_season(id).await
}

/// Delete show from library. WARN: It will actually delete video files
#[utoipa::path(
    delete,
    path = "/api/local_show/{id}",
    params(
        ("id", description = "Show id"),
    ),
    responses(
        (status = 200, description = "Successfully deleted show"),
        (status = 404, description = "Show is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn delete_show(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    state.delete_show(id).await
}

/// Delete movie from library. WARN: It will actually delete video files
#[utoipa::path(
    delete,
    path = "/api/local_movie/{id}",
    params(
        ("id", description = "Movie id"),
    ),
    responses(
        (status = 200, description = "Successfully deleted movie"),
        (status = 404, description = "Movie is not found", body = AppError),
    ),
    tag = "Movies",
)]
pub async fn delete_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    state.delete_movie(id).await
}

/// Remove variant from the library. WARN: It will actually delete video file
#[utoipa::path(
    delete,
    path = "/api/video/{id}/variant/{variant_id}",
    params(
        ("id", description = "Video id"),
        ("variant_id", description = "Variant id"),
    ),
    responses(
        (status = 200, description = "Successfully deleted variant"),
        (status = 404, description = "Variant is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn remove_variant(
    State(state): State<AppState>,
    Path((video_id, variant_id)): Path<(i64, String)>,
) -> Result<(), AppError> {
    state.remove_variant(video_id, &variant_id).await?;
    Ok(())
}

/// Update show metadata
#[utoipa::path(
    put,
    path = "/api/show/{id}",
    params(
        ("id", description = "Show id"),
    ),
    request_body = ShowMetadata,
    responses(
        (status = 200, description = "Updated show metadata"),
        (status = 404, description = "Show is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn alter_show_metadata(
    State(db): State<Db>,
    Path(show_id): Path<i64>,
    Json(metadata): Json<ShowMetadata>,
) -> Result<(), AppError> {
    sqlx::query!(
        "UPDATE shows SET title = ?, plot = ? WHERE id = ?;",
        metadata.title,
        metadata.plot,
        show_id
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Update season metadata
#[utoipa::path(
    put,
    path = "/api/show/{id}/{season}",
    params(
        ("id", description = "Show id"),
        ("season", description = "Season number"),
    ),
    request_body = SeasonMetadata,
    responses(
        (status = 200, description = "Updated season metadata"),
        (status = 404, description = "Season is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn alter_season_metadata(
    State(db): State<Db>,
    Path((show_id, season)): Path<(i64, i64)>,
    Json(metadata): Json<SeasonMetadata>,
) -> Result<(), AppError> {
    sqlx::query!(
        "UPDATE seasons SET plot = ? WHERE show_id = ? AND number = ?;",
        metadata.plot,
        show_id,
        season
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Update episode metadata
#[utoipa::path(
    put,
    path = "/api/show/{id}/{season}/{episode}",
    params(
        ("id", description = "Show id"),
        ("season", description = "Season number"),
        ("episode", description = "Episode number"),
    ),
    request_body = EpisodeMetadata,
    responses(
        (status = 200, description = "Updated episode metadata"),
        (status = 404, description = "Episode is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn alter_episode_metadata(
    State(db): State<Db>,
    Path((show_id, season, episode)): Path<(i64, i64, i64)>,
    Json(metadata): Json<EpisodeMetadata>,
) -> Result<(), AppError> {
    sqlx::query!(
        r#"UPDATE episodes SET title = ?, plot = ?
        FROM seasons WHERE seasons.show_id = ? AND seasons.number = ? AND episodes.number = ?;"#,
        metadata.title,
        metadata.plot,
        show_id,
        season,
        episode
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Update movie metadata
#[utoipa::path(
    put,
    path = "/api/movie/{id}",
    params(
        ("id", description = "Movie id"),
    ),
    request_body = MovieMetadata,
    responses(
        (status = 200, description = "Updated movie metadata"),
        (status = 404, description = "Movie is not found", body = AppError),
    ),
    tag = "Movies",
)]
pub async fn alter_movie_metadata(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(metadata): Json<MovieMetadata>,
) -> Result<(), AppError> {
    sqlx::query!(
        "UPDATE movies SET title = ?, plot = ? WHERE id = ?;",
        metadata.title,
        metadata.plot,
        id
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}

/// Fix show metadata match
#[utoipa::path(
    post,
    path = "/api/show/{show_id}/fix_metadata",
    params(
        ("show_id", description = "Id of the show that needs to be fixed"),
        ProviderQuery,
        StringIdQuery,
    ),
    responses(
        (status = 200, description = "Fixed show metadata"),
        (status = 404, description = "Show is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn fix_show_metadata(
    State(app_state): State<AppState>,
    Path(show_id): Path<i64>,
    Query(provider_query): Query<ProviderQuery>,
    Query(new_id): Query<StringIdQuery>,
) -> Result<(), AppError> {
    let show = app_state
        .providers_stack
        .get_show(&new_id.id, provider_query.provider)
        .await?;
    let params = app_state.metadata_fetch_params();
    app_state.fix_show_metadata(show_id, show, params).await
}

/// Fix movie metadata match
#[utoipa::path(
    post,
    path = "/api/movie/{movie_id}/fix_metadata",
    params(
        ("movie_id", description = "Id of the movie that needs to be fixed"),
        ProviderQuery,
        StringIdQuery,
    ),
    responses(
        (status = 200, description = "Fixed movie metadata"),
        (status = 404, description = "Movie is not found", body = AppError),
    ),
    tag = "Movies",
)]
pub async fn fix_movie_metadata(
    State(app_state): State<AppState>,
    Path(movie_id): Path<i64>,
    Query(provider_query): Query<ProviderQuery>,
    Query(new_id): Query<StringIdQuery>,
) -> Result<(), AppError> {
    let movie = app_state
        .providers_stack
        .get_movie(&new_id.id, provider_query.provider)
        .await?;
    app_state.fix_movie_metadata(movie_id, movie).await
}

/// Fix metadata match
#[utoipa::path(
    post,
    path = "/api/fix_metadata/{content_id}",
    params(
        ("content_id", description = "Id of the content that needs to be fixed"),
        ProviderQuery,
        StringIdQuery,
        ContentTypeQuery,
    ),
    responses(
        (status = 200, description = "Fixed metadata match"),
        (status = 404, description = "Content is not found", body = AppError),
    ),
    tag = "Metadata",
)]
pub async fn fix_metadata(
    Path(content_id): Path<i64>,
    State(app_state): State<AppState>,
    Query(provider_query): Query<ProviderQuery>,
    Query(content_type_query): Query<ContentTypeQuery>,
    Query(new_id): Query<StringIdQuery>,
) -> Result<(), AppError> {
    let params = app_state.metadata_fetch_params();
    match content_type_query.content_type {
        ContentType::Movie => {
            let movie = app_state
                .providers_stack
                .get_movie(&new_id.id, provider_query.provider)
                .await?;
            app_state.fix_movie_metadata(content_id, movie).await
        }
        ContentType::Show => {
            let show = app_state
                .providers_stack
                .get_show(&new_id.id, provider_query.provider)
                .await?;
            app_state.fix_show_metadata(content_id, show, params).await
        }
    }
}

/// Reset show metadata
#[utoipa::path(
    post,
    path = "/api/show/{show_id}/reset_metadata",
    params(
        ("show_id", description = "Id of the show that needs to be fixed"),
    ),
    responses(
        (status = 200, description = "Succsessfully reset show metadata"),
        (status = 404, description = "Show is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn reset_show_metadata(
    Path(show_id): Path<i64>,
    State(app_state): State<AppState>,
) -> Result<(), AppError> {
    app_state.reset_show_metadata(show_id).await
}

/// Reset movie metadata
#[utoipa::path(
    post,
    path = "/api/movie/{movie_id}/reset_metadata",
    params(
        ("movie_id", description = "Id of the movie that needs to be fixed"),
    ),
    responses(
        (status = 200),
    ),
    tag = "Movies",
)]
pub async fn reset_movie_metadata(
    Path(movie_id): Path<i64>,
    State(app_state): State<AppState>,
) -> Result<(), AppError> {
    app_state.reset_movie_metadata(movie_id).await
}

/// Reset content's metadata
#[utoipa::path(
    post,
    path = "/api/reset_metadata/{content_id}",
    params(
        ("content_id", description = "Id of the content that needs to be fixed"),
        ContentTypeQuery,
    ),
    responses(
        (status = 200),
        (status = 404, description = "Content is not found", body = AppError),
    ),
    tag = "Metadata",
)]
pub async fn reset_metadata(
    Path(content_id): Path<i64>,
    Query(content_type_query): Query<ContentTypeQuery>,
    State(app_state): State<AppState>,
) -> Result<(), AppError> {
    match content_type_query.content_type {
        ContentType::Movie => app_state.reset_movie_metadata(content_id).await,
        ContentType::Show => app_state.reset_show_metadata(content_id).await,
    }
}

/// Start transcode video job
#[utoipa::path(
    post,
    path = "/api/video/{id}/transcode",
    params(
        ("id", description = "Video id"),
    ),
    request_body = TranscodePayload,
    responses(
        (status = 202, description = "Transcode task is started"),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn transcode_video(
    State(app_state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<TranscodePayload>,
) -> Result<StatusCode, AppError> {
    app_state.transcode_video(id, payload).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Start previews generation job on video
#[utoipa::path(
    post,
    path = "/api/video/{id}/previews",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 202, description = "Previews task is started"),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn generate_previews(
    State(app_state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppError> {
    app_state.generate_previews(id).await?;
    Ok(StatusCode::ACCEPTED)
}

/// Delete previews on video
#[utoipa::path(
    delete,
    path = "/api/video/{id}/previews",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200),
        (status = 404, description = "Previews directory is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn delete_previews(Path(id): Path<i64>) -> Result<(), AppError> {
    let previews_dir = PreviewsDirAsset::new(id);
    previews_dir.delete_dir().await?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize)]
pub struct CancelTaskPayload {
    pub task_id: Uuid,
}

/// Cancel task with provided id
#[utoipa::path(
    delete,
    path = "/api/tasks/transcode/{id}",
    params(
        ("id", description = "Task id"),
    ),
    responses(
        (status = 200),
        (status = 400, description = "Task can't be canceled or it is not found", body = AppError),
    ),
    tag = "Tasks",
)]
pub async fn cancel_transcode_task(
    State(tasks): State<&'static TaskResource>,
    Path(task_id): Path<Uuid>,
) -> Result<(), StatusCode> {
    tasks
        .transcode_tasks
        .cancel_task(task_id)
        .map_err(|_| StatusCode::BAD_REQUEST)
}

/// Get all running transcode tasks
#[utoipa::path(
    get,
    path = "/api/tasks/transcode",
    responses(
        (status = 200, body = inline(Vec<Task<TranscodeJob>>)),
    ),
    tag = "Tasks",
)]
pub async fn transcode_tasks(
    State(tasks): State<&'static TaskResource>,
) -> Json<serde_json::Value> {
    Json(tasks.transcode_tasks.tasks())
}

/// Cancel task with provided id
#[utoipa::path(
    delete,
    path = "/api/tasks/previews/{id}",
    params(
        ("id", description = "Task id"),
    ),
    responses(
        (status = 200),
        (status = 400, description = "Task can't be canceled", body = AppError),
        (status = 400, description = "Task can't be found", body = AppError),
    ),
    tag = "Tasks",
)]
pub async fn cancel_previews_task(
    State(tasks): State<&'static TaskResource>,
    Path(task_id): Path<Uuid>,
) -> Result<(), AppError> {
    tasks.previews_tasks.cancel_task(task_id)?;
    Ok(())
}

/// Get all running tasks
#[utoipa::path(
    get,
    path = "/api/tasks/previews",
    responses(
        (status = 200, body = inline(Vec<Task<PreviewsJob>>))
    ),
    tag = "Tasks",
)]
pub async fn previews_tasks(State(tasks): State<&'static TaskResource>) -> Json<serde_json::Value> {
    Json(tasks.previews_tasks.tasks())
}

/// SSE stream of current tasks progress
#[utoipa::path(
    get,
    path = "/api/tasks/progress",
    responses(
        (status = 200, body = [u8]),
    ),
    tag = "Tasks",
)]
pub async fn progress(
    State(tasks): State<&'static TaskResource>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let ProgressChannel(channel) = &tasks.progress_channel;
    let rx = channel.subscribe();

    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).map(|item| {
        if let Ok(item) = item {
            Ok(Event::default().json_data(item).unwrap())
        } else {
            Ok(Event::default())
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Latest log
#[utoipa::path(
    get,
    path = "/api/log/latest",
    responses(
        (status = 200, body = Vec<crate::tracing::JsonTracingEvent>, content_type = "application/json"),
    ),
    tag = "Logs",
)]
pub async fn latest_log() -> Result<(TypedHeader<headers::ContentType>, String), AppError> {
    use tokio::fs;
    use tokio::io;
    let log_path = &APP_RESOURCES.log_path;
    let file = fs::File::open(log_path).await?;
    let take = 40_000;
    let length = file.metadata().await?.len();
    let start = std::cmp::min(length, take) as i64;
    let mut reader = io::BufReader::new(file);
    let mut buffer = String::new();
    reader
        .seek(io::SeekFrom::End(-start))
        .await
        .expect("start is not bigger then file");
    let mut json = String::from('[');
    reader.read_line(&mut buffer).await.unwrap();
    if length < take {
        json.push_str(&buffer);
        json.push(',');
    }
    buffer.clear();
    while let Ok(amount) = reader.read_line(&mut buffer).await {
        if amount == 0 {
            break;
        }
        json.push_str(&buffer);
        json.push(',');
        buffer.clear();
    }
    // remove trailing comma xd
    json.pop();
    json.push(']');
    Ok((TypedHeader(headers::ContentType::json()), json))
}

/// Server configuration
#[utoipa::path(
    get,
    path = "/api/configuration",
    responses(
        (status = 200, body = config::UtoipaConfigSchema),
    ),
    tag = "Configuration",
)]
pub async fn server_configuration() -> Json<Vec<SerializedSetting>> {
    Json(config::CONFIG.json())
}

/// Server version
#[utoipa::path(
    get,
    path = "/api/version",
    responses(
        (status = 200, body = String),
    ),
    tag = "Configuration",
)]
pub async fn server_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Server capabalities
#[utoipa::path(
    get,
    path = "/api/configuration/capabilities",
    responses(
        (status = 200, body = Capabilities),
    ),
    tag = "Configuration",
)]
pub async fn server_capabilities() -> Result<Json<Capabilities>, AppError> {
    let capabilities = Capabilities::parse().await?;
    Ok(Json(capabilities))
}

/// Update server configuration
#[utoipa::path(
    patch,
    path = "/api/configuration",
    request_body(
        content = serde_json::Value, description = "Key/value configuration pairs", content_type = "application/json"
    ),
    responses(
        (status = 200, body = ConfigurationApplyResult),
    ),
    tag = "Configuration",
)]
pub async fn update_server_configuration(
    Json(new_config): Json<serde_json::Value>,
) -> Result<Json<ConfigurationApplyResult>, AppError> {
    let result = config::CONFIG.apply_json(new_config)?;
    let table = config::CONFIG.construct_table();

    let config_path = APP_RESOURCES.config_path.to_owned();
    let mut config_file = config::ConfigFile::open(config_path).await?;

    config_file.write_toml(table).await?;
    Ok(Json(result))
}

/// Reset server configuration to its defaults
#[utoipa::path(
    post,
    path = "/api/configuration/reset",
    responses(
        (status = 200, description = "Server configuration is reset"),
    ),
    tag = "Configuration",
)]
pub async fn reset_server_configuration() -> Result<(), AppError> {
    config::CONFIG.reset_config_values();

    let table = config::CONFIG.construct_table();

    let config_path = APP_RESOURCES.config_path.to_owned();
    let mut config_file = config::ConfigFile::open(config_path).await?;

    config_file.write_toml(table).await?;
    Ok(())
}

#[derive(Debug, Deserialize, serde::Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ProviderOrder {
    Discover(Vec<MetadataProvider>),
    Movie(Vec<MetadataProvider>),
    Show(Vec<MetadataProvider>),
    Torrent(Vec<TorrentIndexIdentifier>),
}

/// Update providers order
///
/// Returns updated order
#[utoipa::path(
    put,
    path = "/api/configuration/providers",
    request_body = ProviderOrder,
    responses(
        (status = 200, body = Vec<String>, description = "Updated ordering of providers"),
    ),
    tag = "Configuration",
)]
pub async fn order_providers(
    State(providers): State<&'static MetadataProvidersStack>,
    Json(new_order): Json<ProviderOrder>,
) -> Json<Vec<String>> {
    let new_order: Vec<_> = match new_order {
        ProviderOrder::Discover(order) => providers
            .order_discover_providers(order)
            .into_iter()
            .map(|x| x.provider_identifier().to_string())
            .collect(),
        ProviderOrder::Movie(order) => providers
            .order_movie_providers(order)
            .into_iter()
            .map(|x| x.provider_identifier().to_string())
            .collect(),
        ProviderOrder::Show(order) => providers
            .order_show_providers(order)
            .into_iter()
            .map(|x| x.provider_identifier().to_string())
            .collect(),
        ProviderOrder::Torrent(order) => providers
            .order_torrent_indexes(order)
            .into_iter()
            .map(|x| x.provider_identifier().to_string())
            .collect(),
    };
    Json(new_order)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ProviderOrderResponse {
    discover: Vec<MetadataProvider>,
    movie: Vec<MetadataProvider>,
    show: Vec<MetadataProvider>,
    torrent: Vec<TorrentIndexIdentifier>,
}

/// Get providers order
#[utoipa::path(
    get,
    path = "/api/configuration/providers",
    responses(
        (status = 200, body = ProviderOrderResponse, description = "Ordering of providers"),
    ),
    tag = "Configuration",
)]
pub async fn get_providers_order(
    State(providers): State<&'static MetadataProvidersStack>,
) -> Json<ProviderOrderResponse> {
    let movie = providers
        .movie_providers()
        .iter()
        .map(|p| p.provider_identifier())
        .collect();

    let show = providers
        .show_providers()
        .iter()
        .map(|p| p.provider_identifier())
        .collect();
    let discover = providers
        .discover_providers()
        .iter()
        .map(|p| p.provider_identifier())
        .collect();

    let torrent = providers
        .torrent_indexes()
        .iter()
        .map(|p| p.provider_identifier())
        .collect();

    Json(ProviderOrderResponse {
        movie,
        show,
        discover,
        torrent,
    })
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateHistoryPayload {
    time: i64,
    is_finished: bool,
}

/// Update history entry
#[utoipa::path(
    put,
    path = "/api/history/{id}",
    params(
        ("id", description = "History id"),
    ),
    request_body = UpdateHistoryPayload,
    responses(
        (status = 200, description = "History update is successful"),
        (status = 404, description = "History entry is not found", body = AppError),
    ),
    tag = "History",
)]
pub async fn update_history(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdateHistoryPayload>,
) -> Result<(), AppError> {
    sqlx::query!(
        "UPDATE history SET time = ?, is_finished = ? WHERE id = ? RETURNING time;",
        payload.time,
        payload.is_finished,
        id,
    )
    .fetch_one(&db.pool)
    .await?;

    Ok(())
}

/// Delete all history for the default user
#[utoipa::path(
    delete,
    path = "/api/history",
    responses(
        (status = 200),
    ),
    tag = "History",
)]
pub async fn clear_history(State(db): State<Db>) -> Result<(), AppError> {
    sqlx::query!("DELETE FROM history")
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Delete history entry
#[utoipa::path(
    delete,
    path = "/api/history/{id}",
    params(
        ("id", description = "History id"),
    ),
    responses(
        (status = 200, description = "Successfully removed history item"),
        (status = 404, description = "History entry is not found", body = AppError),
    ),
    tag = "History",
)]
pub async fn remove_history_item(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    sqlx::query!("DELETE FROM history WHERE id = ?;", id)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Update/Insert video history
#[utoipa::path(
    put,
    path = "/api/video/{id}/history",
    params(
        ("id", description = "Video id"),
    ),
    request_body = UpdateHistoryPayload,
    responses(
        (status = 200, description = "History entry is updated"),
        (status = 201, description = "History is created"),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn update_video_history(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdateHistoryPayload>,
) -> Result<StatusCode, AppError> {
    let update_time = OffsetDateTime::now_utc();
    let query = sqlx::query!(
        "UPDATE history SET time = ?, is_finished = ?, update_time = ? WHERE video_id = ? RETURNING time;",
        payload.time,
        payload.is_finished,
        update_time,
        id,
    );
    if let Err(err) = query.fetch_one(&db.pool).await {
        match err {
            sqlx::Error::RowNotFound => {
                db.pool
                    .insert_history(crate::db::DbHistory {
                        id: None,
                        time: payload.time,
                        is_finished: payload.is_finished,
                        update_time,
                        video_id: id,
                    })
                    .await?;
                return Ok(StatusCode::CREATED);
            }
            rest => return Err(rest.into()),
        };
    }
    Ok(StatusCode::OK)
}

/// Delete video history entry
#[utoipa::path(
    delete,
    path = "/api/video/{id}/history",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200, description = "History entry is deleted"),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn remove_video_history(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    sqlx::query!("DELETE FROM history WHERE video_id = ?;", id)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Root and other related directories/drives
#[utoipa::path(
    get,
    path = "/api/file_browser/root_dirs",
    responses(
        (status = 200, body = BrowseRootDirs),
    ),
    tag = "FileBrowser",
)]
pub async fn root_dirs() -> Json<BrowseRootDirs> {
    Json(BrowseRootDirs::new())
}

/// Browse internals of the given directory
#[utoipa::path(
    get,
    path = "/api/file_browser/browse/{key}",
    params(
        ("key" = String, description = "Key of directory to explore. It is base64 encoded path in current implementation"),
    ),
    responses(
        (status = 200, body = BrowseDirectory),
    ),
    tag = "FileBrowser",
)]
pub async fn browse_directory(Path(key): Path<FileKey>) -> Result<Json<BrowseDirectory>, AppError> {
    let resolved_dir = BrowseDirectory::explore(key).await?;
    Ok(Json(resolved_dir))
}

/// Get parent directory. Returns same directory if parent is not found
#[utoipa::path(
    get,
    path = "/api/file_browser/parent/{key}",
    params(
        ("key" = String, description = "Get parent directory"),
    ),
    responses(
        (status = 200, body = BrowseFile),
    ),
    tag = "FileBrowser",
)]
pub async fn parent_directory(Path(mut key): Path<FileKey>) -> Result<Json<BrowseFile>, AppError> {
    if let Some(parent) = key.path.parent() {
        key.path = parent.to_owned();
    }
    let resolved_dir = BrowseFile::from(key.path);
    Ok(Json(resolved_dir))
}

/// Retrieve transcoded segment
#[utoipa::path(
    get,
    path = "/api/transcode/{id}/segment/{segment}",
    params(
        ("id", description = "Transcode job"),
        ("segment", description = "Desired segment"),
    ),
    responses(
        (status = 200),
        (status = 404, description = "Transcode job is not found", body = AppError),
        (status = 500, description = "Worker is not available", body = AppError),
    ),
    tag = "Transcoding",
)]
pub async fn transcoded_segment(
    Path((task_id, index)): Path<(String, usize)>,
    State(tasks): State<&'static TaskResource>,
) -> Result<bytes::Bytes, AppError> {
    let sender = {
        let tasks = tasks.active_streams.lock().unwrap();
        let task_id = uuid::Uuid::from_str(&task_id).unwrap();
        let stream = tasks
            .iter()
            .find(|t| t.uuid == task_id)
            .ok_or(AppError::not_found("Requested stream is not found"))?;
        stream.sender.clone()
    };
    let (tx, rx) = oneshot::channel();
    sender
        .send((index, tx))
        .await
        .map_err(|_| AppError::bad_request("Stream is not available"))?;
    if let Ok(bytes) = rx.await {
        Ok(bytes)
    } else {
        Err(AppError::internal_error(
            "Transcode worker is not available",
        ))
    }
}

/// Start transcoded stream
#[utoipa::path(
    post,
    path = "/api/video/{id}/stream_transcode",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200, body = Task<TranscodeJob>),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Transcoding",
)]
pub async fn create_transcode_stream(
    Path(_id): Path<i64>,
    State(_app_state): State<AppState>,
) -> Result<Json<Task<TranscodeJob>>, AppError> {
    todo!()
}

/// M3U8 manifest of live transcode task
#[utoipa::path(
    get,
    path = "/api/transcode/{id}/manifest",
    params(
        ("id", description = "Task id"),
    ),
    responses(
        (status = 200, body = String),
        (status = 400, description = "Task uuid is incorrect", body = AppError),
        (status = 404, description = "Task is not found", body = AppError),
    ),
    tag = "Transcoding",
)]
pub async fn transcode_stream_manifest(
    Path(stream_id): Path<String>,
    State(tasks): State<&'static TaskResource>,
) -> Result<String, AppError> {
    let streams = tasks.active_streams.lock().unwrap();
    let id = uuid::Uuid::from_str(&stream_id)
        .map_err(|_| AppError::bad_request("Failed to parse uuid"))?;
    let stream = streams
        .iter()
        .find(|s| s.uuid == id)
        .ok_or(AppError::not_found("Stream is not found"))?;

    Ok(stream.manifest.as_ref().to_string())
}

/// Detect intros for given season
#[utoipa::path(
    post,
    path = "/api/show/{show_id}/{season}/detect_intros",
    params(
        ("show_id", description = "Show id"),
        ("season", description = "Season number"),
    ),
    responses(
        (status = 202, description = "Intro detection task is started"),
        (status = 404, description = "Season is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn detect_intros(
    Path((show_id, season)): Path<(i64, i64)>,
    State(app_state): State<AppState>,
) -> Result<(), AppError> {
    let tasks = app_state.tasks;
    let job = IntroJob {
        show_id,
        season: season as usize,
    };
    let id = tasks.intro_detection_tasks.start_task(job, None)?;
    tokio::spawn(async move {
        match app_state.detect_intros(show_id, season).await {
            Ok(_) => tasks.intro_detection_tasks.finish_task(id),
            Err(_) => tasks
                .intro_detection_tasks
                .error_task(id, TaskError::Failure),
        };
    });
    Ok(())
}

/// Get intro for the video
#[utoipa::path(
    get,
    path = "/api/video/{video_id}/intro",
    params(
        ("video_id", description = "Video Id"),
    ),
    responses(
        (status = 200, description = "Intro"),
        (status = 404, description = "Intro is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn video_intro(
    Path(video_id): Path<i64>,
    State(db): State<Db>,
) -> Result<Json<Intro>, AppError> {
    let intro = sqlx::query_as!(
        Intro,
        "SELECT start_sec, end_sec FROM episode_intro WHERE video_id = ?",
        video_id,
    )
    .fetch_one(&db.pool)
    .await?;
    Ok(Json(intro))
}

/// Delete intro for the video
#[utoipa::path(
    delete,
    path = "/api/video/{video_id}/intro",
    params(
        ("video_id", description = "Video Id"),
    ),
    responses(
        (status = 200, description = "Intro was removed successfully"),
        (status = 404, description = "Intro is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn delete_video_intro(
    Path(video_id): Path<i64>,
    State(db): State<Db>,
) -> Result<(), AppError> {
    sqlx::query!(
        "DELETE FROM episode_intro WHERE video_id = ? RETURNING id",
        video_id,
    )
    .fetch_one(&db.pool)
    .await?;
    Ok(())
}

/// Delete all season intros
#[utoipa::path(
    delete,
    path = "/api/show/{show_id}/{season}/intros",
    params(
        ("show_id", description = "Show id"),
        ("season", description = "Season number"),
    ),
    responses(
        (status = 200, description = "Intros are removed"),
        (status = 404, description = "Season is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn delete_season_intros(
    Path((show_id, season)): Path<(i64, i64)>,
    State(db): State<Db>,
) -> Result<(), AppError> {
    let mut tx = db.pool.begin().await?;
    let intros = sqlx::query!(
        r#"SELECT episode_intro.id FROM episode_intro
        JOIN videos ON videos.id = episode_intro.video_id
        JOIN episodes ON episodes.id = videos.episode_id
        JOIN seasons ON seasons.id = episodes.season_id
        WHERE seasons.show_id = ? AND seasons.number = ?;"#,
        show_id,
        season,
    )
    .fetch_all(&mut *tx)
    .await?;
    for intro in intros {
        tx.remove_intro(intro.id).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Delete all intros for the episode
#[utoipa::path(
    delete,
    path = "/api/show/{show_id}/{season}/{episode}/intros",
    params(
        ("show_id", description = "Show id"),
        ("season", description = "Season number"),
        ("episode", description = "Episode number"),
    ),
    responses(
        (status = 200, description = "Intros are removed"),
        (status = 404, description = "Episode is not found", body = AppError),
    ),
    tag = "Shows",
)]
pub async fn delete_episode_intros(
    Path((show_id, season, episode)): Path<(i64, i64, i64)>,
    State(db): State<Db>,
) -> Result<(), AppError> {
    let intros = sqlx::query!(
        r#"SELECT episode_intro.id FROM episode_intro
        JOIN videos ON videos.id = episode_intro.video_id
        JOIN episodes ON episodes.id = videos.episode_id
        JOIN seasons ON seasons.id = episodes.season_id
        WHERE seasons.show_id = ? AND seasons.number = ? AND episodes.number = ?;"#,
        show_id,
        season,
        episode
    )
    .fetch_all(&db.pool)
    .await?;
    let mut tx = db.pool.begin().await?;
    for intro in intros {
        tx.remove_intro(intro.id).await?;
    }
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct EditIntroPayload {
    /// Start range specified in seconds
    start: i64,
    /// End range specified in seconds
    end: i64,
}

/// Update intros for the video
/// If into does not exist it will be created
#[utoipa::path(
    put,
    path = "/api/video/{video_id}/intro",
    params(
        ("video_id", description = "Video Id"),
    ),
    request_body = EditIntroPayload,
    responses(
        (status = 200, description = "Intro is updated"),
        (status = 201, description = "Intro is newly created"),
        (status = 400, description = "Intro payload is incorrect", body = AppError),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn update_video_intro(
    Path(video_id): Path<i64>,
    State(db): State<Db>,
    Json(EditIntroPayload { start, end }): Json<EditIntroPayload>,
) -> Result<StatusCode, AppError> {
    if start < 0 || end < 0 {
        return Err(AppError::bad_request("intro can't timing must be > 0"));
    }
    if end < start {
        return Err(AppError::bad_request("start timing can't be less than end"));
    }

    let update = sqlx::query!(
        "UPDATE episode_intro SET start_sec = ?, end_sec = ? WHERE video_id = ? RETURNING id;",
        start,
        end,
        video_id,
    )
    .fetch_one(&db.pool)
    .await;
    match update {
        Ok(r) => {
            tracing::trace!("Updated intro with id {}", r.id);
            Ok(StatusCode::OK)
        }
        Err(sqlx::Error::RowNotFound) => {
            let db_intro = crate::db::DbEpisodeIntro {
                id: None,
                video_id,
                start_sec: start,
                end_sec: end,
            };
            db.insert_intro(db_intro).await?;
            Ok(StatusCode::CREATED)
        }
        Err(e) => Err(e)?,
    }
}

/// Get all running tasks
#[utoipa::path(
    get,
    path = "/api/tasks/intro_detection",
    responses(
        (status = 200, body = inline(Vec<Task<IntroJob>>))
    ),
    tag = "Tasks",
)]
pub async fn intro_detection_tasks(
    State(tasks): State<&'static TaskResource>,
) -> Json<serde_json::Value> {
    Json(tasks.intro_detection_tasks.tasks())
}

#[cfg(test)]
mod tests {
    use crate::library::TranscodePayload;

    #[test]
    fn parse_transcode_payload() {
        use crate::library::{AudioCodec, VideoCodec};
        let json = serde_json::json!({
            "audio_codec": "aac",
            "audio_track": 2,
            "video_codec": "hevc",
            "resolution": "1920x1080",
        })
        .to_string();
        let payload: TranscodePayload = serde_json::from_str(&json).unwrap();
        assert_eq!(payload.audio_codec.unwrap(), AudioCodec::AAC);
        assert_eq!(payload.video_codec.unwrap(), VideoCodec::Hevc);
        assert_eq!(payload.resolution.unwrap(), (1920, 1080).into());
    }
}
