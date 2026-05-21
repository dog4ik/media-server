use crate::api::Json;
use crate::db::DbActions;
use crate::{
    app_state::{AppError, AppState},
    db::Db,
    intro_detection::IntroJob,
    progress::TaskError,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Intro {
    pub start_sec: i64,
    pub end_sec: i64,
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
) -> Result<StatusCode, AppError> {
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
    Ok(StatusCode::ACCEPTED)
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
        r#"SELECT intros.start_sec, intros.end_sec FROM intros
        JOIN episodes ON episodes.id = intros.episode_id
        WHERE episodes.metadata_id = (SELECT metadata_id FROM videos WHERE id = ?)"#,
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
        r#"DELETE FROM intros WHERE episode_id = (
            SELECT id FROM episodes WHERE metadata_id = (SELECT metadata_id FROM videos WHERE id = ?)
        ) RETURNING id"#,
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
        r#"SELECT intros.id FROM intros
        JOIN episodes ON episodes.id = intros.episode_id
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
        r#"SELECT intros.id FROM intros
        JOIN episodes ON episodes.id = intros.episode_id
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
        r#"UPDATE intros SET start_sec = ?, end_sec = ?
        WHERE episode_id = (SELECT id FROM episodes WHERE metadata_id = (SELECT metadata_id FROM videos WHERE id = ?))
        RETURNING id;"#,
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
            let episode_id = sqlx::query!(
                "SELECT id FROM episodes WHERE metadata_id = (SELECT metadata_id FROM videos WHERE id = ?)",
                video_id
            )
            .fetch_one(&db.pool)
            .await?
            .id;
            let db_intro = crate::db::DbIntro {
                id: None,
                episode_id,
                start_sec: start,
                end_sec: end,
            };
            db.insert_intro(db_intro).await?;
            Ok(StatusCode::CREATED)
        }
        Err(e) => Err(e)?,
    }
}
