use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::headers::Range;
use axum_extra::TypedHeader;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use tokio_util::io::ReaderStream;

use crate::app_state::AppError;
use crate::db::{DbExternalId, DbHistory};
use crate::ffmpeg::{FFprobeAudioStream, FFprobeSubtitleStream, FFprobeVideoStream};
use crate::library::assets::{
    FileAsset, PreviewAsset, PreviewsDirAsset, SubtitleAsset, VariantAsset,
};
use crate::library::{AudioCodec, Resolution, Source, SubtitlesCodec, VideoCodec};
use crate::metadata::{
    EpisodeMetadata, ExternalIdMetadata, MetadataProvider, MetadataProvidersStack,
    MetadataSearchResult, SeasonMetadata, ShowMetadata, ShowMetadataProvider,
};
use crate::torrent_index::Torrent;
use crate::{app_state::AppState, db::Db};

use super::content::ServeContent;
use super::{
    ContentTypeQuery, IdQuery, LanguageQuery, NumberQuery, PageQuery, ProviderQuery, SearchQuery,
    StringIdQuery, VariantQuery,
};

fn sqlx_err_wrap(err: sqlx::Error) -> StatusCode {
    match err {
        sqlx::Error::RowNotFound => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ContentRequestQuery {
    origin: Option<MetadataProvider>,
    id: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct DetailedVideo {
    pub id: i64,
    pub path: PathBuf,
    pub previews_count: usize,
    pub size: u64,
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
    pub subtitle_tracks: Vec<DetailedSubtitleTrack>,
    pub variants: Vec<DetailedVariant>,
    pub scan_date: String,
    pub history: Option<DbHistory>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetailedVariant {
    pub id: String,
    pub path: PathBuf,
    pub size: u64,
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetailedAudioTrack {
    pub is_default: bool,
    pub sample_rate: String,
    pub channels: i32,
    pub profile: Option<String>,
    pub codec: AudioCodec,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetailedSubtitleTrack {
    pub is_default: bool,
    pub language: Option<String>,
    pub codec: SubtitlesCodec,
}

#[derive(Debug, Clone, Serialize)]
pub struct DetailedVideoTrack {
    pub is_default: bool,
    pub resolution: Resolution,
    pub profile: String,
    pub level: i32,
    pub bitrate: usize,
    pub framerate: f64,
    pub codec: VideoCodec,
}

impl DetailedVideoTrack {
    pub fn from_video_stream(stream: FFprobeVideoStream<'_>, bitrate: usize) -> Self {
        DetailedVideoTrack {
            is_default: stream.is_default(),
            resolution: stream.resoultion(),
            profile: stream.profile.to_string(),
            level: stream.level,
            bitrate,
            framerate: stream.framerate(),
            codec: stream.codec(),
        }
    }
}

impl Into<DetailedAudioTrack> for FFprobeAudioStream<'_> {
    fn into(self) -> DetailedAudioTrack {
        DetailedAudioTrack {
            is_default: self.disposition.default == 1,
            sample_rate: self.sample_rate.to_string(),
            channels: self.channels,
            profile: self.profile.map(|x| x.to_string()),
            codec: self.codec(),
        }
    }
}

impl Into<DetailedSubtitleTrack> for FFprobeSubtitleStream<'_> {
    fn into(self) -> DetailedSubtitleTrack {
        DetailedSubtitleTrack {
            is_default: self.is_defalut(),
            language: self.language.map(|x| x.to_string()),
            codec: self.codec(),
        }
    }
}

impl DetailedVariant {
    pub fn from_video(video: crate::library::Video) -> Self {
        let id = video
            .path()
            .file_stem()
            .expect("file to have stem like {size}.{hash}")
            .to_string_lossy()
            .to_string();
        Self {
            id,
            size: video.file_size(),
            duration: video.duration(),
            video_tracks: video
                .video_streams()
                .into_iter()
                .map(|s| DetailedVideoTrack::from_video_stream(s, video.bitrate()))
                .collect(),
            audio_tracks: video
                .audio_streams()
                .into_iter()
                .map(|s| s.into())
                .collect(),
            path: video.path().to_path_buf(),
        }
    }
}

pub async fn previews(
    Query(video_id): Query<IdQuery>,
    Query(number): Query<NumberQuery>,
) -> Result<Body, AppError> {
    let preview_asset = PreviewAsset::new(video_id.id, number.number);
    let file = preview_asset.open().await?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    Ok(body)
}

pub async fn subtitles(
    Query(video_id): Query<IdQuery>,
    Query(lang): Query<LanguageQuery>,
) -> Result<Body, AppError> {
    let subtitle_asset = SubtitleAsset::new(video_id.id, lang.lang.unwrap_or_default());
    let file = subtitle_asset.open().await?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);
    Ok(body)
}

pub async fn pull_video_subtitle(
    Query(video_id): Query<IdQuery>,
    Query(number): Query<NumberQuery>,
    State(state): State<AppState>,
) -> Result<String, AppError> {
    state
        .pull_subtitle_from_video(video_id.id, number.number)
        .await
}

pub async fn watch(
    Query(video_id): Query<IdQuery>,
    variant: Option<Query<VariantQuery>>,
    State(state): State<AppState>,
    range: Option<TypedHeader<Range>>,
) -> Result<impl IntoResponse, AppError> {
    if let Some(Query(VariantQuery { variant })) = variant {
        let variant_asset = VariantAsset::new(video_id.id, variant);
        let video = variant_asset.video().await?;
        return Ok(video.serve(range).await);
    } else {
        let AppState { library, .. } = state;
        let video = {
            let library = library.lock().unwrap();
            library
                .get_source(video_id.id)
                .map(|x| x.video.clone())
                .ok_or(AppError::not_found("Video not found"))?
        };
        return Ok(video.serve(range).await);
    }
}

pub async fn all_local_shows(
    Query(q): Query<PageQuery>,
    State(db): State<Db>,
) -> Result<Json<Vec<ShowMetadata>>, AppError> {
    const PAGE_SIZE: i32 = 20;
    let page = (q.page.unwrap_or(1) - 1).max(0) as i32;
    let offset = page * PAGE_SIZE;
    Ok(Json(db.all_shows().await?))
}

pub async fn local_show(
    Query(id): Query<StringIdQuery>,
    Query(provider): Query<ProviderQuery>,
    State(db): State<Db>,
) -> Result<Json<ShowMetadata>, AppError> {
    let provider = provider.provider.to_string();
    let local_id = sqlx::query!(
        "SELECT show_id FROM external_ids WHERE metadata_id = ? AND metadata_provider = ?",
        id.id,
        provider
    )
    .fetch_one(&db.pool)
    .await?
    .show_id
    .ok_or(AppError::not_found("Show is not found locally"))?;

    Ok(Json(db.show(&local_id.to_string()).await?))
}

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

pub async fn contents_video(
    Path(id): Path<i64>,
    Query(content_type): Query<ContentTypeQuery>,
    State(state): State<AppState>,
) -> Result<Json<DetailedVideo>, StatusCode> {
    let video_id = match content_type.content_type {
        crate::metadata::ContentType::Movie => {
            sqlx::query!("SELECT video_id FROM movies WHERE id = ?", id)
                .fetch_one(&state.db.pool)
                .await
                .map(|x| x.video_id)
        }
        crate::metadata::ContentType::Show => {
            sqlx::query!("SELECT video_id FROM episodes WHERE id = ?", id)
                .fetch_one(&state.db.pool)
                .await
                .map(|x| x.video_id)
        }
    }
    .map_err(|_| StatusCode::NOT_FOUND)?;
    get_video_by_id(Path(video_id), State(state)).await
}

pub async fn get_all_variants(State(state): State<AppState>) -> Json<Vec<serde_json::Value>> {
    let (shows, movies): (Vec<_>, Vec<_>) = {
        let library = state.library.lock().unwrap();
        (
            library.shows.values().map(|x| x.source.clone()).collect(),
            library.movies.values().map(|x| x.source.clone()).collect(),
        )
    };
    let mut summary = Vec::new();
    let mut add_summary = |title: String, poster: Option<String>, video_id: i64, source: Source| {
        let variants: Vec<_> = source
            .variants
            .into_iter()
            .map(|x| DetailedVariant::from_video(x))
            .collect();
        let json = serde_json::json!({
        "title": title,
        "poster": poster,
        "video_id": video_id,
        "variants": variants
        });
        summary.push(json);
    };
    for show_source in shows {
        if show_source.variants.len() == 0 {
            continue;
        }
        let db_show = sqlx::query!(
            "SELECT episodes.title, episodes.poster FROM episodes
        JOIN videos ON videos.id = episodes.video_id
        WHERE videos.id = ?",
            show_source.id
        )
        .fetch_one(&state.db.pool)
        .await;
        if let Ok(db_show) = db_show {
            add_summary(db_show.title, db_show.poster, show_source.id, show_source);
        }
    }

    for movie_source in movies {
        if movie_source.variants.len() == 0 {
            continue;
        }
        let db_movie = sqlx::query!(
            "SELECT movies.title, movies.poster FROM movies
        JOIN videos ON videos.id = movies.video_id
        WHERE videos.id = ?",
            movie_source.id
        )
        .fetch_one(&state.db.pool)
        .await;
        if let Ok(db_movie) = db_movie {
            add_summary(
                db_movie.title,
                db_movie.poster,
                movie_source.id,
                movie_source,
            );
        }
    }
    Json(summary)
}

pub async fn get_video_by_id(
    Path(id): Path<i64>,
    State(state): State<AppState>,
) -> Result<Json<DetailedVideo>, StatusCode> {
    let AppState { library, db, .. } = state;
    let db_video = sqlx::query!(
        r#"SELECT videos.scan_date, videos.path, history.time,
        history.id, history.update_time, history.is_finished 
        FROM videos
        LEFT JOIN history ON history.video_id = videos.id
        WHERE videos.id = ?;"#,
        id
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;
    let source = {
        let library = library.lock().unwrap();
        let file = library.get_source(id).ok_or(StatusCode::NOT_FOUND)?;
        file.clone()
    };

    let detailed_variants = source
        .variants
        .iter()
        .map(|v| {
            let id = v.path().file_stem().expect("file stem to be id");
            DetailedVariant {
                id: id.to_string_lossy().to_string(),
                path: v.path().to_path_buf(),
                size: v.file_size(),
                duration: v.duration(),
                video_tracks: v
                    .video_streams()
                    .into_iter()
                    .map(|s| DetailedVideoTrack::from_video_stream(s, v.bitrate()))
                    .collect(),
                audio_tracks: v.audio_streams().into_iter().map(|s| s.into()).collect(),
            }
        })
        .collect();

    let date = db_video.scan_date.expect("scan date always defined");
    let history = if let (Some(id), Some(time), Some(is_finished), Some(update_time)) = (
        db_video.id,
        db_video.time,
        db_video.is_finished,
        db_video.update_time,
    ) {
        Some(DbHistory {
            id: Some(id),
            time,
            is_finished,
            update_time,
            video_id: db_video.id.unwrap(),
        })
    } else {
        None
    };
    let previews_dir = PreviewsDirAsset::new(id);
    let detailed_video = DetailedVideo {
        id,
        path: source.video.path().to_path_buf(),
        previews_count: previews_dir.previews_count(),
        size: source.video.file_size(),
        duration: source.video.duration(),
        variants: detailed_variants,
        scan_date: date.to_string(),
        video_tracks: source
            .video
            .video_streams()
            .into_iter()
            .map(|s| DetailedVideoTrack::from_video_stream(s, source.video.bitrate()))
            .collect(),
        audio_tracks: source
            .video
            .audio_streams()
            .into_iter()
            .map(|s| s.into())
            .collect(),
        subtitle_tracks: source
            .video
            .subtitle_streams()
            .into_iter()
            .map(|s| s.into())
            .collect(),
        history,
    };
    Ok(Json(detailed_video))
}

pub async fn get_show(
    State(providers): State<&'static MetadataProvidersStack>,
    Query(ProviderQuery { provider }): Query<ProviderQuery>,
    Path(id): Path<String>,
) -> Result<Json<ShowMetadata>, AppError> {
    let res = providers.get_show(&id, provider).await?;

    Ok(Json(res))
}

pub async fn get_season(
    State(providers): State<&'static MetadataProvidersStack>,
    Query(ProviderQuery { provider }): Query<ProviderQuery>,
    Path((show_id, season)): Path<(String, usize)>,
) -> Result<Json<SeasonMetadata>, AppError> {
    let res = providers.get_season(&show_id, season, provider).await?;
    Ok(Json(res))
}

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

// todo: pagination
pub async fn all_history(State(db): State<Db>) -> Result<Json<Vec<DbHistory>>, AppError> {
    let history = sqlx::query_as!(DbHistory, "SELECT * FROM history LIMIT 50;")
        .fetch_all(&db.pool)
        .await?;
    Ok(Json(history))
}
