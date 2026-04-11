use std::collections::HashMap;

use serde::Serialize;

use crate::{
    api::{
        api_data::{
            LocalDataLookup,
            api_types::{Actor, History},
        },
        server::Intro,
    },
    metadata::{EpisodeMetadata, LocaleMetadata, MetadataProvider, SeasonMetadata, ShowMetadata},
};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalShowData {
    pub id: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalSeasonData {
    pub id: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalEpisodeData {
    pub id: i64,
    pub history: Option<super::api_types::History>,
    pub intro: Option<Intro>,
}

/// Show API data structure
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Show {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub plot: Option<String>,
    /// Array of available season numbers
    pub seasons: Option<Vec<usize>>,
    pub episodes_amount: Option<usize>,
    pub release_date: Option<String>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
    pub cast: Option<Vec<Actor>>,
    pub local: Option<LocalShowData>,
}

impl Show {
    pub async fn extend_with_lookup(
        meta: ShowMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        let local = lookup
            .show_data(meta.metadata_provider, &meta.metadata_id)
            .await?;

        Ok(Self::extend_meta(meta, local))
    }

    pub fn extend_meta(meta: ShowMetadata, local: Option<LocalShowData>) -> Self {
        Self {
            metadata_id: meta.metadata_id,
            metadata_provider: meta.metadata_provider,
            poster: meta.poster,
            backdrop: meta.backdrop,
            plot: meta.plot,
            seasons: meta.seasons,
            episodes_amount: meta.episodes_amount,
            release_date: meta.release_date,
            title: meta.title,
            locale_metadata: meta.locale_metadata,
            cast: None,
            local,
        }
    }
}

/// Season API data structure
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Season {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub release_date: Option<String>,
    pub title: Option<String>,
    pub episodes: Vec<Episode>,
    pub plot: Option<String>,
    pub poster: Option<String>,
    pub number: usize,
    pub local: Option<LocalSeasonData>,
}

impl Season {
    pub async fn extend_from_metadata(
        meta: SeasonMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        let local = lookup
            .season_data(meta.metadata_provider, &meta.metadata_id, meta.number)
            .await?;
        let mut episodes = Vec::with_capacity(meta.episodes.len());
        #[derive(sqlx::FromRow)]
        struct Record {
            id: i64,
            history_id: Option<i64>,
            time: Option<i64>,
            update_time: Option<time::OffsetDateTime>,
            is_finished: Option<bool>,
            metadata_provider: MetadataProvider,
            metadata_id: String,
            intro_id: Option<i64>,
            start_sec: Option<i64>,
            end_sec: Option<i64>,
        }
        let mut local_episodes = sqlx::QueryBuilder::new(
            "select episodes.id,
            external_ids.metadata_id, external_ids.metadata_provider,
            history.id as history_id, history.time, history.update_time, history.is_finished,
            intros.id as intro_id, intros.start_sec, intros.end_sec
            from external_ids
            join episodes on episodes.content_id = external_ids.content_id
            left join intros on intros.episode_id = episodes.id
            left join history on history.content_id = episodes.content_id
            where (external_ids.metadata_provider, external_ids.metadata_id) in",
        )
        .push_tuples(meta.episodes.iter(), |mut b, meta| {
            b.push_bind(meta.metadata_provider)
                .push_bind(&meta.metadata_id);
        })
        .build_query_as::<Record>()
        .fetch_all(&lookup.db.pool)
        .await?
        .into_iter()
        .map(|r| {
            (
                (r.metadata_provider, r.metadata_id),
                LocalEpisodeData {
                    id: r.id,
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
        for episode_meta in meta.episodes {
            let local_episode = Episode {
                metadata_id: episode_meta.metadata_id.clone(),
                metadata_provider: episode_meta.metadata_provider,
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
            };
            episodes.push(local_episode);
        }

        Ok(Self {
            metadata_id: meta.metadata_id,
            metadata_provider: meta.metadata_provider,
            release_date: meta.release_date,
            title: meta.title,
            episodes,
            plot: meta.plot,
            poster: meta.poster,
            number: meta.number,
            local,
        })
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Episode {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub release_date: Option<String>,
    pub number: usize,
    pub title: String,
    pub plot: Option<String>,
    pub season_number: usize,
    pub runtime: Option<crate::MediaDuration>,
    pub poster: Option<String>,
    pub cast: Option<Vec<Actor>>,
    pub local: Option<LocalEpisodeData>,
}

impl Episode {
    pub async fn extend_from_metadata(
        mut meta: EpisodeMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        let local = lookup
            .episode_data(
                meta.metadata_provider,
                &meta.metadata_id,
                meta.season_number,
                meta.number,
            )
            .await?;
        let cast = if let Some(cast) = std::mem::take(&mut meta.cast) {
            Some(lookup.extend_actors(cast).await?)
        } else {
            None
        };
        Ok(Episode {
            metadata_id: meta.metadata_id,
            metadata_provider: meta.metadata_provider,
            release_date: meta.release_date,
            number: meta.number,
            title: meta.title,
            plot: meta.plot,
            season_number: meta.season_number,
            runtime: meta.runtime,
            poster: meta.poster,
            cast,
            local,
        })
    }
}
