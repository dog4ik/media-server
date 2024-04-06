use std::path::PathBuf;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::headers::Range;
use axum_extra::TypedHeader;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

use crate::app_state::AppError;
use crate::db::DbExternalId;
use crate::ffmpeg::{FFprobeAudioStream, FFprobeVideoStream};
use crate::library::{AudioCodec, Resolution, Source, Summary, VideoCodec};
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
    pub resources_folder: String,
    pub size: u64,
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
    pub variants: Vec<DetailedVariant>,
    pub scan_date: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DetailedVariant {
    pub id: String,
    pub path: PathBuf,
    pub size: u64,
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DetailedAudioTrack {
    pub is_default: bool,
    pub sample_rate: String,
    pub channels: i32,
    pub profile: Option<String>,
    pub codec: AudioCodec,
}

#[derive(Debug, Clone, FromRow, Serialize)]
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

impl DetailedVariant {
    pub fn from_video(video: crate::library::Video) -> Self {
        let id = video
            .path
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
            path: video.path,
        }
    }
}

pub async fn previews(
    Query(video_id): Query<IdQuery>,
    Query(number): Query<NumberQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let AppState { library, db, .. } = state;
    let video = sqlx::query!("SELECT * FROM videos WHERE id = ?", video_id.id)
        .fetch_one(&db.pool)
        .await
        .map_err(sqlx_err_wrap)?;
    let video_path = PathBuf::from(video.path);
    let file = {
        let library = library.lock().unwrap();
        library.find_source(&video_path).cloned()
    };
    if let Some(file) = file {
        return Ok(file.serve_previews(number.number).await);
    } else {
        return Err(StatusCode::NOT_FOUND);
    };
}

pub async fn subtitles(
    Query(video_id): Query<IdQuery>,
    Query(lang): Query<LanguageQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let AppState { library, db, .. } = state;
    let video = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id.id)
        .fetch_one(&db.pool)
        .await
        .map_err(sqlx_err_wrap)?;
    let video_path = PathBuf::from(video.path);
    let file = {
        let library = library.lock().unwrap();
        library.find_source(&video_path).cloned()
    };
    if let Some(file) = file {
        return Ok(file.serve_subs(lang.lang).await);
    } else {
        return Err(StatusCode::NOT_FOUND);
    };
}

pub async fn watch(
    Query(video_id): Query<IdQuery>,
    variant: Option<Query<VariantQuery>>,
    State(state): State<AppState>,
    range: Option<TypedHeader<Range>>,
) -> Result<impl IntoResponse, StatusCode> {
    let AppState { library, db, .. } = state;
    let video = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id.id)
        .fetch_one(&db.pool)
        .await
        .map_err(sqlx_err_wrap)?;
    let path = PathBuf::from(video.path);
    let file = {
        let library = library.lock().unwrap();
        library.find_source(&path).map(|x| x.clone())
    }
    .ok_or(StatusCode::NOT_FOUND)?;
    if let Some(Query(VariantQuery { variant })) = variant {
        let file = file.find_variant(&variant).ok_or(StatusCode::NOT_FOUND)?;
        return Ok(file.serve(range).await);
    }
    return Ok(file.origin.serve(range).await);
}

pub async fn get_summary(State(state): State<AppState>) -> Json<Vec<Summary>> {
    let library = state.library.lock().unwrap();
    return Json(library.get_summary());
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
    let (shows, movies): (Vec<Source>, Vec<Source>) = {
        let library = state.library.lock().unwrap();
        (
            library.shows.iter().map(|x| x.source.clone()).collect(),
            library.movies.iter().map(|x| x.source.clone()).collect(),
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
        let path = show_source.source_path().to_string_lossy().to_string();
        if show_source.variants.len() == 0 {
            continue;
        }
        let db_show = sqlx::query!(
            "SELECT episodes.* FROM episodes
        JOIN videos ON videos.id = episodes.video_id
        WHERE videos.path = ?",
            path
        )
        .fetch_one(&state.db.pool)
        .await;
        if let Ok(db_show) = db_show {
            add_summary(db_show.title, db_show.poster, db_show.video_id, show_source);
        }
    }

    for movie_source in movies {
        let path = movie_source.source_path().to_string_lossy().to_string();
        if movie_source.variants.len() == 0 {
            continue;
        }
        let db_movie = sqlx::query!(
            "SELECT movies.* FROM movies
        JOIN videos ON videos.id = movies.video_id
        WHERE videos.path = ?",
            path
        )
        .fetch_one(&state.db.pool)
        .await;
        if let Ok(db_movie) = db_movie {
            add_summary(
                db_movie.title,
                db_movie.poster,
                db_movie.video_id,
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
        "SELECT scan_date, path, resources_folder FROM videos WHERE id = ?",
        id
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;
    let source = {
        let library = library.lock().unwrap();
        let file = library
            .find_source(db_video.path)
            .ok_or(StatusCode::NOT_FOUND)?;
        file.clone()
    };

    let detailed_variants = source
        .variants
        .iter()
        .map(|v| {
            let id = v
                .path
                .file_stem()
                .expect("file to have stem like {size}.{hash}");
            DetailedVariant {
                id: id.to_string_lossy().to_string(),
                path: v.path.clone(),
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
    let detailed_video = DetailedVideo {
        id,
        path: source.source_path().to_path_buf(),
        resources_folder: source.resources_folder_name(),
        size: source.origin.file_size(),
        duration: source.duration(),
        variants: detailed_variants,
        scan_date: date.to_string(),
        video_tracks: source
            .origin
            .video_streams()
            .into_iter()
            .map(|s| DetailedVideoTrack::from_video_stream(s, source.origin.bitrate()))
            .collect(),
        audio_tracks: source
            .origin
            .audio_streams()
            .into_iter()
            .map(|s| s.into())
            .collect(),
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

/// Search torrent by content's imdb id
pub async fn search_torrent(
    Query(query): Query<StringIdQuery>,
    State(providers): State<&'static MetadataProvidersStack>,
) -> Result<Json<Vec<Torrent>>, AppError> {
    let out = providers.get_torrents(&query.id).await;
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
