use std::path::PathBuf;
use std::sync::Mutex;
use std::{convert::Infallible, fmt::Display};

use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, Query};
use axum::http::{Request, StatusCode};
use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use axum_extra::headers::ContentType;
use axum_extra::TypedHeader;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncSeekExt};
use tokio::sync::oneshot;
use tokio_stream::{Stream, StreamExt};
use tracing::{debug, info};
use uuid::Uuid;

use super::{IdQuery, StringIdQuery};
use crate::app_state::AppError;
use crate::config::ServerConfiguration;
use crate::library::TranscodePayload;
use crate::progress::{TaskKind, TaskResource};
use crate::{
    app_state::AppState,
    db::Db,
    progress::{ProgressChannel, ProgressChunk},
};

pub async fn reconciliate_lib(State(app_state): State<AppState>) -> Result<(), AppError> {
    app_state.reconciliate_library().await
}

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
            .ok_or(AppError::bad_request(format!("key {} not found", key)))
    }

    pub fn i64(&self, key: &str) -> Result<i64, AppError> {
        self.get_value(key)?
            .as_i64()
            .ok_or(AppError::bad_request("cant parse number"))
    }

    pub fn str(&self, key: &str) -> Result<&str, AppError> {
        self.get_value(key)?
            .as_str()
            .ok_or(AppError::bad_request("cant parse string"))
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

#[derive(Debug, Clone, Deserialize)]
pub struct TranscodeFilePayload {
    pub payload: TranscodePayload,
    pub video_id: i64,
}

#[test]
fn parse_transcode_payload() {
    use crate::library::{AudioCodec, VideoCodec};
    let json = serde_json::json!({
    "payload": {
        "audio_codec": "aac",
        "audio_track": 2,
        "video_codec": "hevc",
        "resolution": "1920x1080",
        },
    "video_id" : 23
    })
    .to_string();
    let payload: TranscodeFilePayload = serde_json::from_str(&json).unwrap();
    assert_eq!(payload.payload.audio_codec.unwrap(), AudioCodec::AAC);
    assert_eq!(payload.payload.video_codec.unwrap(), VideoCodec::Hevc);
    assert_eq!(payload.payload.resolution.unwrap(), (1920, 1080).into());
}

impl Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Tmdb => write!(f, "tmdb"),
        }
    }
}

fn sqlx_err_wrap(err: sqlx::Error) -> AppError {
    match err {
        sqlx::Error::RowNotFound => AppError::not_found("row not found"),
        _ => AppError::internal_error("unknown database error"),
    }
}

pub async fn remove_video(
    State(state): State<AppState>,
    Query(IdQuery { id }): Query<IdQuery>,
) -> Result<(), AppError> {
    state.remove_video(id).await
}

#[derive(Deserialize)]
pub struct RemoveVariantPayload {
    pub video_id: i64,
    pub variant_id: String,
}

pub async fn remove_variant(
    State(state): State<AppState>,
    Json(payload): Json<RemoveVariantPayload>,
) -> Result<(), AppError> {
    state
        .remove_variant(payload.video_id, &payload.variant_id)
        .await?;
    Ok(())
}

pub async fn alter_show_metadata(
    State(db): State<Db>,
    json: JsonExtractor,
) -> Result<(), AppError> {
    let show_id = json.i64("id")?;
    let title = json.str("title")?;
    let plot = json.str("plot")?;
    sqlx::query!(
        "UPDATE shows SET title = ?, plot = ? WHERE id = ?;",
        title,
        plot,
        show_id
    )
    .execute(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;
    Ok(())
}

pub async fn alter_season_metadata(
    State(db): State<Db>,
    json: JsonExtractor,
) -> Result<(), AppError> {
    let show_id = json.i64("id")?;
    let plot = json.str("plot")?;
    sqlx::query!("UPDATE seasons SET plot = ? WHERE id = ?;", plot, show_id)
        .execute(&db.pool)
        .await
        .map_err(sqlx_err_wrap)?;
    Ok(())
}

pub async fn alter_episode_metadata(
    State(db): State<Db>,
    json: JsonExtractor,
) -> Result<(), AppError> {
    let show_id = json.i64("id")?;
    let title = json.str("title")?;
    let plot = json.str("plot")?;
    sqlx::query!(
        "UPDATE episodes SET title = ?, plot = ? WHERE id = ?;",
        title,
        plot,
        show_id
    )
    .execute(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;
    Ok(())
}

pub async fn alter_movie_metadata(
    State(db): State<Db>,
    json: JsonExtractor,
) -> Result<(), AppError> {
    let movie_id = json.i64("id")?;
    let title = json.str("title")?;
    let plot = json.str("plot")?;
    sqlx::query!(
        "UPDATE movies SET title = ?, plot = ? WHERE id = ?;",
        title,
        plot,
        movie_id
    )
    .execute(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;
    Ok(())
}

pub async fn transcode_video(
    State(app_state): State<AppState>,
    Json(body): Json<TranscodeFilePayload>,
) {
    tokio::spawn(async move {
        let _ = app_state.transcode_video(body.video_id, body.payload).await;
    });
}

pub async fn generate_previews(
    State(app_state): State<AppState>,
    Query(IdQuery { id }): Query<IdQuery>,
) {
    tokio::spawn(async move {
        app_state.generate_previews(id).await.unwrap();
    });
}

#[derive(Debug, Clone, Deserialize)]
pub struct CancelTaskPayload {
    pub task_id: Uuid,
}

pub async fn cancel_task(
    State(tasks): State<TaskResource>,
    Json(CancelTaskPayload { task_id }): Json<CancelTaskPayload>,
) -> Result<(), StatusCode> {
    tasks
        .cancel_task(task_id)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    Ok(())
}

pub async fn mock_progress(
    State(tasks): State<TaskResource>,
    Query(StringIdQuery { id: target }): Query<StringIdQuery>,
) {
    debug!("Emitting fake progress with target: {}", target);
    let (tx, rx) = oneshot::channel();
    let task_id = tasks
        .start_task(PathBuf::from(target), TaskKind::Scan, Some(tx))
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
            _ = rx => {
                tasks.cancel_task(task_id).expect("task to be canceleable");
                debug!("Canceled fake progress with id: {}", task_id);
            }
        }
    });
}

pub async fn get_tasks(State(tasks): State<TaskResource>) -> String {
    let tasks = tasks.tasks.lock().unwrap();
    serde_json::to_string(&*tasks).unwrap()
}

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

pub async fn latest_log() -> Result<(TypedHeader<ContentType>, String), AppError> {
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
    Ok((TypedHeader(ContentType::json()), json))
}

pub async fn server_configuration(
    State(configuration): State<&'static Mutex<ServerConfiguration>>,
) -> Json<ServerConfiguration> {
    let configuration = configuration.lock().unwrap();
    Json(configuration.clone())
}
