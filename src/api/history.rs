use std::time::Duration;

use axum::extract::State;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::{
    api::{
        CursorQuery, Json, OptionalUuidQuery, Path, Query, TakeQuery,
        api_data::{
            api_types::{Content, History},
            local_show::Episode,
        },
        server::CursoredResponse,
    },
    app_state::{AppError, AppState},
    db::{self, Db, DbActions, query_builders::DbHistoryQuery},
    metadata::{EpisodeMetadata, MovieMetadata},
    watch::WatchProgress,
};

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub struct HistoryEntry {
    #[serde(flatten)]
    pub content: Content,
    pub metadata_id: i64,
    pub runtime: crate::MediaDuration,
    #[serde(flatten)]
    pub history_content_type: HistoryContentType,
    pub history: History,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum HistoryContentType {
    Episode {
        show_id: i64,
        show_title: String,
        season_number: i64,
        number: i64,
        episode_id: i64,
    },
    Movie {
        movie_id: i64,
    },
}

impl From<DbHistoryQuery> for HistoryEntry {
    fn from(
        DbHistoryQuery {
            metadata,
            history,
            episode,
            show_id,
            season_number,
            show_title,
            movie,
            runtime,
        }: DbHistoryQuery,
    ) -> Self {
        let history_content_type = if episode.id.is_some() {
            HistoryContentType::Episode {
                show_id,
                show_title,
                season_number,
                number: episode.number,
                episode_id: episode.id.expect("episode id is not null"),
            }
        } else {
            let movie_id = movie
                .id
                .expect("movie id is not empty if history type is movie");
            HistoryContentType::Movie { movie_id }
        };
        Self {
            metadata_id: metadata.id.expect("metadata id is not null"),
            content: Content::from(metadata),
            runtime: Duration::from_secs(runtime as u64).into(),
            history_content_type,
            history: History::from(history),
        }
    }
}

/// Get all watch history of the default user. Limit defaults to 50 if not specified
#[utoipa::path(
    get,
    path = "/api/history",
    responses(
        (status = 200, description = "All history", body = CursoredResponse<HistoryEntry>),
    ),
    params(
        TakeQuery,
        CursorQuery,
    ),
    tag = "History",
)]
pub async fn all_history(
    Query(TakeQuery { take }): Query<TakeQuery>,
    Query(CursorQuery { cursor }): Query<CursorQuery>,
    State(db): State<Db>,
) -> Result<Json<CursoredResponse<HistoryEntry>>, AppError> {
    let take = take.unwrap_or(50);
    let cursor: Option<i64> = cursor
        .map(|x| {
            x.parse()
                .map_err(|_| AppError::bad_request("invalid cursor"))
        })
        .transpose()?;
    let mut builder = db::DbQueryBuilder::default();
    DbHistoryQuery::build(cursor, take, &mut builder);
    let history: Vec<HistoryEntry> = builder
        .build_query_as::<DbHistoryQuery>()
        .fetch_all(&db.pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

    let cursor = history
        .last()
        .map(|x| x.history.update_time.0.unix_timestamp());
    Ok(Json(CursoredResponse::new(history, cursor)))
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct MovieHistory {
    pub movie: MovieMetadata,
    pub history: History,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ShowHistory {
    pub show_id: i64,
    pub episode: EpisodeMetadata,
    pub history: History,
}
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ShowSuggestion {
    pub show_id: i64,
    pub episode: Episode,
    pub history: Option<History>,
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
        history.metadata_id, movies.id AS movie_id FROM history
    JOIN movies ON movies.metadata_id = history.metadata_id WHERE history.is_finished = false
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
            history: History {
                id: entry.history_id,
                time: entry.time,
                is_finished: entry.is_finished,
                update_time: entry.update_time.into(),
            },
            movie: movie_metadata.into(),
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
        history.metadata_id, episodes.number AS episode_number, seasons.show_id AS show_id,
        seasons.number AS season_number FROM history
    JOIN episodes ON episodes.metadata_id = history.metadata_id
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
            history: Some(History {
                id: entry.history_id,
                time: entry.time,
                is_finished: entry.is_finished,
                update_time: entry.update_time.into(),
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
        OptionalUuidQuery,
    ),
    request_body = UpdateHistoryPayload,
    responses(
        (status = 200, description = "History update is successful"),
        (status = 404, description = "History entry is not found", body = AppError),
    ),
    tag = "History",
)]
pub async fn update_history(
    State(app_state): State<AppState>,
    Path(id): Path<i64>,
    Query(OptionalUuidQuery { id: task_id }): Query<OptionalUuidQuery>,
    Json(payload): Json<UpdateHistoryPayload>,
) -> Result<(), AppError> {
    let update_time = time::OffsetDateTime::now_utc();
    let db = app_state.db;
    tracing::trace!(
        history_id = id,
        time = payload.time,
        "Updating history entry"
    );
    sqlx::query_scalar!(
        "UPDATE history SET time = ?, is_finished = ?, update_time = ? WHERE id = ? RETURNING metadata_id;",
        payload.time,
        payload.is_finished,
        update_time,
        id,
    )
    .fetch_one(&db.pool)
    .await?;
    if let Some(task_id) = task_id {
        let watch_sessions = &app_state.tasks.watch_sessions;
        let current_time = std::time::Duration::from_secs(payload.time as u64).into();
        let progress = WatchProgress { current_time };
        watch_sessions.send_progress(
            task_id,
            crate::progress::ProgressStatus::Pending { progress },
        );
    }
    Ok(())
}

/// Update/Insert history for specific metadata item
#[utoipa::path(
    put,
    path = "/api/metadata/{id}/history",
    params(
        ("id", description = "Metadata id"),
        OptionalUuidQuery,
    ),
    request_body = UpdateHistoryPayload,
    responses(
        (status = 200, description = "History entry is updated"),
        (status = 201, description = "History is created"),
        (status = 404, description = "Metadata is not found", body = AppError),
    ),
    tag = "Metadata",
)]
pub async fn update_metadata_history(
    State(app_state): State<AppState>,
    Path(metadata_id): Path<i64>,
    Query(OptionalUuidQuery { id: task_id }): Query<OptionalUuidQuery>,
    Json(payload): Json<UpdateHistoryPayload>,
) -> Result<StatusCode, AppError> {
    let db = app_state.db;
    if let Some(task_id) = task_id {
        let watch_sessions = &app_state.tasks.watch_sessions;
        let current_time = std::time::Duration::from_secs(payload.time as u64).into();
        let progress = WatchProgress { current_time };
        watch_sessions.send_progress(
            task_id,
            crate::progress::ProgressStatus::Pending { progress },
        );
    }
    let update_time = time::OffsetDateTime::now_utc().into();
    tracing::trace!(%metadata_id, time = payload.time, "Updating history");
    let query = sqlx::query!(
        "UPDATE history SET time = ?, is_finished = ?, update_time = ? WHERE metadata_id = ? RETURNING id;",
        payload.time,
        payload.is_finished,
        update_time,
        metadata_id,
    );
    if query.fetch_optional(&db.pool).await?.is_none() {
        db.pool
            .insert_history(crate::db::DbHistory {
                id: None,
                time: payload.time,
                is_finished: payload.is_finished,
                update_time: Some(update_time),
                metadata_id,
            })
            .await?;
        return Ok(StatusCode::CREATED);
    }
    Ok(StatusCode::OK)
}

/// Delete video history entry
#[utoipa::path(
    delete,
    path = "/api/metadata/{id}/history",
    params(
        ("id", description = "Metadata id"),
    ),
    responses(
        (status = 200, description = "History entry is deleted"),
        (status = 404, description = "Metadata is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn remove_metadata_history(
    State(db): State<Db>,
    Path(id): Path<i64>,
) -> Result<(), AppError> {
    let rows = sqlx::query!("DELETE FROM history WHERE metadata_id = ?;", id)
        .execute(&db.pool)
        .await?;
    if rows.rows_affected() == 0 {
        return Err(AppError::not_found("Content not found"));
    }
    Ok(())
}
