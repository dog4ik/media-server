use std::collections::HashMap;

use sqlx::QueryBuilder;

use crate::{
    db::{Db, DbActions},
    metadata::{MetadataProvider, MovieMetadata, ShowMetadata},
    api::{api_data::api_types::History, server::Intro},
};

pub mod api_types;
pub mod local_movie;
pub mod local_show;

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
    ) -> sqlx::Result<Option<i64>> {
        if metadata_provider == MetadataProvider::Local {
            Ok(Some(metadata_id.parse().unwrap()))
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
    ) -> sqlx::Result<Option<i64>> {
        if metadata_provider == MetadataProvider::Local {
            Ok(Some(metadata_id.parse().unwrap()))
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
            metadata_provider: MetadataProvider,
            metadata_id: String,
        }
        let mut local_map = QueryBuilder::new(
            r#"select shows.id, external_ids.metadata_provider, external_ids.metadata_id from external_ids
            join shows on shows.content_id = external_ids.content_id
            where (external_ids.metadata_provider, external_ids.metadata_id) in"#,
        )
            .push_tuples(shows.iter(), |mut b, meta| {
                b.push_bind(meta.metadata_provider.to_string())
                    .push_bind(&meta.metadata_id);
            })
            .build_query_as::<Record>()
            .fetch_all(&self.db.pool).await?
            .into_iter()
            .map(|v| ((v.metadata_provider, v.metadata_id), local_show::LocalShowData { id: v.id }))
            .collect::<HashMap<_, _>>();

        Ok(shows
            .into_iter()
            .map(|meta| {
                let local = local_map.remove(&(meta.metadata_provider, meta.metadata_id.clone()));
                local_show::Show::extend_meta(meta, local)
            })
            .collect())
    }

    pub async fn extend_movies_with_local_data(
        &self,
        movies: Vec<MovieMetadata>,
    ) -> sqlx::Result<Vec<local_movie::Movie>> {
        #[derive(sqlx::FromRow)]
        struct HistoryRecord {}
        #[derive(sqlx::FromRow)]
        struct Record {
            id: i64,
            metadata_provider: MetadataProvider,
            metadata_id: String,
            // history
            history_id: Option<i64>,
            time: Option<i64>,
            is_finished: Option<bool>,
            update_time: Option<time::OffsetDateTime>,
        }
        let mut local_map = QueryBuilder::new(
            r#"select
            movies.id,
            external_ids.metadata_provider, external_ids.metadata_id,
            history.id as history_id, history.time, history.is_finished, history.update_time
            from external_ids
            join movies on movies.content_id = external_ids.content_id
            left join history on history.content_id = movies.content_id
            where (external_ids.metadata_provider, external_ids.metadata_id) in"#,
        )
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
                (v.metadata_provider, v.metadata_id),
                local_movie::LocalMovieData {
                    id: v.id,
                    history: v.history_id.map(|id| History {
                        id,
                        time: v.time.unwrap(),
                        is_finished: v.is_finished.unwrap(),
                        update_time: v.update_time.unwrap(),
                    }),
                },
            )
        })
        .collect::<HashMap<_, _>>();

        Ok(movies
            .into_iter()
            .map(|meta| {
                let local = local_map.remove(&(meta.metadata_provider, meta.metadata_id.clone()));
                local_movie::Movie::extend_meta(meta, local)
            })
            .collect())
    }

    async fn movie_data(
        &self,
        external_provider: MetadataProvider,
        external_id: &str,
    ) -> sqlx::Result<Option<local_movie::LocalMovieData>> {
        let Some(id) = self
            .crossreference_movie(external_provider, external_id)
            .await?
        else {
            return Ok(None);
        };

        Ok(sqlx::query!(
                r#"select movies.id,
            history.id as "history_id?", history.time, history.is_finished, history.update_time as history_update_time from movies
            left join history on history.content_id = movies.content_id
            where movies.id = ? limit 1"#,
                  id
        ).fetch_optional(&self.db.pool).await?.map(|r| local_movie::LocalMovieData {
            id: r.id,
            history: r.history_id.map(|id| api_types::History { id,
                time: r.time.unwrap(),
                is_finished: r.is_finished.unwrap(),
                update_time: r.history_update_time.unwrap()
            })
        }))
    }

    async fn show_data(
        &self,
        external_provider: MetadataProvider,
        external_id: &str,
    ) -> sqlx::Result<Option<local_show::LocalShowData>> {
        self.crossreference_show(external_provider, external_id)
            .await
            .map(|v| v.map(|local_id| local_show::LocalShowData { id: local_id }))
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
            "SELECT id from seasons WHERE seasons.show_id = ? and seasons.number = ?",
            local_id,
            season,
        )
        .fetch_optional(&self.db.pool)
        .await?
        .map(|v| local_show::LocalSeasonData { id: v.id }))
    }

    async fn episode_data(
        &self,
        external_provider: MetadataProvider,
        external_id: &str,
        season: usize,
        episode: usize,
    ) -> sqlx::Result<Option<local_show::LocalEpisodeData>> {
        let season = season as i64;
        let episode = episode as i64;
        let Some(local_id) = self
            .crossreference_show(external_provider, external_id)
            .await?
        else {
            return Ok(None);
        };

        Ok(sqlx::query!(
                r#"select episodes.id as episode_id,
            history.id as "history_id?", history.is_finished, history.time as history_time, history.update_time as history_update_time,
            intros.id as "intro_id?", intros.start_sec as intro_start, intros.end_sec as intro_end
            from episodes
            join seasons on seasons.id = episodes.season_id
            left join intros on intros.episode_id = episodes.id
            left join history on history.content_id = episodes.content_id
            WHERE seasons.show_id = ? and seasons.number = ? and episodes.number = ? limit 1"#,
            local_id,
            season,
            episode,
        )
            .fetch_optional(&self.db.pool)
            .await?
            .map(|r|
                local_show::LocalEpisodeData {
                    id: r.episode_id,
                    history: r.history_id.map(|id| api_types::History {
                        id,
                        time: r.history_time.unwrap(),
                        is_finished: r.is_finished.unwrap(),
                        update_time: r.history_update_time.unwrap(),
                    }),
                    intro: r.intro_start.zip(r.intro_end).map(|(start_sec, end_sec)| Intro { start_sec, end_sec })
                }))
    }
}
