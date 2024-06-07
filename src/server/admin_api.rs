use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;
use std::{convert::Infallible, fmt::Display};

use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Path, Query};
use axum::http::{Request, StatusCode};
use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use axum_extra::{headers, TypedHeader};
use serde::Deserialize;
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::sync::oneshot;
use tokio_stream::{Stream, StreamExt};
use tracing::{debug, info};
use uuid::Uuid;

use super::StringIdQuery;
use crate::app_state::AppError;
use crate::config::{FileConfigSchema, ServerConfiguration, APP_RESOURCES};
use crate::library::assets::{AssetDir, PreviewsDirAsset};
use crate::library::TranscodePayload;
use crate::metadata::{
    ContentType, EpisodeMetadata, MetadataProvider, MetadataProvidersStack, MovieMetadata,
    SeasonMetadata, ShowMetadata,
};
use crate::progress::{Task, TaskKind, TaskResource};
use crate::stream::transcode_stream::TranscodeStream;
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
    )
)]
pub async fn reconciliate_lib(State(app_state): State<AppState>) -> Result<(), AppError> {
    app_state.reconciliate_library().await
}

/// Clear the database. For debug purposes only.
#[utoipa::path(
    delete,
    path = "/api/clear_db",
    responses(
        (status = 200, body = String),
    )
)]
pub async fn clear_db(State(app_state): State<AppState>) -> Result<String, StatusCode> {
    info!("Clearing database");
    app_state
        .db
        .clear()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok("done".into())
}

pub struct JsonExtractor(pub serde_json::Map<String, serde_json::Value>);

impl JsonExtractor {
    fn get_value(&self, key: &str) -> Result<&serde_json::Value, AppError> {
        self.0
            .get(key)
            .ok_or(AppError::bad_request(format!("key {} is not found", key)))
    }

    pub fn i64(&self, key: &str) -> Result<i64, AppError> {
        self.get_value(key)?
            .as_i64()
            .ok_or(AppError::bad_request("can't parse number"))
    }

    pub fn str(&self, key: &str) -> Result<&str, AppError> {
        self.get_value(key)?
            .as_str()
            .ok_or(AppError::bad_request("can't parse string"))
    }
}

#[axum::async_trait]
impl<S: Send + Sync> FromRequest<S> for JsonExtractor {
    type Rejection = JsonRejection;

    async fn from_request(req: Request<Body>, state: &S) -> Result<Self, Self::Rejection> {
        let Json(json): axum::Json<serde_json::Map<String, serde_json::Value>> =
            Json::from_request(req, state).await?;
        Ok(JsonExtractor(json))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RefreshShowMetadataPayload {
    pub metadata_provider: Option<Provider>,
    pub show_id: i32,
    pub season: Option<i32>,
    pub episode: Option<i32>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RefreshMovieMetadataPayload {
    pub movie_id: usize,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    #[default]
    Tmdb,
}

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

impl Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Tmdb => write!(f, "tmdb"),
        }
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
    )
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
    )
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
    )
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
    )
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
    )
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
    )
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
    )
)]
pub async fn transcode_video(
    State(app_state): State<AppState>,
    Path(id): Path<i64>,
    Json(payload): Json<TranscodePayload>,
) {
    tokio::spawn(async move {
        let _ = app_state.transcode_video(id, payload).await;
    });
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
    )
)]
pub async fn generate_previews(State(app_state): State<AppState>, Path(id): Path<i64>) {
    tokio::spawn(async move {
        let _ = app_state.generate_previews(id).await;
    });
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
    )
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
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200),
        (status = 400, description = "Task can't be canceled or it is not found"),
    )
)]
pub async fn cancel_task(
    State(tasks): State<TaskResource>,
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
    )
)]
pub async fn mock_progress(
    State(tasks): State<TaskResource>,
    Query(StringIdQuery { id: target }): Query<StringIdQuery>,
) {
    debug!("Emitting fake progress with target: {}", target);
    let child_token = tasks.parent_cancellation_token.child_token();
    let task_id = tasks
        .start_task(
            TaskKind::Scan {
                target: target.into(),
            },
            Some(child_token.clone()),
        )
        .unwrap();
    let ProgressChannel(channel) = &tasks.progress_channel;
    let channel = channel.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = async {
                let mut progress = 0;
                while progress <= 100 {
                    let _ = channel.send(ProgressChunk::pending(task_id, progress));
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
    )
)]
pub async fn get_tasks(State(tasks): State<TaskResource>) -> Json<Vec<Task>> {
    let tasks = tasks.tasks.lock().unwrap().to_vec();
    Json(tasks)
}

/// SSE stream of current tasks progress
#[utoipa::path(
    get,
    path = "/api/tasks/progress",
    responses(
        (status = 200, body = [u8]),
    )
)]
pub async fn progress(
    State(tasks): State<TaskResource>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let ProgressChannel(channel) = tasks.progress_channel;
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
        (status = 200, body = String, content_type = "application/json"),
    )
)]
pub async fn latest_log() -> Result<(TypedHeader<headers::ContentType>, String), AppError> {
    use tokio::fs;
    use tokio::io;
    let file = fs::File::open("log.log").await?;
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

/// Server configuartion
#[utoipa::path(
    get,
    path = "/api/configuration",
    responses(
        (status = 200, body = ServerConfiguration),
    )
)]
pub async fn server_configuration(
    State(configuration): State<&'static Mutex<ServerConfiguration>>,
) -> Json<ServerConfiguration> {
    let configuration = configuration.lock().unwrap();
    Json(configuration.clone())
}

/// Current server configuartion schema
#[utoipa::path(
    get,
    path = "/api/configuration/schema",
    responses(
        (status = 200, body = FileConfigSchema),
    )
)]
pub async fn server_configuration_schema(
    State(configuration): State<&'static Mutex<ServerConfiguration>>,
) -> Json<FileConfigSchema> {
    let configuration = configuration.lock().unwrap();
    Json(configuration.into_schema())
}

/// Update server configuartion
#[utoipa::path(
    put,
    path = "/api/configuration",
    request_body = FileConfigSchema,
    responses(
        (status = 200, body = ServerConfiguration, description = "Updated server configuration"),
    )
)]
pub async fn update_server_configuration(
    State(configuration): State<&'static Mutex<ServerConfiguration>>,
    Json(new_config): Json<FileConfigSchema>,
) -> Json<ServerConfiguration> {
    let mut configuration = configuration.lock().unwrap();
    configuration.apply_config_schema(new_config);
    configuration.flush().unwrap();
    Json(configuration.clone())
}

/// Reset server configuration to its defauts
#[utoipa::path(
    post,
    path = "/api/configuration/reset",
    responses(
        (status = 200, body = ServerConfiguration, description = "Updated server configuration"),
    )
)]
pub async fn reset_server_configuration(
    State(configuration): State<&'static Mutex<ServerConfiguration>>,
) -> Json<ServerConfiguration> {
    let mut configuration = configuration.lock().unwrap();
    configuration.apply_config_schema(FileConfigSchema::default());
    configuration.flush().unwrap();
    Json(configuration.clone())
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TorrentDownloadHint {
    content_type: ContentType,
    metadata_provider: MetadataProvider,
    metadata_id: String,
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TorrentDownloadPayload {
    save_location: Option<String>,
    content_hint: Option<TorrentDownloadHint>,
    magnet: String,
}

/// Start torrent download
#[utoipa::path(
    post,
    path = "/api/torrent/download",
    request_body = TorrentDownloadPayload,
    responses(
        (status = 200),
    )
)]
pub async fn download_torrent(
    State(app_state): State<AppState>,
    Json(payload): Json<TorrentDownloadPayload>,
) -> Result<(), AppError> {
    use torrent::Torrent;
    let torrent = Torrent::from_mangnet_link(&payload.magnet).await?;
    let default_path = APP_RESOURCES.get().unwrap().resources_path.join("torrents");
    let save_location = payload
        .save_location
        .map(|l| PathBuf::from(l))
        .unwrap_or(default_path);
    tokio::spawn(async move {
        let _ = app_state.download_torrent(torrent, save_location).await;
    });
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
    )
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
    )
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

/// Update/Insert history
#[utoipa::path(
    put,
    path = "/api/history/{id}",
    params(
        ("id", description = "Video id"),
    ),
    request_body = UpdateHistoryPayload,
    responses(
        (status = 200),
        (status = 201),
    )
)]
pub async fn update_history(
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
                db.insert_history(crate::db::DbHistory {
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

/// Delete all history for default user
#[utoipa::path(
    delete,
    path = "/api/history",
    responses(
        (status = 200),
    )
)]
pub async fn clear_history(State(db): State<Db>) -> Result<(), AppError> {
    sqlx::query!("DELETE FROM history")
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Delete history for specific video
#[utoipa::path(
    delete,
    path = "/api/history/{id}",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200),
    )
)]
pub async fn remove_history_item(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    sqlx::query!("DELETE FROM history WHERE video_id = ?;", id)
        .execute(&db.pool)
        .await?;
    Ok(())
}

/// Recieve transcoded segment
#[utoipa::path(
    delete,
    path = "/api/transcode/{id}/segment/{segment}",
    params(
        ("id", description = "Transcode job"),
        ("segment", description = "Desired segment"),
    ),
    responses(
        (status = 200),
        (status = 404, description = "Transcode job is not found"),
        (status = 500, description = "Worker is not avialable"),
    )
)]
pub async fn transcoded_segment(
    Path((task_id, index)): Path<(String, usize)>,
    State(tasks): State<TaskResource>,
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
    sender.send((index, tx)).await.unwrap();
    if let Ok(bytes) = rx.await {
        Ok(bytes)
    } else {
        Err(AppError::internal_error(
            "Transcode worker is not avaiblable",
        ))
    }
}

/// Start transcoded stream
#[utoipa::path(
    delete,
    path = "/api/video/:id/stream_transcode",
    params(
        ("id", description = "Video id"),
    ),
    responses(
        (status = 200),
        (status = 404, description = "Video is not found"),
    )
)]
pub async fn create_transcode_stream(
    Path(id): Path<i64>,
    State(app_state): State<AppState>,
) -> Result<(), AppError> {
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
    let stream = TranscodeStream::init(id, video_path, tracker, cancellation_token).await?;
    let mut streams = tasks.active_streams.lock().unwrap();
    streams.push(stream);
    Ok(())
}

/// M3U8 manifest of live transcode task
#[utoipa::path(
    delete,
    path = "/api/transcode/:id/manifest",
    params(
        ("id", description = "Task id"),
    ),
    responses(
        (status = 200),
        (status = 400, description = "Task uuid is incorrect"),
        (status = 404, description = "Task is not found"),
    )
)]
pub async fn transcode_stream_manifest(
    Path(stream_id): Path<String>,
    State(tasks): State<TaskResource>,
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
