use crate::{
    AppError,
    api::{
        Json, Path,
        api_data::{
            local_movie::Movie,
            local_show::{Episode, Show},
        },
    },
    app_state::AppState,
    db::{
        Db, DbActions, DbList, DbListItem, DbQueryBuilder, ListKind,
        query_builders::{DbFullEpisodeQuery, DbMovieQuery, DbShowQuery},
    },
    lists,
    metadata::{
        MetadataProvider,
        metadata_api::{
            PendingInsert,
            asset_saver::AssetTasks,
            batch::BatchApi,
            movie::MovieMetadataApi,
            show::{EpisodeInput, EpisodeNumber, SeasonInput, ShowMetadataApi, ShowTree},
        },
    },
};

use std::collections::HashMap;

use axum::{
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
struct List {
    id: i64,
    name: String,
    description: Option<String>,
    size: usize,
    created_at: super::CrateOffsetDateTime,
    updated_at: super::CrateOffsetDateTime,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct AllLists {
    saved: List,
    watch: List,
    custom: Vec<List>,
}

/// Get all lists
#[utoipa::path(
    get,
    path = "/api/lists",
    responses(
        (status = 200, description = "List of all lists", body = AllLists),
    ),
    tag = "Lists",
)]
async fn all_lists(State(db): State<Db>) -> crate::Result<Json<AllLists>> {
    let lists = sqlx::query!("select *, (select count(id) from list_items where list_items.list_id = lists.id) as count from lists
        where kind = 'user' order by updated_at desc")
        .fetch_all(&db.pool)
        .await?;
    let custom = lists
        .into_iter()
        .map(|r| List {
            id: r.id,
            name: r.name,
            description: r.description,
            size: r.count as usize,
            created_at: r.created_at.into(),
            updated_at: r.updated_at.into(),
        })
        .collect();
    let mut system_lists = sqlx::query!("select *, (select count(id) from list_items where list_items.list_id = lists.id) as count from lists
        where kind != 'user' order by id")
        .fetch_all(&db.pool)
        .await?.into_iter();

    let saved = system_lists.next().expect("saved system list exists");
    let saved = List {
        id: saved.id,
        name: saved.name,
        description: saved.description,
        size: saved.count as usize,
        created_at: saved.created_at.into(),
        updated_at: saved.updated_at.into(),
    };

    let watch = system_lists.next().expect("watch system list exists");
    let watch = List {
        id: watch.id,
        name: watch.name,
        description: watch.description,
        size: watch.count as usize,
        created_at: watch.created_at.into(),
        updated_at: watch.updated_at.into(),
    };
    debug_assert_eq!(saved.id, ListKind::SAVED_ID);
    debug_assert_eq!(watch.id, ListKind::WATCH_ID);

    Ok(Json(AllLists {
        saved,
        watch,
        custom,
    }))
}

/// Get single list info
#[utoipa::path(
    get,
    path = "/api/lists/{id}",
    responses(
        (status = 200, description = "List info", body = List),
        (status = 404, description = "List was not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn get_list(Path(id): Path<i64>, State(db): State<Db>) -> crate::Result<Json<List>> {
    let list = sqlx::query!("select *, (select count(id) from list_items where list_items.list_id = lists.id) as count from lists
        where id = ?", id)
        .fetch_one(&db.pool)
        .await?;
    let list = List {
        id: list.id,
        name: list.name,
        description: list.description,
        size: list.count as usize,
        created_at: list.created_at.into(),
        updated_at: list.updated_at.into(),
    };
    Ok(Json(list))
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "content_type")]
pub enum ListContent {
    Show(Show),
    Movie(Movie),
    Episode(ListEpisode),
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ListEpisode {
    #[serde(flatten)]
    pub episode: Episode,
    /// Local id of the show this episode belongs to
    pub show_id: i64,
    pub show_title: String,
}

/// Get list contents
#[utoipa::path(
    get,
    path = "/api/lists/{id}/items",
    params(
        ("id", description = "List id"),
    ),
    responses(
        (status = 200, description = "Content stored in the list", body = Vec<ListContent>),
    ),
    tag = "Lists",
)]
async fn list_contents(
    Path(id): Path<i64>,
    State(db): State<Db>,
) -> crate::Result<Json<Vec<ListContent>>> {
    // Insertion order of the list, used to order the result once the per-kind queries are merged.
    let ordered = sqlx::query_scalar!(
        "select metadata_id from list_items where list_id = ? order by created_at, id",
        id
    )
    .fetch_all(&db.pool)
    .await?;

    const FILTER: &str =
        " where metadata.id in (select metadata_id from list_items where list_id = ";

    let mut shows_query = DbQueryBuilder::default();
    DbShowQuery::build(&mut shows_query);
    let shows = shows_query
        .push(FILTER)
        .push_bind(id)
        .push(")")
        .build_query_as::<DbShowQuery>()
        .fetch_all(&db.pool)
        .await?;

    let mut movies_query = DbQueryBuilder::default();
    DbMovieQuery::build(&mut movies_query);
    let movies = movies_query
        .push(FILTER)
        .push_bind(id)
        .push(")")
        .build_query_as::<DbMovieQuery>()
        .fetch_all(&db.pool)
        .await?;

    let mut episodes_query = DbQueryBuilder::default();
    DbFullEpisodeQuery::build(&mut episodes_query);
    let episodes = episodes_query
        .push(FILTER)
        .push_bind(id)
        .push(")")
        .build_query_as::<DbFullEpisodeQuery>()
        .fetch_all(&db.pool)
        .await?;

    let mut by_metadata: HashMap<i64, ListContent> = HashMap::new();
    for show in shows {
        by_metadata.insert(show.metadata.id.unwrap(), ListContent::Show(show.into()));
    }
    for movie in movies {
        by_metadata.insert(movie.metadata.id.unwrap(), ListContent::Movie(movie.into()));
    }
    for query_result in episodes {
        let metadata_id = query_result.episode.metadata.id.unwrap();
        let show_id = query_result.show_id;
        let show_title = query_result.show_title;
        by_metadata.insert(
            metadata_id,
            ListContent::Episode(ListEpisode {
                episode: query_result.episode.into(),
                show_id,
                show_title,
            }),
        );
    }

    let contents = ordered
        .into_iter()
        .filter_map(|metadata_id| by_metadata.remove(&metadata_id))
        .collect();
    Ok(Json(contents))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "content_type")]
enum ListItemContentType {
    Movie,
    Show { episodes: Option<EpisodesList> },
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct EpisodesList {
    pub season: usize,
    pub episodes: Vec<usize>,
}

impl From<EpisodesList> for ShowTree<EpisodeNumber> {
    fn from(EpisodesList { season, episodes }: EpisodesList) -> Self {
        Self {
            seasons: vec![SeasonInput {
                number: season,
                episodes: episodes
                    .into_iter()
                    .map(|number| EpisodeInput {
                        number,
                        items: Vec::new(),
                    })
                    .collect(),
            }],
        }
    }
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
enum ListItems {
    Local {
        metadata_ids: Vec<i64>,
    },
    External {
        id: String,
        provider: MetadataProvider,
        #[serde(flatten)]
        content_type: ListItemContentType,
    },
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub(super) struct CreateList {
    name: String,
    description: Option<String>,
}

/// Create custom list
#[utoipa::path(
    post,
    path = "/api/lists/create",
    request_body = CreateList,
    responses(
        (status = 201, description = "Successfully created list"),
    ),
    tag = "Lists",
)]
async fn create_list(
    State(db): State<Db>,
    Json(CreateList { name, description }): Json<CreateList>,
) -> crate::Result<StatusCode> {
    let now = OffsetDateTime::now_utc();
    db.insert_list(&DbList {
        id: None,
        kind: ListKind::User,
        name,
        description,
        created_at: now,
        updated_at: now,
    })
    .await?;
    Ok(StatusCode::CREATED)
}

/// Update custom list
#[utoipa::path(
    put,
    path = "/api/lists/{id}",
    params(
        ("id", description = "List id"),
    ),
    responses(
        (status = 200, description = "Successfully updated list"),
        (status = 404, description = "List not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn update_list(
    Path(id): Path<i64>,
    State(db): State<Db>,
    Json(CreateList { name, description }): Json<CreateList>,
) -> crate::Result<StatusCode> {
    let updated_at = OffsetDateTime::now_utc();
    let res = sqlx::query!(
        "update lists set name = ?, description = ?, updated_at = ? where id = ?",
        name,
        description,
        updated_at,
        id
    )
    .execute(&db.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::not_found("list not found"));
    }
    Ok(StatusCode::OK)
}

/// Delete custom list
#[utoipa::path(
    delete,
    path = "/api/lists/{id}",
    params(
        ("id", description = "List id"),
    ),
    responses(
        (status = 200, description = "Successfully deleted list"),
        (status = 404, description = "List not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn delete_list(Path(id): Path<i64>, State(db): State<Db>) -> crate::Result<StatusCode> {
    let res = sqlx::query!(
        "delete from lists where id = ? and kind = ?",
        id,
        ListKind::User
    )
    .execute(&db.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::not_found("list not found"));
    }
    Ok(StatusCode::OK)
}

const CONTENT_LINK_ERROR_TEXT: &str = "one of the content metadata items was not found";

async fn resolve_content(
    app_state: &AppState,
    items: ListItems,
) -> crate::Result<PendingInsert<Vec<i64>>> {
    let db = app_state.db;
    let providers = app_state.providers_stack;
    match items {
        ListItems::Local { metadata_ids } => Ok(PendingInsert {
            content: metadata_ids,
            tx: db.pool.begin().await?,
            assets: AssetTasks::new(app_state.http_client.clone()),
        }),
        ListItems::External {
            id,
            provider,
            content_type,
        } => match content_type {
            ListItemContentType::Movie => {
                let Some(movie_provider) = providers.movie_provider(provider) else {
                    return Err(AppError::not_found(
                        "requested movie provider is not available",
                    ));
                };
                let movie_api =
                    MovieMetadataApi::new(movie_provider, &db, app_state.http_client.clone());
                let resolved = movie_api.get_or_insert_movie(&id).await?;
                Ok(PendingInsert {
                    content: vec![resolved.content.metadata_id],
                    tx: resolved.tx,
                    assets: resolved.assets,
                })
            }
            ListItemContentType::Show { episodes } => {
                let Some(show_provider) = providers.show_provider(provider) else {
                    return Err(AppError::not_found(
                        "requested tv show provider is not available",
                    ));
                };
                let show_api =
                    ShowMetadataApi::new(show_provider, &db, app_state.http_client.clone());
                let flushed = show_api
                    .get_or_insert_show_tree(
                        &id,
                        episodes.map(Into::<ShowTree<_>>::into).unwrap_or_default(),
                    )
                    .await?;
                Ok(flushed.map(|tree| {
                    if tree.seasons.is_empty() {
                        vec![tree.metadata_id]
                    } else {
                        tree.seasons
                            .into_iter()
                            .flat_map(|s| s.episodes)
                            .map(|e| e.metadata_id)
                            .collect()
                    }
                }))
            }
        },
    }
}

/// Links every resolved metadata id into `list_id`, commits, then kicks off asset downloads.
/// `release_viewed` is stored per item for the watchlist and left `None` for other lists.
async fn link_content(
    pending: PendingInsert<Vec<i64>>,
    list_id: i64,
    release_viewed: Option<bool>,
) -> crate::Result<()> {
    let PendingInsert {
        content: metadata_ids,
        mut tx,
        assets,
    } = pending;
    let created_at = OffsetDateTime::now_utc();
    for metadata_id in metadata_ids {
        match tx
            .insert_list_item(&DbListItem {
                id: None,
                list_id,
                metadata_id,
                release_viewed,
                created_at,
            })
            .await
        {
            Ok(_) => {}
            Err(sqlx::Error::Database(e)) if e.is_foreign_key_violation() => {
                return Err(AppError::not_found(CONTENT_LINK_ERROR_TEXT));
            }
            Err(e) => return Err(e.into()),
        }
    }
    tx.commit().await?;
    assets.save(16, ()).await;
    Ok(())
}

/// Add content to custom list
#[utoipa::path(
    post,
    path = "/api/lists/{id}/add",
    params(
        ("id", description = "List id"),
    ),
    responses(
        (status = 201, description = "Successfully added content item"),
        (status = 404, description = "List or content not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn add_item(
    Path(list_id): Path<i64>,
    State(app_state): State<AppState>,
    Json(items): Json<ListItems>,
) -> crate::Result<StatusCode> {
    let pending = resolve_content(&app_state, items).await?;
    link_content(pending, list_id, None).await?;
    Ok(StatusCode::CREATED)
}

async fn remove_list_item(db: &Db, list_id: i64, metadata_id: i64) -> crate::Result<()> {
    let res = sqlx::query!(
        "delete from list_items where list_id = ? and metadata_id = ?",
        list_id,
        metadata_id
    )
    .execute(&db.pool)
    .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::not_found("content or list was not found"));
    }
    Ok(())
}

/// Remove item from custom list
#[utoipa::path(
    delete,
    path = "/api/lists/{id}/remove/{metadata_id}",
    params(
        ("id", description = "List id"),
        ("metadata_id", description = "Target metadata id"),
    ),
    responses(
        (status = 200, description = "Successfully removed content item"),
        (status = 404, description = "List or content not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn remove_item(
    Path((list_id, metadata_id)): Path<(i64, i64)>,
    State(db): State<Db>,
) -> crate::Result<()> {
    remove_list_item(&db, list_id, metadata_id).await
}

/// Export list in json but group all episodes into a single show
#[utoipa::path(
    get,
    path = "/api/lists/{id}/export",
    params(
        ("id", description = "List id"),
    ),
    responses(
        (status = 200, description = "Exported list in json format", content_type = "application/json", body = Vec<lists::ExportedGroupedItem>),
        (status = 404, description = "List not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn export_list(
    Path(list_id): Path<i64>,
    State(db): State<Db>,
) -> crate::Result<impl axum::response::IntoResponse> {
    let list_name: String = sqlx::query_scalar!("select name from lists where id = ?", list_id)
        .fetch_one(&db.pool)
        .await?
        .chars()
        // ensure that list name contains only ascii to use it in header
        .map(|c| if c.is_ascii() { c } else { '?' })
        .collect();

    let items = lists::export_grouped_list(&db, list_id).await?;

    let headers = HeaderMap::from_iter([(
        HeaderName::from_static("content-disposition"),
        HeaderValue::from_str(&format!(
            "attachment; filename=\"exported_list_{list_name}.json\""
        ))
        .expect("filename to be ascii only"),
    )]);
    Ok((headers, Json(items)).into_response())
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ImportResult {
    count: usize,
}

/// Import grouped list in json
#[utoipa::path(
    post,
    path = "/api/lists/{id}/import",
    params(
        ("id", description = "List id"),
    ),
    responses(
        (status = 200, description = "Import results", body = ImportResult),
        (status = 500, description = "Import error", body = AppError),
    ),
    tag = "Lists",
)]
async fn import_list(
    Path(list_id): Path<i64>,
    State(AppState {
        db,
        providers_stack,
        http_client,
        ..
    }): State<AppState>,
    Json(items): Json<Vec<lists::ExportedGroupedItem>>,
) -> crate::Result<Json<ImportResult>> {
    let mut batch_api = BatchApi::<EpisodeNumber, bool, ()>::new(db.clone(), http_client.clone());
    for item in items {
        let Some(prime_id) = item.external_ids.into_iter().find(|id| id.is_prime) else {
            tracing::error!("Primary external id for {} was not found", item.title);
            continue;
        };
        match item.content_type {
            lists::ExportedGroupedContentType::Movie => {
                let Some(provider) = providers_stack.movie_provider(prime_id.provider) else {
                    tracing::error!("Movie provider {} is not available", prime_id.provider);
                    continue;
                };
                let movie_api = MovieMetadataApi::new(provider, db, http_client.clone());
                batch_api.spawn_movie(movie_api, prime_id.id, ());
            }
            lists::ExportedGroupedContentType::Show {
                self_in_list,
                episodes,
            } => {
                let Some(provider) = providers_stack.show_provider(prime_id.provider) else {
                    tracing::error!("Show provider {} is not available", prime_id.provider);
                    continue;
                };
                let show_api = ShowMetadataApi::new(provider, db, http_client.clone());
                batch_api.spawn_show(show_api, prime_id.id, episodes, self_in_list);
            }
        }
    }

    let mut result = ImportResult { count: 0 };
    batch_api
        .join_all(
            &mut result,
            |local_id, tx, _, ctx| {
                Box::pin(async move {
                    let now = OffsetDateTime::now_utc();
                    match tx
                        .insert_list_item(&DbListItem {
                            id: None,
                            list_id,
                            metadata_id: local_id.metadata_id,
                            release_viewed: None,
                            created_at: now,
                        })
                        .await
                    {
                        Ok(_) => {
                            ctx.count += 1;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to insert movie into the list: {e}");
                        }
                    };
                    Ok(())
                })
            },
            |show, tx, self_in_list, ctx| {
                Box::pin(async move {
                    let now = OffsetDateTime::now_utc();
                    if self_in_list {
                        match tx
                            .insert_list_item(&DbListItem {
                                id: None,
                                list_id,
                                metadata_id: show.metadata_id,
                                release_viewed: None,
                                created_at: now,
                            })
                            .await
                        {
                            Ok(_) => {
                                ctx.count += 1;
                            }
                            Err(e) => {
                                tracing::warn!("Failed to insert show into the list: {e}");
                            }
                        };
                    }
                    for episode in show.episodes() {
                        match tx
                            .insert_list_item(&DbListItem {
                                id: None,
                                list_id,
                                metadata_id: episode.metadata_id,
                                release_viewed: None,
                                created_at: now,
                            })
                            .await
                        {
                            Ok(_) => ctx.count += 1,
                            Err(e) => {
                                tracing::warn!("Failed to insert episode into the list: {e}");
                            }
                        };
                    }
                    Ok(())
                })
            },
        )
        .await?;

    Ok(Json(result))
}

/// Add content to the saved list
#[utoipa::path(
    post,
    path = "/api/lists/saved/add",
    responses(
        (status = 201, description = "Successfully saved content item"),
        (status = 404, description = "Content not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn add_to_saved(
    State(app_state): State<AppState>,
    Json(items): Json<ListItems>,
) -> crate::Result<StatusCode> {
    let pending = resolve_content(&app_state, items).await?;
    link_content(pending, ListKind::SAVED_ID, None).await?;
    Ok(StatusCode::CREATED)
}

/// Remove item from saved list
#[utoipa::path(
    delete,
    path = "/api/lists/saved/remove/{metadata_id}",
    params(
        ("metadata_id", description = "Target metadata id"),
    ),
    responses(
        (status = 200, description = "Successfully removed content item"),
        (status = 404, description = "Content not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn remove_saved_item(
    Path(metadata_id): Path<i64>,
    State(db): State<Db>,
) -> crate::Result<()> {
    remove_list_item(&db, ListKind::SAVED_ID, metadata_id).await
}

/// Add content to the watchlist
#[utoipa::path(
    post,
    path = "/api/lists/watchlist/add",
    responses(
        (status = 201, description = "Successfully added content to watchlist"),
        (status = 404, description = "Content not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn add_to_watchlist(
    State(app_state): State<AppState>,
    Json(items): Json<ListItems>,
) -> crate::Result<StatusCode> {
    let pending = resolve_content(&app_state, items).await?;
    link_content(pending, ListKind::WATCH_ID, Some(true)).await?;
    Ok(StatusCode::CREATED)
}

/// Remove item from watch list
#[utoipa::path(
    delete,
    path = "/api/lists/watchlist/remove/{metadata_id}",
    params(
        ("metadata_id", description = "Target metadata id"),
    ),
    responses(
        (status = 200, description = "Successfully removed content item"),
        (status = 404, description = "Content not found", body = AppError),
    ),
    tag = "Lists",
)]
async fn remove_watchlist_item(
    Path(metadata_id): Path<i64>,
    State(db): State<Db>,
) -> crate::Result<()> {
    remove_list_item(&db, ListKind::WATCH_ID, metadata_id).await
}

pub fn router() -> axum::Router<AppState> {
    use axum::routing::{delete, get, post, put};

    axum::Router::new()
        .route("/", get(all_lists))
        .route("/create", post(create_list))
        .route("/{id}", put(update_list).delete(delete_list).get(get_list))
        .route("/{id}/items", get(list_contents))
        .route("/{id}/add", post(add_item))
        .route("/{id}/remove/{id}", delete(remove_item))
        .route("/{id}/export", get(export_list))
        .route("/{id}/import", post(import_list))
        .route("/saved/add", post(add_to_saved))
        .route("/saved/remove/{id}", delete(remove_saved_item))
        .route("/watchlist/add", post(add_to_watchlist))
        .route("/watchlist/remove/{id}", delete(remove_watchlist_item))
}

#[cfg(test)]
mod tests {
    use super::*;

    use sqlx::SqlitePool;

    use crate::AppErrorKind;
    use crate::db::DbActions;
    use crate::metadata::metadata_api::tests::leak_db;

    #[sqlx::test]
    async fn linking_unknown_metadata_ids_fails(pool: SqlitePool) -> anyhow::Result<()> {
        let db = leak_db(pool);
        let list_id = db
            .insert_list(&DbList {
                kind: ListKind::User,
                name: "test".into(),
                id: None,
                description: None,
                created_at: OffsetDateTime::now_utc(),
                updated_at: OffsetDateTime::now_utc(),
            })
            .await?;
        let pending = PendingInsert {
            content: vec![100],
            tx: db.pool.begin().await?,
            assets: AssetTasks::new(reqwest::Client::new()),
        };
        let res = link_content(pending, list_id, None)
            .await
            .expect_err("request should fail");
        assert_eq!(res.kind, AppErrorKind::NotFound);
        assert_eq!(res.message, CONTENT_LINK_ERROR_TEXT);
        Ok(())
    }
}
