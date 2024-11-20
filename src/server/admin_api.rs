use std::convert::Infallible;
use std::fmt::Display;
use std::path::PathBuf;
use std::str::FromStr;

use axum::extract::{Multipart, Path, Query};
use axum::http::StatusCode;
use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use axum_extra::{headers, TypedHeader};
use base64::Engine;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::sync::oneshot;
use tokio_stream::{Stream, StreamExt};
use torrent::{MagnetLink, TorrentFile};
use tracing::{debug, info};
use uuid::Uuid;

use super::{ContentTypeQuery, OptionalContentTypeQuery, ProviderQuery, StringIdQuery};
use crate::app_state::AppError;
use crate::config::{
    self, Capabilities, ConfigurationApplyResult, SerializedSetting, APP_RESOURCES,
};
use crate::db::{DbActions, DbEpisodeIntro};
use crate::file_browser::{BrowseDirectory, BrowseFile, BrowseRootDirs, FileKey};
use crate::library::assets::{AssetDir, PreviewsDirAsset};
use crate::library::TranscodePayload;
use crate::metadata::{
    ContentType, EpisodeMetadata, MetadataProvidersStack, MovieMetadata, SeasonMetadata,
    ShowMetadata,
};
use crate::progress::{Task, TaskKind, TaskResource, VideoTaskType};
use crate::stream::transcode_stream::TranscodeStream;
use crate::torrent::{
    DownloadContentHint, ResolveMagnetLinkPayload, TorrentClient, TorrentDownloadPayload,
    TorrentInfo,
};
use crate::{
    app_state::AppState,
    db::Db,
    progress::{ProgressChannel, ProgressChunk},
};

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
    let task_id = app_state.tasks.start_task(TaskKind::Scan, None)?;
    tokio::spawn(async move {
        match app_state.reconciliate_library().await {
            Ok(_) => {
                app_state.tasks.finish_task(task_id);
            }
            Err(err) => {
                tracing::error!("Library reconcilliation task failed: {err}");
                app_state.tasks.error_task(task_id);
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
    info!("Clearing database");
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
        JOIN videos ON movies.video_id = videos.id
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
    app_state.fix_show_metadata(show_id, show).await
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
            app_state.fix_show_metadata(content_id, show).await
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
    path = "/api/tasks/{id}",
    params(
        ("id", description = "Task id"),
    ),
    responses(
        (status = 200),
        (status = 400, description = "Task can't be canceled or it is not found"),
    ),
    tag = "Tasks",
)]
pub async fn cancel_task(
    State(tasks): State<&'static TaskResource>,
    Path(task_id): Path<Uuid>,
) -> Result<(), StatusCode> {
    tasks
        .cancel_task(task_id)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(())
}

/// Create fake task and progress. For debug purposes only
#[utoipa::path(
    post,
    path = "/api/mock_progress",
    params(
        StringIdQuery,
    ),
    responses(
        (status = 200),
    ),
    tag = "Tasks",
)]
pub async fn mock_progress(
    State(tasks): State<&'static TaskResource>,
    Query(StringIdQuery { id: target }): Query<StringIdQuery>,
) {
    debug!("Emitting fake progress with target: {}", target);
    let child_token = tasks.parent_cancellation_token.child_token();
    let task_id = tasks
        .start_task(TaskKind::Scan, Some(child_token.clone()))
        .unwrap();
    let ProgressChannel(channel) = &tasks.progress_channel;
    let channel = channel.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = async {
                let mut progress = 0;
                while progress <= 100 {
                    let _ = channel.send(ProgressChunk::pending(task_id, Some(progress as f32), None));
                    progress += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                tasks.finish_task(task_id);
                debug!("finished fake progress with id: {}", task_id);
            }=> {},
            _ = child_token.cancelled() => {
                tasks.cancel_task(task_id).expect("task to be canceleable");
                debug!("Canceled fake progress with id: {}", task_id);
            }
        }
    });
}

/// Get all running tasks
#[utoipa::path(
    get,
    path = "/api/tasks",
    responses(
        (status = 200, body = Vec<Task>),
        (status = 400, description = "Task can't be canceled or it is not found"),
    ),
    tag = "Tasks",
)]
pub async fn get_tasks(State(tasks): State<&'static TaskResource>) -> Json<Vec<Task>> {
    let tasks = tasks.tasks.lock().unwrap().to_vec();
    Json(tasks)
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

/// Parse .torrent file
#[utoipa::path(
    post,
    path = "/api/torrent/parse_torrent_file",
    params(
        OptionalContentTypeQuery,
    ),
    responses(
        (status = 200, body = TorrentInfo),
        (status = 400, description = "Failed to parse torrent file"),
    ),
    tag = "Torrent",
)]
pub async fn parse_torrent_file(
    State(providers_stack): State<&'static MetadataProvidersStack>,
    Query(hint): Query<Option<DownloadContentHint>>,
    mut multipart: Multipart,
) -> Result<Json<TorrentInfo>, AppError> {
    if let Ok(Some(field)) = multipart.next_field().await {
        let data = field.bytes().await.unwrap();
        let torrent_file =
            TorrentFile::from_bytes(data).map_err(|x| AppError::bad_request(x.to_string()))?;
        let torrent_info = TorrentInfo::new(&torrent_file.info, hint, providers_stack).await;
        return Ok(Json(torrent_info));
    }
    Err(AppError::bad_request("Failed to handle multipart request"))
}

/// Resolve magnet link
#[utoipa::path(
    get,
    path = "/api/torrent/resolve_magnet_link",
    params(
        ResolveMagnetLinkPayload,
        ("content_type" = Option<ContentType>, Query, description = "Content type"),
        ("metadata_provider" = Option<crate::metadata::MetadataProvider>, Query, description = "Metadata provider"),
        ("metadata_id" = Option<String>, Query, description = "Metadata id"),
    ),
    responses(
        (status = 200, body = TorrentInfo),
        (status = 400, description = "Failed to parse magnet link"),
    ),
    tag = "Torrent",
)]
pub async fn resolve_magnet_link(
    State(client): State<&'static TorrentClient>,
    State(providers_stack): State<&'static MetadataProvidersStack>,
    Query(payload): Query<ResolveMagnetLinkPayload>,
    hint: Option<Query<DownloadContentHint>>,
) -> Result<Json<TorrentInfo>, AppError> {
    let magnet_link = MagnetLink::from_str(&payload.magnet_link)
        .map_err(|_| AppError::bad_request("Failed to parse magnet link"))?;
    let info = client.resolve_magnet_link(&magnet_link).await?;
    let torrent_info = TorrentInfo::new(&info, hint.map(|x| x.0), providers_stack).await;
    Ok(Json(torrent_info))
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

/// Update providers order
#[utoipa::path(
    get,
    path = "/api/configuration/providers",
    responses(
        (status = 200, body = Vec<ProviderOrder>, description = "Ordering of providers"),
    ),
    tag = "Configuration",
)]
pub async fn providers_order(
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
        (status = 200, body = Task),
        (status = 404, description = "Video is not found"),
    ),
    tag = "Transcoding",
)]
pub async fn create_transcode_stream(
    Path(id): Path<i64>,
    State(app_state): State<AppState>,
) -> Result<Json<Task>, AppError> {
    let AppState { library, tasks, .. } = app_state;
    let video_path = {
        let library = library.lock().unwrap();
        let source = library
            .get_source(id)
            .ok_or(AppError::not_found("Requested video is not found"))?;
        source.video.path().to_path_buf()
    };
    let cancellation_token = tasks.parent_cancellation_token.child_token();
    let tracker = tasks.tracker.clone();
    let task_id = tasks.start_task(
        TaskKind::Video {
            video_id: id,
            task_type: VideoTaskType::LiveTranscode,
        },
        Some(cancellation_token.clone()),
    )?;
    let stream =
        TranscodeStream::init(id, video_path, task_id, tracker, cancellation_token).await?;
    {
        let mut streams = tasks.active_streams.lock().unwrap();
        streams.push(stream);
    }
    let task = {
        tasks
            .tasks
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == task_id)
            .expect("Task to be created")
            .clone()
    };
    Ok(Json(task))
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
        JOIN videos ON videos.id = episodes.video_id
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

/// Download torrent
#[utoipa::path(
    post,
    path = "/api/torrent/download",
    request_body = TorrentDownloadPayload,
    responses(
        (status = 200),
        (status = 400),
    ),
    tag = "Torrent",
)]
pub async fn download_torrent(
    State(app_state): State<AppState>,
    Json(payload): Json<TorrentDownloadPayload>,
) -> Result<(), AppError> {
    let AppState {
        providers_stack,
        torrent_client,
        tasks,
        ..
    } = app_state;
    let magnet_link = MagnetLink::from_str(&payload.magnet_link)
        .map_err(|_| AppError::bad_request("Failed to parse magnet link"))?;
    let tracker_list = magnet_link.all_trackers().ok_or(AppError::bad_request(
        "Magnet links without tracker list are not supported",
    ))?;
    let info = torrent_client
        .resolve_magnet_link(&magnet_link)
        .await
        .map_err(|e| AppError::bad_request(e.to_string()))?;
    let info_hash = info.hash();
    let mut torrent_info = TorrentInfo::new(&info, payload.content_hint, providers_stack).await;

    let enabled_files = payload
        .enabled_files
        .unwrap_or_else(|| (0..info.files_amount()).collect());
    for enabled_idx in &enabled_files {
        torrent_info.contents.files[*enabled_idx].enabled = true;
    }
    let save_location = payload
        .save_location
        .map(PathBuf::from)
        .or_else(|| {
            let content_type = torrent_info
                .contents
                .content
                .as_ref()
                .map(|c| c.content_type())?;
            let movie_folders: config::MovieFolders = config::CONFIG.get_value();
            let show_folders: config::ShowFolders = config::CONFIG.get_value();
            match content_type {
                ContentType::Show => show_folders.first(),
                ContentType::Movie => movie_folders.first(),
            }
            .map(|f| f.to_owned())
        })
        .ok_or(AppError::bad_request("Could not determine save location"))?;
    tracing::debug!("Selected torrent output: {}", save_location.display());
    let content = torrent_info.contents.content.clone();
    let handle = torrent_client
        .download(
            save_location,
            tracker_list,
            info,
            torrent_info,
            enabled_files,
        )
        .await?;

    tasks.tracker.spawn(async move {
        let _ = tasks
            .observe_task(handle, TaskKind::Torrent { info_hash, content })
            .await;
        torrent_client.remove_download(info_hash)
    });

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
