use std::{collections::HashMap, time::Duration};

use sqlx::QueryBuilder;

use crate::{
    api::{
        api_data::{
            api_types::{Actor, History},
            local_movie::Movie,
            local_show::{Episode, LocalEpisodeData},
        },
        server::Intro,
    },
    db::{Db, DbActions, LocalContentId, query_builders::ListsQueryJson},
    metadata::{EpisodeMetadata, MetadataProvider, MovieMetadata, PersonMetadata, ShowMetadata},
};

pub mod api_types;
pub mod local_actor;
pub mod local_movie;
pub mod local_show;

/// Extend external metadata with local information
#[derive(Debug)]
pub struct LocalDataLookup {
    db: Db,
}

impl LocalDataLookup {
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    async fn crossreference_show(
        &self,
        metadata_provider: MetadataProvider,
        metadata_id: &str,
    ) -> sqlx::Result<Option<LocalContentId>> {
        if metadata_provider == MetadataProvider::Local {
            let id: i64 = metadata_id.parse().unwrap();
            sqlx::query_as!(
                LocalContentId,
                r#"SELECT id as "id!", metadata_id FROM shows WHERE id = ?"#,
                id
            )
            .fetch_optional(&self.db.pool)
            .await
        } else {
            self.db
                .crossreference_show(metadata_provider, metadata_id)
                .await
        }
    }

    async fn crossreference_movie(
        &self,
        metadata_provider: MetadataProvider,
        metadata_id: &str,
    ) -> sqlx::Result<Option<LocalContentId>> {
        if metadata_provider == MetadataProvider::Local {
            let id: i64 = metadata_id.parse().unwrap();
            sqlx::query_as!(
                LocalContentId,
                r#"SELECT id as "id!", metadata_id FROM movies WHERE id = ?"#,
                id
            )
            .fetch_optional(&self.db.pool)
            .await
        } else {
            self.db
                .crossreference_movie(metadata_provider, metadata_id)
                .await
        }
    }

    pub async fn extend_shows_with_local_data(
        &self,
        shows: Vec<ShowMetadata>,
    ) -> sqlx::Result<Vec<local_show::Show>> {
        #[derive(sqlx::FromRow)]
        struct Record {
            id: i64,
            metadata_id: i64,
            external_provider: MetadataProvider,
            external_id: String,
            #[sqlx(json, default, nullish)]
            lists: Option<Vec<ListsQueryJson>>,
        }
        let mut local_map = QueryBuilder::new(format!(
            r#"select shows.id, shows.metadata_id, external_ids.external_provider, external_ids.external_id, {lists} from external_ids
            join shows on shows.metadata_id = external_ids.metadata_id
            join metadata on metadata.id = shows.metadata_id
            where (external_ids.external_provider, external_ids.external_id) in"#,
            lists = ListsQueryJson::SQL_JSON_AGGR,
        ))
            .push_tuples(shows.iter(), |mut b, meta| {
                b.push_bind(meta.metadata_provider.to_string())
                    .push_bind(&meta.metadata_id);
            })
            .build_query_as::<Record>()
            .fetch_all(&self.db.pool).await?
            .into_iter()
            .map(|v| ((v.external_provider, v.external_id), local_show::LocalShowData { id: v.id, metadata_id: v.metadata_id, lists: v.lists.into_iter().flatten().map(Into::into).collect() }))
            .collect::<HashMap<_, _>>();

        Ok(shows
            .into_iter()
            .map(|meta| {
                let local = local_map.remove(&(meta.metadata_provider, meta.metadata_id.clone()));
                local_show::Show {
                    provider_id: meta.metadata_id,
                    provider: meta.metadata_provider,
                    poster: meta.poster,
                    backdrop: meta.backdrop,
                    plot: meta.plot,
                    seasons: meta.seasons,
                    episodes_amount: meta.episodes_amount,
                    release_date: meta.release_date,
                    title: meta.title,
                    locale_metadata: meta.locale_metadata,
                    cast: meta.cast.map(|v| v.into_iter().map(Into::into).collect()),
                    external_ids: meta.external_ids,
                    genres: meta.genres,
                    next_episode_air_date: meta.next_episode_air_date,
                    local,
                }
            })
            .collect())
    }

    pub async fn extend_episodes_with_local_data(
        &self,
        meta: Vec<EpisodeMetadata>,
    ) -> sqlx::Result<Vec<Episode>> {
        #[derive(sqlx::FromRow)]
        struct Record {
            id: i64,
            metadata_id: i64,
            videos_count: i64,
            history_id: Option<i64>,
            time: Option<i64>,
            update_time: Option<time::OffsetDateTime>,
            is_finished: Option<bool>,
            external_provider: MetadataProvider,
            external_id: String,
            intro_id: Option<i64>,
            start_sec: Option<i64>,
            end_sec: Option<i64>,
            #[sqlx(json, default, nullish)]
            lists: Option<Vec<ListsQueryJson>>,
        }
        let mut local_episodes = sqlx::QueryBuilder::new(format!(
            "select episodes.id, episodes.metadata_id,
            (select count(id) from videos where videos.metadata_id = episodes.metadata_id) as videos_count,
            external_ids.external_id, external_ids.external_provider,
            history.id as history_id, history.time, history.update_time, history.is_finished,
            intros.id as intro_id, intros.start_sec, intros.end_sec, {lists}
            from external_ids
            join episodes on episodes.metadata_id = external_ids.metadata_id
            join metadata on metadata.id = episodes.metadata_id
            left join intros on intros.episode_id = episodes.id
            left join history on history.metadata_id = episodes.metadata_id
            where (external_ids.external_provider, external_ids.external_id) in",
            lists = ListsQueryJson::SQL_JSON_AGGR,
        ))
        .push_tuples(meta.iter(), |mut b, meta| {
            b.push_bind(meta.metadata_provider)
                .push_bind(&meta.metadata_id);
        })
        .build_query_as::<Record>()
        .fetch_all(&self.db.pool)
        .await?
        .into_iter()
        .map(|r| {
            (
                (r.external_provider, r.external_id),
                LocalEpisodeData {
                    metadata_id: r.metadata_id,
                    id: r.id,
                    videos_count: r.videos_count,
                    lists: r.lists.into_iter().flatten().map(Into::into).collect(),
                    history: r.history_id.map(|id| History {
                        id,
                        time: r.time.unwrap(),
                        is_finished: r.is_finished.unwrap(),
                        update_time: r.update_time.map(Into::into).unwrap(),
                    }),
                    intro: r.intro_id.map(|_| Intro {
                        start_sec: r.start_sec.unwrap(),
                        end_sec: r.end_sec.unwrap(),
                    }),
                },
            )
        })
        .collect::<HashMap<_, _>>();
        Ok(meta
            .into_iter()
            .map(|episode_meta| Episode {
                provider_id: episode_meta.metadata_id.clone(),
                provider: episode_meta.metadata_provider,
                release_date: episode_meta.release_date,
                number: episode_meta.number,
                title: episode_meta.title,
                plot: episode_meta.plot,
                season_number: episode_meta.season_number,
                runtime: episode_meta.runtime,
                poster: episode_meta.poster,
                cast: None,
                local: local_episodes
                    .remove(&(episode_meta.metadata_provider, episode_meta.metadata_id)),
            })
            .collect())
    }

    pub async fn extend_movies_with_local_data(
        &self,
        movies: Vec<MovieMetadata>,
    ) -> sqlx::Result<Vec<local_movie::Movie>> {
        #[derive(sqlx::FromRow)]
        struct Record {
            id: i64,
            metadata_id: i64,
            videos_count: i64,
            external_provider: MetadataProvider,
            external_id: String,

            // history
            history_id: Option<i64>,
            time: Option<i64>,
            is_finished: Option<bool>,
            duration: i64,
            update_time: Option<time::OffsetDateTime>,
            #[sqlx(json, default, nullish)]
            lists: Option<Vec<ListsQueryJson>>,
        }
        let mut local_map = QueryBuilder::new(format!(
            r#"select
            movies.id, movies.metadata_id, movies.duration,
            (select count(id) from videos where videos.metadata_id = movies.metadata_id) as videos_count,
            external_ids.external_provider, external_ids.external_id,
            history.id as history_id, history.time, history.is_finished, history.update_time, {lists}
            from external_ids
            join movies on movies.metadata_id = external_ids.metadata_id
            join metadata on metadata.id = movies.metadata_id
            left join history on history.metadata_id = movies.metadata_id
            where (external_ids.external_provider, external_ids.external_id) in"#,
            lists = ListsQueryJson::SQL_JSON_AGGR,
        ))
        .push_tuples(movies.iter(), |mut b, meta| {
            b.push_bind(meta.metadata_provider.to_string())
                .push_bind(&meta.metadata_id);
        })
        .build_query_as::<Record>()
        .fetch_all(&self.db.pool)
        .await?
        .into_iter()
        .map(|v| {
            (
                (v.external_provider, v.external_id),
                local_movie::LocalMovieData {
                    id: v.id,
                    metadata_id: v.metadata_id,
                    videos_count: v.videos_count,
                    lists: v.lists.into_iter().flatten().map(Into::into).collect(),
                    local_duration: Duration::from_secs(v.duration as u64).into(),
                    history: v.history_id.map(|id| History {
                        id,
                        time: v.time.unwrap(),
                        is_finished: v.is_finished.unwrap(),
                        update_time: v.update_time.map(Into::into).unwrap(),
                    }),
                },
            )
        })
        .collect::<HashMap<_, _>>();

        Ok(movies
            .into_iter()
            .map(|meta| {
                let local = local_map.remove(&(meta.metadata_provider, meta.metadata_id.clone()));
                Movie {
                    provider_id: meta.metadata_id,
                    provider: meta.metadata_provider,
                    poster: meta.poster,
                    backdrop: meta.backdrop,
                    plot: meta.plot,
                    release_date: meta.release_date,
                    runtime: meta.runtime,
                    title: meta.title,
                    locale_metadata: meta.locale_metadata,
                    cast: None,
                    external_ids: meta.external_ids,
                    genres: meta.genres,
                    local,
                }
            })
            .collect())
    }

    #[tracing::instrument(skip_all)]
    pub async fn extend_actors(
        &self,
        actor_metadata: Vec<PersonMetadata>,
    ) -> sqlx::Result<Vec<Actor>> {
        #[derive(sqlx::FromRow)]
        struct Record {
            id: i64,
            external_metadata_provider: MetadataProvider,
            external_metadata_id: String,
        }
        let mut local_map = QueryBuilder::new(
            r#"select actors.id, actors.external_metadata_provider, actors.external_metadata_id from actors
            where (actors.external_metadata_provider, actors.external_metadata_id) in "#,
        )
        .push_tuples(actor_metadata.iter(), |mut b, meta| {
            b.push_bind(meta.metadata_provider.to_string())
                .push_bind(&meta.metadata_id);
        })
        .build_query_as::<Record>()
        .fetch_all(&self.db.pool)
        .await?
        .into_iter()
        .map(|v| {
            (
                (v.external_metadata_provider, v.external_metadata_id),
                local_actor::LocalActorData { id: v.id },
            )
        })
        .collect::<HashMap<_, _>>();

        Ok(actor_metadata
            .into_iter()
            .map(|meta| {
                let local = local_map.remove(&(meta.metadata_provider, meta.metadata_id.clone()));
                Actor::extend_meta(meta, local)
            })
            .collect())
    }

    async fn season_data(
        &self,
        external_provider: MetadataProvider,
        external_id: &str,
        season: usize,
    ) -> sqlx::Result<Option<local_show::LocalSeasonData>> {
        let season = season as i64;
        let Some(local_id) = self
            .crossreference_show(external_provider, external_id)
            .await?
        else {
            return Ok(None);
        };

        Ok(sqlx::query!(
            "SELECT seasons.id, seasons.metadata_id from seasons WHERE seasons.show_id = ? and seasons.number = ?",
            local_id.id,
            season,
        )
        .fetch_optional(&self.db.pool)
        .await?
        .map(|v| local_show::LocalSeasonData { id: v.id, metadata_id: v.metadata_id }))
    }
}
