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
use crate::db::{DbExternalId, DbHistory};
use crate::ffmpeg::{FFprobeAudioStream, FFprobeSubtitleStream, FFprobeVideoStream};
use crate::library::assets::{
    BackdropAsset, BackdropContentType, FileAsset, PosterAsset, PosterContentType, PreviewAsset,
    PreviewsDirAsset, VariantAsset,
};
use crate::library::{AudioCodec, Resolution, Source, SubtitlesCodec, VideoCodec};
use crate::metadata::{
    EpisodeMetadata, ExternalIdMetadata, MetadataProvider, MetadataProvidersStack,
    MetadataSearchResult, MovieMetadata, SeasonMetadata, ShowMetadata,
};
use crate::torrent_index::Torrent;
use crate::{app_state::AppState, db::Db};

use super::{
    ContentTypeQuery, IdQuery, NumberQuery, PageQuery, ProviderQuery, SearchQuery, VariantQuery,
};

#[derive(Debug, Deserialize, Clone)]
pub struct ContentRequestQuery {
    origin: Option<MetadataProvider>,
    id: String,
}

#[derive(Debug, Serialize, FromRow, utoipa::ToSchema)]
pub struct DetailedVideo {
    pub id: i64,
    #[schema(value_type = String)]
    pub path: PathBuf,
    pub previews_count: usize,
    pub size: u64,
    #[schema(value_type = SerdeDuration)]
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
    pub subtitle_tracks: Vec<DetailedSubtitleTrack>,
    pub variants: Vec<DetailedVariant>,
    pub scan_date: String,
    pub history: Option<DbHistory>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedVariant {
    pub id: String,
    #[schema(value_type = String)]
    pub path: PathBuf,
    pub size: u64,
    #[schema(value_type = SerdeDuration)]
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

/// Get preview by video id
#[utoipa::path(
    get,
    path = "/api/video/{id}/preview",
    params(
        ("id", description = "video id"),
        NumberQuery,
    ),
    responses(
        (status = 200, description = "Binary image", body = [u8]),
        (status = 304),
        (status = 404, description = "Preiew is not found", body = AppError),
    )
)]
pub async fn previews(
    Path(video_id): Path<i64>,
    Query(number): Query<NumberQuery>,
    is_modified_sience: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let preview_asset = PreviewAsset::new(video_id, number.number);
    let response = preview_asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_sience)
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
    )
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
        (status = 200, description = "Video stream", body = [u8]),
        (status = 404, description = "Video is not found", body = AppError),
    )
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
        return Ok(video.serve(range).await);
    } else {
        let AppState { library, .. } = state;
        let video = {
            let library = library.lock().unwrap();
            library
                .get_source(video_id)
                .map(|x| x.video.clone())
                .ok_or(AppError::not_found("Video not found"))?
        };
        return Ok(video.serve(range).await);
    }
}

#[utoipa::path(
    get,
    path = "/api/local_shows",
    responses(
        (status = 200, description = "All local shows", body = Vec<ShowMetadata>),
    )
)]
pub async fn all_local_shows(
    Query(q): Query<PageQuery>,
    State(db): State<Db>,
) -> Result<Json<Vec<ShowMetadata>>, AppError> {
    const PAGE_SIZE: i32 = 20;
    let page = (q.page.unwrap_or(1) - 1).max(0) as i32;
    let offset = page * PAGE_SIZE;
    Ok(Json(db.all_shows().await?))
}

#[utoipa::path(
    get,
    path = "/api/local_movies",
    responses(
        (status = 200, description = "All local movies", body = Vec<MovieMetadata>),
    )
)]
pub async fn all_local_movies(
    Query(q): Query<PageQuery>,
    State(db): State<Db>,
) -> Result<Json<Vec<MovieMetadata>>, AppError> {
    const PAGE_SIZE: i32 = 20;
    let page = (q.page.unwrap_or(1) - 1).max(0) as i32;
    let offset = page * PAGE_SIZE;
    Ok(Json(db.all_movies().await?))
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
    )
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
    )
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
    )
)]
pub async fn contents_video(
    Query(IdQuery { id }): Query<IdQuery>,
    Query(content_type): Query<ContentTypeQuery>,
    State(state): State<AppState>,
) -> Result<Json<DetailedVideo>, AppError> {
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
    }?;
    get_video_by_id(Path(video_id), State(state)).await
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct VariantSummary {
    pub title: String,
    pub poster: Option<String>,
    pub video_id: i64,
    pub variants: Vec<DetailedVariant>,
}

/// Get all variants in the library
#[utoipa::path(
    get,
    path = "/api/variants",
    responses(
        (status = 200, description = "All variants", body = Vec<VariantSummary>),
    )
)]
pub async fn get_all_variants(State(state): State<AppState>) -> Json<Vec<VariantSummary>> {
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
        summary.push(VariantSummary {
            title,
            poster,
            video_id,
            variants,
        });
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

/// Get video by id
#[utoipa::path(
    get,
    path = "/api/video/{id}",
    params(
        ("id", description = "Video id")
    ),
    responses(
        (status = 200, description = "Requested video", body = DetailedVideo),
    )
)]
pub async fn get_video_by_id(
    Path(id): Path<i64>,
    State(state): State<AppState>,
) -> Result<Json<DetailedVideo>, AppError> {
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
    .await?;
    let source = {
        let library = library.lock().unwrap();
        let file = library
            .get_source(id)
            .ok_or(AppError::not_found("Library video is not found"))?;
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
    )
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
    )
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
        (status = 200, description = "Poster bytes", body = [u8]),
        (status = 304),
        (status = 404, body = AppError)
    )
)]
pub async fn show_poster(
    Path(id): Path<i64>,
    is_modified_sience: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = PosterAsset::new(id, PosterContentType::Show);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_sience)
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
        (status = 200, description = "Poster bytes", body = [u8]),
        (status = 304),
        (status = 404, body = AppError)
    )
)]
pub async fn season_poster(
    Path(id): Path<i64>,
    is_modified_sience: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = PosterAsset::new(id, PosterContentType::Season);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_sience)
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
        (status = 200, description = "Response with image", body = [u8]),
        (status = 304),
        (status = 404, description = "Image not found", body = AppError)
    )
)]
pub async fn show_backdrop(
    Path(id): Path<i64>,
    is_modified_sience: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = BackdropAsset::new(id, BackdropContentType::Show);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_sience)
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
        (status = 200, description = "Poster bytes", body = [u8]),
        (status = 304),
        (status = 404, body = AppError)
    )
)]
pub async fn movie_poster(
    Path(id): Path<i64>,
    is_modified_sience: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = PosterAsset::new(id, PosterContentType::Movie);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_sience)
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
        (status = 200, description = "Backdrop bytes", body = [u8]),
        (status = 304),
        (status = 404, body = AppError)
    )
)]
pub async fn movie_backdrop(
    Path(id): Path<i64>,
    is_modified_sience: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = BackdropAsset::new(id, BackdropContentType::Movie);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_sience)
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
        (status = 200, description = "Poster bytes", body = [u8]),
        (status = 304),
        (status = 404, body = AppError)
    )
)]
pub async fn episode_poster(
    Path(id): Path<i64>,
    is_modified_sience: Option<TypedHeader<axum_extra::headers::IfModifiedSince>>,
) -> Result<impl IntoResponse, AppError> {
    let asset = PosterAsset::new(id, PosterContentType::Episode);
    let response = asset
        .into_response(axum_extra::headers::ContentType::jpeg(), is_modified_sience)
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
    )
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
    )
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
    )
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

/// Search for content. Allows to search for all types of content at once
#[utoipa::path(
    get,
    path = "/api/search/content",
    params(
        SearchQuery,
    ),
    responses(
        (status = 200, description = "Content search results", body = Vec<MetadataSearchResult>),
    )
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
    )
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

/// Get all watch history of the default user. Have hard coded limit of 50 rows for now.
#[utoipa::path(
    get,
    path = "/api/history",
    responses(
        (status = 200, description = "All history", body = Vec<DbHistory>),
    )
)]
pub async fn all_history(State(db): State<Db>) -> Result<Json<Vec<DbHistory>>, AppError> {
    // todo: pagination
    let history = sqlx::query_as!(DbHistory, "SELECT * FROM history LIMIT 50;")
        .fetch_all(&db.pool)
        .await?;
    Ok(Json(history))
}
