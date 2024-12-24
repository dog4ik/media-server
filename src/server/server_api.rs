use std::convert::Infallible;
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use axum_extra::headers::Range;
use axum_extra::{headers, TypedHeader};
use base64::Engine;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::sync::oneshot;
use tokio_stream::{Stream, StreamExt};
use uuid::Uuid;

use super::{ContentTypeQuery, ProviderQuery, StringIdQuery};
use super::{CursorQuery, IdQuery, NumberQuery, SearchQuery, TakeParam, VariantQuery};
use crate::app_state::AppError;
use crate::config::{
    self, Capabilities, ConfigurationApplyResult, SerializedSetting, APP_RESOURCES,
};
use crate::db::{DbActions, DbEpisodeIntro};
use crate::db::{DbExternalId, DbHistory};
use crate::ffmpeg::{FFprobeAudioStream, FFprobeSubtitleStream, FFprobeVideoStream};
use crate::ffmpeg::{PreviewsJob, TranscodeJob};
use crate::file_browser::{BrowseDirectory, BrowseFile, BrowseRootDirs, FileKey};
use crate::library::assets::{AssetDir, PreviewsDirAsset};
use crate::library::assets::{
    BackdropAsset, BackdropContentType, FileAsset, PosterAsset, PosterContentType, PreviewAsset,
    VariantAsset,
};
use crate::library::TranscodePayload;
use crate::library::{
    AudioCodec, ContentIdentifier, Resolution, Source, SubtitlesCodec, VideoCodec,
};
use crate::metadata::tmdb_api::TmdbApi;
use crate::metadata::{
    metadata_stack::MetadataProvidersStack, ContentType, EpisodeMetadata, MovieMetadata,
    SeasonMetadata, ShowMetadata,
};
use crate::metadata::{ExternalIdMetadata, MetadataSearchResult};
use crate::progress::{LibraryScanTask, Task, TaskError, TaskResource};
use crate::torrent_index::Torrent;
use crate::{app_state::AppState, db::Db, progress::ProgressChannel};

#[derive(Debug, Serialize, FromRow, utoipa::ToSchema)]
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
    pub sample_rate: String,
    pub channels: i32,
    pub profile: Option<String>,
    pub codec: AudioCodec,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedSubtitleTrack {
    pub is_default: bool,
    pub language: Option<String>,
    pub codec: SubtitlesCodec,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedVideoTrack {
    pub is_default: bool,
    pub resolution: Resolution,
    pub profile: String,
    pub level: i32,
    pub bitrate: usize,
    pub framerate: f64,
    pub codec: VideoCodec,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Intro {
    start_sec: i64,
    end_sec: i64,
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
            size: source.video.file_size(),
            duration: video_metadata.duration(),
            variants: detailed_variants,
            scan_date: date.to_string(),
            video_tracks: video_metadata
                .video_streams()
                .into_iter()
                .map(|s| DetailedVideoTrack::from_video_stream(s, video_metadata.bitrate()))
                .collect(),
            audio_tracks: video_metadata
                .audio_streams()
                .into_iter()
                .map(|s| s.into())
                .collect(),
            subtitle_tracks: video_metadata
                .subtitle_streams()
                .into_iter()
                .map(|s| s.into())
                .collect(),
            history,
            intro,
        })
    }
}

impl DetailedVideoTrack {
    pub fn from_video_stream(stream: FFprobeVideoStream<'_>, bitrate: usize) -> Self {
        DetailedVideoTrack {
            is_default: stream.is_default(),
            resolution: stream.resolution(),
            profile: stream.profile.to_string(),
            level: stream.level,
            bitrate,
            framerate: stream.framerate(),
            codec: stream.codec(),
        }
    }
}

impl From<FFprobeAudioStream<'_>> for DetailedAudioTrack {
    fn from(val: FFprobeAudioStream<'_>) -> Self {
        DetailedAudioTrack {
            is_default: val.disposition.default == 1,
            sample_rate: val.sample_rate.to_string(),
            channels: val.channels,
            profile: val.profile.map(|x| x.to_string()),
            codec: val.codec(),
        }
    }
}

impl From<FFprobeSubtitleStream<'_>> for DetailedSubtitleTrack {
    fn from(val: FFprobeSubtitleStream<'_>) -> Self {
        DetailedSubtitleTrack {
            is_default: val.is_default(),
            language: val.language.map(|x| x.to_string()),
            codec: val.codec(),
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
            size: video.file_size(),
            duration: metadata.duration(),
            video_tracks: metadata
                .video_streams()
                .into_iter()
                .map(|s| DetailedVideoTrack::from_video_stream(s, metadata.bitrate()))
                .collect(),
            audio_tracks: metadata
                .audio_streams()
                .into_iter()
                .map(|s| s.into())
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

/// Pull subtitle from video file
#[utoipa::path(
    get,
    path = "/api/video/{id}/pull_subtitle",
    params(
        ("id", description = "video id"),
        NumberQuery,
    ),
    responses(
        (status = 200, description = "Subtitle", body = String),
        (status = 404, description = "Video is not found", body = AppError),
    ),
    tag = "Videos",
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

/// Video stream
#[utoipa::path(
    get,
    path = "/api/video/{id}/watch",
    params(
        ("id", description = "video id"),
        VariantQuery,
    ),
    responses(
        (status = 206, description = "Video progressive download stream", body = [u8], content_type = "video/x-matroska"),
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
        (status = 206, description = "Video progressive download stream", content_type = "video/x-matroska"),
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
        (status = 206, description = "Movie video progressive download stream", content_type = "video/x-matroska"),
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
        (status = 404, body = AppError),
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

/// Get video by content local id
#[utoipa::path(
    get,
    path = "/api/video/by_content",
    params(
        ContentTypeQuery,
        IdQuery,
    ),
    responses(
        (status = 200, description = "Desired video", body = DetailedVideo),
        (status = 404, description = "Video is not found"),
    ),
    tag = "Videos",
)]
pub async fn contents_video(
    Query(IdQuery { id }): Query<IdQuery>,
    Query(content_type): Query<ContentTypeQuery>,
    State(state): State<AppState>,
) -> Result<Json<DetailedVideo>, AppError> {
    let video_id = match content_type.content_type {
        crate::metadata::ContentType::Movie => {
            sqlx::query!("SELECT id FROM videos WHERE movie_id = ?", id)
                .fetch_one(&state.db.pool)
                .await
                .map(|x| x.id)
        }
        crate::metadata::ContentType::Show => {
            sqlx::query!("SELECT id FROM videos WHERE episode_id = ?", id)
                .fetch_one(&state.db.pool)
                .await
                .map(|x| x.id)
        }
    }?;
    get_video_by_id(Path(video_id), State(state)).await
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
    ),
    responses(
        (status = 200, description = "Torrent search results", body = Vec<Torrent>),
    ),
    tag = "Torrent",
)]
pub async fn search_torrent(
    Query(query): Query<SearchQuery>,
    State(providers): State<&'static MetadataProvidersStack>,
) -> Result<Json<Vec<Torrent>>, AppError> {
    if query.search.is_empty() {
        return Ok(Json(Vec::new()));
    }
    let out = providers.get_torrents(&query.search).await;
    Ok(Json(out))
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
    State(tmdb_api): State<&'static TmdbApi>,
) -> Result<Json<Vec<ShowMetadata>>, AppError> {
    let res = tmdb_api.trending_shows().await?;
    let shows = res
        .results
        .into_iter()
        .map(|search_result| search_result.into())
        .collect();
    Ok(Json(shows))
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
    State(tmdb_api): State<&'static TmdbApi>,
) -> Result<Json<Vec<MovieMetadata>>, AppError> {
    let res = tmdb_api.trending_movies().await?;
    let shows = res
        .results
        .into_iter()
        .map(|search_result| search_result.into())
        .collect();
    Ok(Json(shows))
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
        (status = 200),
        (status = 400, body = AppError),
    ),
    tag = "Videos",
)]
pub async fn reconciliate_lib(State(app_state): State<AppState>) -> Result<(), AppError> {
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
    Ok(())
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
    ),
    tag = "Videos",
)]
pub async fn remove_video(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    state.remove_video(id).await
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
        (status = 200),
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
        (status = 200),
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
        (status = 200),
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
        (status = 200),
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
        (status = 200),
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

/// Delete movie
#[utoipa::path(
    delete,
    path = "/api/movie/{id}",
    params(
        ("id", description = "Movie id"),
    ),
    responses(
        (status = 200),
    ),
    tag = "Movies",
)]
pub async fn delete_movie(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    // We can just remove video and refresh the library for cleanup instead of doing it manually
    // for every content type in the library!
    let AppState { db, library, .. } = state;
    let video_id = sqlx::query!(
        r#"SELECT videos.id FROM movies
        JOIN videos ON movies.id = videos.movie_id
        WHERE movies.id = ?;"#,
        id
    )
    .fetch_one(&db.pool)
    .await?
    .id;

    // TODO: Fix not found errors when video assets do not exist

    let library_file = {
        let mut library = library.lock().unwrap();
        library.videos.remove(&video_id)
    };
    if let Some(movie) = library_file {
        let (resources_removal, video_removal) = tokio::join!(
            movie.source.delete_all_resources(),
            movie.source.video.delete(),
        );
        if let Err(err) = resources_removal {
            tracing::warn!("Failed to cleanup video resources dir: {err}");
        };

        if let Err(err) = video_removal {
            tracing::error!("Failed to delete video: {err}");
        };
    }
    db.pool.remove_movie(id).await?;
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
        (status = 200),
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
        (status = 200),
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
        (status = 200),
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
        (status = 200),
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
        (status = 200),
    ),
    tag = "Videos",
)]
pub async fn transcode_video(
    State(app_state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<TranscodePayload>,
) -> Result<(), AppError> {
    app_state.transcode_video(id, payload).await?;
    Ok(())
}

/// Start previews generation job on video
#[utoipa::path(
    post,
    path = "/api/video/{id}/previews",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200),
    ),
    tag = "Videos",
)]
pub async fn generate_previews(
    State(app_state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    app_state.generate_previews(id).await
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
        (status = 400, description = "Task can't be canceled or it is not found"),
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
        (status = 200, body = Vec<Task<TranscodeJob>>),
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
        (status = 400, description = "Task can't be canceled or it is not found"),
    ),
    tag = "Tasks",
)]
pub async fn cancel_previews_task(
    State(tasks): State<&'static TaskResource>,
    Path(task_id): Path<Uuid>,
) -> Result<(), StatusCode> {
    tasks
        .previews_tasks
        .cancel_task(task_id)
        .map_err(|_| StatusCode::BAD_REQUEST)
}

/// Get all running tasks
#[utoipa::path(
    get,
    path = "/api/tasks/previews",
    responses(
        (status = 200, body = Vec<Task<PreviewsJob>>),
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
        (status = 200),
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
pub enum ProviderType {
    Discover,
    Movie,
    Show,
    Torrent,
}

#[derive(Debug, Deserialize, serde::Serialize, utoipa::ToSchema)]
pub struct ProviderOrder {
    provider_type: ProviderType,
    order: Vec<String>,
}

/// Update providers order
#[utoipa::path(
    put,
    path = "/api/configuration/providers",
    request_body = ProviderOrder,
    responses(
        (status = 200, body = ProviderOrder, description = "Updated ordering of providers"),
    ),
    tag = "Configuration",
)]
pub async fn order_providers(
    State(providers): State<&'static MetadataProvidersStack>,
    Json(payload): Json<ProviderOrder>,
) -> Json<ProviderOrder> {
    let new_order: Vec<_> = match payload.provider_type {
        ProviderType::Discover => providers
            .order_discover_providers(payload.order)
            .into_iter()
            .map(|x| x.provider_identifier().to_string())
            .collect(),
        ProviderType::Movie => providers
            .order_movie_providers(payload.order)
            .into_iter()
            .map(|x| x.provider_identifier().to_string())
            .collect(),
        ProviderType::Show => providers
            .order_show_providers(payload.order)
            .into_iter()
            .map(|x| x.provider_identifier().to_string())
            .collect(),
        ProviderType::Torrent => providers
            .order_torrent_indexes(payload.order)
            .into_iter()
            .map(|x| x.provider_identifier().to_string())
            .collect(),
    };
    Json(ProviderOrder {
        provider_type: payload.provider_type,
        order: new_order,
    })
}

/// Get providers order
#[utoipa::path(
    get,
    path = "/api/configuration/providers",
    responses(
        (status = 200, body = Vec<ProviderOrder>, description = "Ordering of providers"),
    ),
    tag = "Configuration",
)]
pub async fn get_providers_order(
    State(providers): State<&'static MetadataProvidersStack>,
) -> Json<Vec<ProviderOrder>> {
    let movie_order = ProviderOrder {
        provider_type: ProviderType::Movie,
        order: providers
            .movie_providers()
            .iter()
            .map(|p| p.provider_identifier().into())
            .collect(),
    };
    let show_order = ProviderOrder {
        provider_type: ProviderType::Show,
        order: providers
            .show_providers()
            .iter()
            .map(|p| p.provider_identifier().into())
            .collect(),
    };
    let discover_order = ProviderOrder {
        provider_type: ProviderType::Discover,
        order: providers
            .discover_providers()
            .iter()
            .map(|p| p.provider_identifier().into())
            .collect(),
    };
    let torrent_order = ProviderOrder {
        provider_type: ProviderType::Torrent,
        order: providers
            .torrent_indexes()
            .iter()
            .map(|p| p.provider_identifier().into())
            .collect(),
    };
    Json(vec![movie_order, show_order, discover_order, torrent_order])
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateHistoryPayload {
    time: i64,
    is_finished: bool,
}

/// Update history
#[utoipa::path(
    put,
    path = "/api/history/{id}",
    params(
        ("id", description = "History id"),
    ),
    request_body = UpdateHistoryPayload,
    responses(
        (status = 200),
    ),
    tag = "History",
)]
pub async fn update_history(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdateHistoryPayload>,
) -> Result<StatusCode, AppError> {
    let query = sqlx::query!(
        "UPDATE history SET time = ?, is_finished = ? WHERE id = ? RETURNING time;",
        payload.time,
        payload.is_finished,
        id,
    );
    query.fetch_one(&db.pool).await?;
    Ok(StatusCode::OK)
}

/// Delete all history for default user
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
        (status = 200),
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
        (status = 200),
        (status = 201, description = "History is created"),
    ),
    tag = "Videos",
)]
pub async fn update_video_history(
    State(db): State<Db>,
    Path(id): Path<i64>,
    Json(payload): Json<UpdateHistoryPayload>,
) -> Result<StatusCode, AppError> {
    let query = sqlx::query!(
        "UPDATE history SET time = ?, is_finished = ? WHERE video_id = ? RETURNING time;",
        payload.time,
        payload.is_finished,
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
                        update_time: OffsetDateTime::now_utc(),
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
        (status = 200),
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
        (status = 404, description = "Transcode job is not found"),
        (status = 500, description = "Worker is not available"),
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
        (status = 404, description = "Video is not found"),
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
        (status = 400, description = "Task uuid is incorrect"),
        (status = 404, description = "Task is not found"),
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
        (status = 200),
        (status = 404, description = "Show or season are not found"),
    ),
    tag = "Shows",
)]
pub async fn detect_intros(
    Path((show_id, season)): Path<(i64, i64)>,
    State(app_state): State<AppState>,
) -> Result<(), AppError> {
    let AppState { db, library, .. } = app_state;
    let video_ids = sqlx::query!(
        r#"SELECT videos.id FROM episodes
        JOIN seasons ON seasons.id = episodes.season_id
        JOIN videos ON videos.episode_id = episodes.id
        WHERE seasons.show_id = ? AND seasons.number = ?;"#,
        show_id,
        season,
    )
    .fetch_all(&db.pool)
    .await?;
    let paths: Vec<PathBuf> = {
        let library = library.lock().unwrap();
        let mut paths = Vec::with_capacity(video_ids.len());
        for id in &video_ids {
            paths.push(
                library
                    .videos
                    .get(&id.id)
                    .map(|s| s.source.video.path().to_path_buf())
                    .ok_or(AppError::internal_error("One of the episodes is not found"))?,
            );
        }
        paths
    };
    let intros = crate::intro_detection::intro_detection(paths).await?;
    for (i, intro) in intros.into_iter().enumerate() {
        let id = video_ids[i].id;
        if let Some(intro) = intro {
            let db_intro = DbEpisodeIntro {
                id: None,
                video_id: id,
                start_sec: intro.start.as_secs() as i64,
                end_sec: intro.end.as_secs() as i64,
            };
            if let Err(e) = db.pool.insert_intro(db_intro).await {
                tracing::warn!("Failed to insert intro for video id({id}): {e}");
            };
        } else {
            tracing::warn!("Could not detect intro for video with id {id}");
        }
    }
    Ok(())
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
