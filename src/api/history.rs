use std::time::Duration;

use axum::extract::State;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::{
    AppError,
    api::{
        CursorQuery, Json, OptionalUuidQuery, Path, Query, TakeQuery,
        api_data::{
            api_types::{Content, History},
            local_show::Episode,
        },
        lists::EpisodesList,
        server::CursoredResponse,
    },
    app_state::AppState,
    config,
    db::{self, Db, DbActions, LocalContentId, query_builders::DbHistoryQuery},
    metadata::{
        EpisodeMetadata, MetadataProvider, MovieMetadata, MovieMetadataProvider,
        ShowMetadataProvider,
        metadata_api::{
            PendingInsert,
            movie::MovieMetadataApi,
            show::{EpisodeNumber, ShowMetadataApi, WrittenShow},
        },
    },
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
) -> crate::Result<Json<CursoredResponse<HistoryEntry>>> {
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
pub async fn suggest_movies(State(db): State<Db>) -> crate::Result<Json<Vec<MovieHistory>>> {
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
pub async fn suggest_shows(State(db): State<Db>) -> crate::Result<Json<Vec<ShowSuggestion>>> {
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
pub async fn clear_history(State(db): State<Db>) -> crate::Result<()> {
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
pub async fn remove_history_item(State(db): State<Db>, Path(id): Path<i64>) -> crate::Result<()> {
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
) -> crate::Result<()> {
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
) -> crate::Result<StatusCode> {
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
) -> crate::Result<()> {
    let rows = sqlx::query!("DELETE FROM history WHERE metadata_id = ?;", id)
        .execute(&db.pool)
        .await?;
    if rows.rows_affected() == 0 {
        return Err(AppError::not_found("Content not found"));
    }
    Ok(())
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "content_type")]
pub enum MarkAsWatchedContent {
    Movie,
    Show(EpisodesList),
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct MarkAsWatched {
    pub content: MarkAsWatchedContent,
    pub provider: MetadataProvider,
    pub provider_id: String,
}

/// Mark external metadata item as watched
#[utoipa::path(
    post,
    path = "/api/history/external_mark_as_watched",
    request_body = MarkAsWatched,
    responses(
        (status = 201, description = "History entry is created"),
        (status = 404, description = "Metadata is not found", body = AppError),
    ),
    tag = "Videos",
)]
pub async fn external_mark_as_watched(
    State(AppState {
        db,
        providers_stack,
        http_client,
        ..
    }): State<AppState>,
    Json(MarkAsWatched {
        content,
        provider,
        provider_id,
    }): Json<MarkAsWatched>,
) -> crate::Result<()> {
    match content {
        MarkAsWatchedContent::Movie => {
            let Some(provider) = providers_stack.movie_provider(provider) else {
                return Err(AppError::not_found(
                    "request metadata provider was not found",
                ));
            };
            let api = MovieMetadataApi::new(provider, db, http_client);
            mark_external_movie_as_watched(&provider_id, api).await?;
        }
        MarkAsWatchedContent::Show(episodes) => {
            let Some(provider) = providers_stack.show_provider(provider) else {
                return Err(AppError::not_found(
                    "request metadata provider was not found",
                ));
            };
            let api = ShowMetadataApi::new(provider, db, http_client);
            mark_external_show_as_watched(episodes, api, &provider_id).await?;
        }
    };
    Ok(())
}

async fn mark_external_show_as_watched<T>(
    episodes: EpisodesList,
    api: ShowMetadataApi<T>,
    provider_id: &str,
) -> crate::Result<WrittenShow<EpisodeNumber>>
where
    T: ShowMetadataProvider + Clone + Send + Sync + 'static,
{
    // The api resolves/inserts the tree (retrying on concurrent inserts) and hands back the still
    // open transaction so the history rows are written atomically with the metadata.
    let PendingInsert {
        content: written,
        mut tx,
        assets,
    } = api.get_or_insert_show_tree(provider_id, episodes).await?;
    let update_time = time::OffsetDateTime::now_utc();
    for episode in written.episodes() {
        tx.insert_history(crate::db::DbHistory {
            id: None,
            time: 0,
            is_finished: true,
            update_time: Some(update_time.into()),
            metadata_id: episode.metadata_id,
        })
        .await?;
    }
    tx.commit().await?;
    let config::scan::MaxAssetConcurrency(assets_concurrency) = config::CONFIG.get_value();
    assets.save(assets_concurrency, ()).await;
    Ok(written)
}

async fn mark_external_movie_as_watched<T>(
    provider_id: &str,
    api: MovieMetadataApi<T>,
) -> crate::Result<LocalContentId>
where
    T: MovieMetadataProvider + Clone + Send + Sync + 'static,
{
    // The api resolves/inserts the movie (retrying on concurrent inserts) and hands back the still
    // open transaction so the history row is written atomically with the metadata.
    let PendingInsert {
        content: local_id,
        mut tx,
        assets,
    } = api.get_or_insert_movie(provider_id).await?;
    let update_time = time::OffsetDateTime::now_utc();
    tx.insert_history(crate::db::DbHistory {
        id: None,
        time: 0,
        is_finished: true,
        update_time: Some(update_time.into()),
        metadata_id: local_id.metadata_id,
    })
    .await?;
    tx.commit().await?;
    let config::scan::MaxAssetConcurrency(assets_concurrency) = config::CONFIG.get_value();
    assets.save(assets_concurrency, ()).await;
    Ok(local_id)
}

#[cfg(test)]
mod tests {
    use crate::metadata::metadata_api::asset_saver::AssetTasks;
    use crate::metadata::metadata_api::tests::{
        leak_db,
        provider_mock::{MockProvider, MovieKey, ShowTreeBuilder},
    };
    use sqlx::SqlitePool;
    use sqlx::pool::PoolOptions;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode};
    use tokio::task::JoinSet;

    use super::*;

    async fn leak_db_like_prod(
        pool_opts: PoolOptions<sqlx::Sqlite>,
        connect_opts: SqliteConnectOptions,
    ) -> &'static Db {
        let opts = connect_opts
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let pool = pool_opts
            .connect_with(opts)
            .await
            .expect("connect test pool");
        leak_db(pool)
    }

    #[sqlx::test]
    async fn marking_external_episodes_adds_them_to_database(
        pool: SqlitePool,
    ) -> anyhow::Result<()> {
        let db = leak_db(pool);
        let show_tree = ShowTreeBuilder::new(1).season(1, 1..=2);
        let show_key = show_tree.show_key();
        let provider = MockProvider::new([show_tree], []);
        let api = ShowMetadataApi::new_test(provider, db);
        let written = mark_external_show_as_watched(
            EpisodesList {
                season: 1,
                episodes: vec![1, 2],
            },
            api,
            &show_key.show_metadata().metadata_id,
        )
        .await?;

        assert_eq!(
            written.episodes().count(),
            2,
            "new episodes should be written in database"
        );

        let history_count: i64 = sqlx::query_scalar!(
            "select count(*) from history
            join metadata on metadata.id = history.metadata_id
            where is_finished = 1 and content_type = 'episode'",
        )
        .fetch_one(&db.pool)
        .await?;
        assert_eq!(history_count, 2, "new episodes should be in history");

        Ok(())
    }

    #[sqlx::test]
    async fn marking_external_movie_adds_it_to_database(pool: SqlitePool) -> anyhow::Result<()> {
        let db = leak_db(pool);
        let movie_key = MovieKey::new(1);
        let provider_metadata = movie_key.external_metadata();
        let provider = MockProvider::new([], [movie_key]);

        let api = MovieMetadataApi::new_test(provider, db);
        let content_id =
            mark_external_movie_as_watched(&provider_metadata.metadata_id, api).await?;
        assert_eq!(content_id.metadata_id, 1, "movie was inserted");
        let history = sqlx::query!("select * from history")
            .fetch_all(&db.pool)
            .await?;
        assert_eq!(history.len(), 1, "history should contain watched movie");
        assert_eq!(history[0].metadata_id, content_id.metadata_id);
        Ok(())
    }

    #[sqlx::test]
    async fn concurrent_external_movie_marks_are_idempotent(
        pool_opts: PoolOptions<sqlx::Sqlite>,
        connect_opts: SqliteConnectOptions,
    ) -> anyhow::Result<()> {
        let db = leak_db_like_prod(pool_opts, connect_opts).await;
        let movie_key = MovieKey::new(1);
        let provider_metadata = movie_key.external_metadata();
        let provider = MockProvider::new([], [movie_key]);
        let id = provider_metadata.metadata_id;
        let api = MovieMetadataApi::new_test(provider, db);
        let mut set = JoinSet::new();
        for _ in 0..2 {
            let api = api.clone();
            let id = id.clone();
            set.spawn(async move { mark_external_movie_as_watched(&id, api).await.unwrap() });
        }

        let res = set.join_all().await;
        assert!(
            res.iter()
                .all(|movie| movie.id == res[0].id && movie.metadata_id == res[0].metadata_id),
            "all calls resolve to the same movie"
        );

        let movie_count: i64 = sqlx::query_scalar!("select count(*) from movies")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(movie_count, 1, "exactly one movie row despite the race");
        let ext_count: i64 = sqlx::query_scalar!("select count(*) from external_ids")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(ext_count, 1, "exactly one external id row despite the race");
        Ok(())
    }

    #[sqlx::test]
    async fn concurrent_external_show_marks_are_idempotent(
        pool_opts: PoolOptions<sqlx::Sqlite>,
        connect_opts: SqliteConnectOptions,
    ) -> anyhow::Result<()> {
        let db = leak_db_like_prod(pool_opts, connect_opts).await;
        let show_tree = ShowTreeBuilder::new(1).season(1, 1..=2);
        let show_id = show_tree.show_key().show_metadata().metadata_id;
        let provider = MockProvider::new([show_tree], []);

        let api = ShowMetadataApi::new_test(provider, db);
        let mut set = JoinSet::new();
        for _ in 0..2 {
            let api = api.clone();
            let id = show_id.clone();
            let list = EpisodesList {
                season: 1,
                episodes: vec![1, 2],
            };
            set.spawn(async move { mark_external_show_as_watched(list, api, &id).await.unwrap() });
        }
        let res = set.join_all().await;
        assert!(
            res.iter().all(
                |show| show.show_id == res[0].show_id && show.metadata_id == res[0].metadata_id
            ),
            "all calls resolve to the same show"
        );

        let show_count: i64 = sqlx::query_scalar!("select count(*) from shows")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(show_count, 1, "exactly one show row despite the race");
        let episode_count: i64 = sqlx::query_scalar!("select count(*) from episodes")
            .fetch_one(&db.pool)
            .await?;
        assert_eq!(
            episode_count, 2,
            "exactly two episode rows despite the race"
        );
        Ok(())
    }

    #[sqlx::test]
    async fn marking_local_movie_adds_it_to_database(pool: SqlitePool) -> anyhow::Result<()> {
        let db = leak_db(pool);
        let movie_key = MovieKey::new(1);
        let provider_metadata = movie_key.external_metadata();
        let provider = MockProvider::new([], []);
        let api = MovieMetadataApi::new(provider.clone(), db, reqwest::Client::new());
        let mut tx = db.pool.begin().await?;
        let inserted_movie = api
            .insert_movie_metadata(
                provider_metadata.clone(),
                &mut tx,
                &mut AssetTasks::new(reqwest::Client::new()),
            )
            .await?;
        tx.commit().await?;

        let content_id =
            mark_external_movie_as_watched(&provider_metadata.metadata_id, api).await?;
        assert_eq!(
            content_id.metadata_id, inserted_movie.metadata_id,
            "existing movie was reused"
        );
        let history = sqlx::query!("select * from history")
            .fetch_all(&db.pool)
            .await?;
        assert_eq!(history.len(), 1, "history should contain watched movie");
        assert_eq!(history[0].metadata_id, content_id.metadata_id);
        Ok(())
    }
}
