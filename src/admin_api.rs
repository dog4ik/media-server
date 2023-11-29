use std::path::PathBuf;
use std::{convert::Infallible, fmt::Display};

use axum::extract::Query;
use axum::http::StatusCode;
use axum::{
    extract::State,
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use serde::Deserialize;
use tokio::sync::oneshot;
use tokio_stream::{Stream, StreamExt};
use tracing::{debug, info};
use uuid::Uuid;

use crate::metadata_provider::ShowMetadataProvider;
use crate::process_file::{AudioCodec, VideoCodec};
use crate::progress::{TaskKind, TaskResource};
use crate::public_api::{IdQuery, StringIdQuery};
use crate::{
    app_state::AppState,
    db::Db,
    progress::{ProgressChannel, ProgressChunk},
    tmdb_api::TmdbApi,
};

pub async fn reconciliate_lib(State(app_state): State<AppState>) -> String {
    let tmdb_api = TmdbApi::new(std::env::var("TMDB_TOKEN").expect("tmdb token to be in env"));
    let mut library = app_state.library.lock().await;

    library
        .reconciliate_library(&app_state.db, tmdb_api)
        .await
        .unwrap();
    "Done".into()
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
    pub video_codec: Option<VideoCodec>,
    pub audio_codec: Option<AudioCodec>,
    pub audio_track: Option<usize>,
    pub video_id: i64,
}

impl Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Provider::Tmdb => write!(f, "tmdb"),
        }
    }
}

fn sqlx_err_wrap(err: sqlx::Error) -> StatusCode {
    match err {
        sqlx::Error::RowNotFound => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[axum::debug_handler]
pub async fn remove_video(
    State(state): State<AppState>,
    Query(IdQuery { id }): Query<IdQuery>,
) -> Result<(), StatusCode> {
    state
        .remove_video(id)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)
}

pub async fn refresh_show_metadata(
    State(db): State<Db>,
    Json(payload): Json<RefreshShowMetadataPayload>,
) -> Result<String, StatusCode> {
    let tmdb_api = TmdbApi::new(std::env::var("TMDB_TOKEN").unwrap());

    let metadata_provider = payload.metadata_provider.unwrap_or_default();
    let metadata_provider_name = metadata_provider.to_string();
    let show = sqlx::query!(
        "SELECT * FROM shows WHERE id = ? AND metadata_provider = ?",
        payload.show_id,
        metadata_provider_name
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    match metadata_provider {
        Provider::Tmdb => {
            let metadata = tmdb_api.show(&show.title).await.unwrap();
            db.update_show_metadata(show.id, metadata)
                .await
                .map_err(sqlx_err_wrap)?;
        }
    };

    Ok("Done".into())
}

pub async fn trascode_video(
    State(app_state): State<AppState>,
    Json(payload): Json<TranscodeFilePayload>,
) -> Result<(), StatusCode> {
    tokio::spawn(async move {
        app_state
            .transcode_video(payload.video_id, payload.video_codec, payload.audio_codec)
            .await
            .unwrap();
    });

    Ok(())
}

pub async fn generate_previews(
    State(app_state): State<AppState>,
    Query(IdQuery { id }): Query<IdQuery>,
) -> Result<(), StatusCode> {
    tokio::spawn(async move {
        app_state.generate_previews(id).await.unwrap();
    });
    Ok(())
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
        .await
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
        .add_new_task(PathBuf::from(target), TaskKind::Scan, Some(tx))
        .await
        .unwrap();
    let ProgressChannel(channel) = &tasks.progress_channel;
    let channel = channel.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = async {
                let mut progress = 0;
                let _ = channel.send(ProgressChunk::start(task_id));
                while progress <= 100 {
                    let _ = channel.send(ProgressChunk::pending(task_id, progress));
                    progress += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                tasks.remove_task(task_id).await;
                let _ = channel.send(ProgressChunk::finish(task_id));
                debug!("finished fake progress with id: {}", task_id);
            }=> {},
            _ = rx => {
                tasks.remove_task(task_id).await;
                let _ = channel.send(ProgressChunk::cancel(task_id));
                debug!("Canceled fake progress with id: {}", task_id);
            }
        }
    });
}

pub async fn get_tasks(State(tasks): State<TaskResource>) -> String {
    let tasks = tasks.tasks.lock().await;
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
