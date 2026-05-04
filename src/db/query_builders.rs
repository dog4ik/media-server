use std::time::Duration;

use crate::{
    api::{
        api_data::{
            api_types::{Actor, History},
            local_actor,
            local_movie::{LocalMovieData, Movie},
            local_show::{Episode, LocalEpisodeData, LocalShowData, Show},
        },
        server::Intro,
    },
    db::{self, DbActor, DbQueryBuilder, DbRole},
    metadata::{ExternalIdMetadata, LocaleMetadata, MetadataProvider},
};

#[derive(sqlx::FromRow, Debug)]
pub struct DbShowQuery {
    pub episode_count: i64,
    pub seasons: String,
    #[sqlx(flatten)]
    pub show: db::DbShow,
    #[sqlx(flatten)]
    pub content: db::DbContent,
    #[sqlx(json, default, nullish)]
    pub cast: Option<Vec<CastQueryJson>>,
    #[sqlx(json, default, nullish)]
    pub external_ids: Option<Vec<ExternalIdsQueryJson>>,
}

impl DbShowQuery {
    pub fn build(builder: &mut DbQueryBuilder) {
        builder.push(format_args!(
            "select {show}, {content}, {cast}, {external_ids},
            (select count(episodes.id) from episodes join seasons on episodes.season_id = seasons.id where seasons.show_id = shows.id) as episode_count,
            (select group_concat(seasons.number) from seasons where seasons.show_id = shows.id) as seasons
            from shows
            join content on content.id = shows.content_id
            join seasons on seasons.show_id = shows.id
            ",
            show = db::DbShow::SQL,
            content = db::DbContent::SQL,
            cast = CastQueryJson::SQL_JSON_AGGR,
            external_ids = ExternalIdsQueryJson::SQL_JSON_AGGR,
        ));
    }
}

impl From<DbShowQuery> for Show {
    fn from(
        DbShowQuery {
            episode_count,
            seasons,
            show,
            content,
            cast,
            external_ids,
        }: DbShowQuery,
    ) -> Self {
        let locale_metadata = content.original_language.zip(content.original_title).map(
            |(original_language, original_title)| LocaleMetadata {
                original_language,
                original_title,
            },
        );

        let mut seasons: Vec<_> = seasons.split(',').filter_map(|x| x.parse().ok()).collect();
        seasons.sort_unstable();
        Show {
            metadata_id: show.id.unwrap().to_string(),
            metadata_provider: MetadataProvider::Local,
            poster: content.poster,
            backdrop: show.backdrop,
            plot: content.plot,
            episodes_amount: Some(episode_count as usize),
            seasons: Some(seasons),
            release_date: content.release_date,
            title: content.title,
            locale_metadata,
            cast: cast.map(|c| c.into_iter().map(Into::into).collect()),
            external_ids: external_ids.map(|ids| ids.into_iter().map(Into::into).collect()),
            local: Some(LocalShowData {
                id: show.id.unwrap(),
            }),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct CastQueryJson {
    pub id: i64,
    pub name: String,
    pub poster: Option<String>,
    pub character: Option<String>,
    pub imdb_id: Option<String>,
    pub metadata_provider: MetadataProvider,
    pub metadata_id: String,
}

impl CastQueryJson {
    pub const SQL_JSON_AGGR: &str = "coalesce((select json_group_array(json_object(
'id', actors.id,
'name', actors.name,
'poster', actors.poster,
'character', roles.character,
'imdb_id', actors.imdb_id,
'metadata_provider', actors.metadata_provider,
'metadata_id', actors.metadata_id
))
from roles
join actors on actors.id = roles.actor_id
where roles.content_id = content.id
having count(actors.id) > 0), json('null')) as cast ";
}

impl From<CastQueryJson> for Actor {
    fn from(
        CastQueryJson {
            id,
            name,
            poster,
            character,
            imdb_id,
            metadata_provider,
            metadata_id,
        }: CastQueryJson,
    ) -> Self {
        Self {
            name,
            poster,
            metadata_id: metadata_id.to_string(),
            metadata_provider,
            imdb_id,
            character,
            local: Some(local_actor::LocalActorData { id }),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct ExternalIdsQueryJson {
    pub id: i64,
    pub metadata_provider: MetadataProvider,
    pub metadata_id: String,
    pub is_prime: i64,
}

impl ExternalIdsQueryJson {
    pub const SQL_JSON_AGGR: &str = "coalesce((select json_group_array(json_object(
'id', external_ids.id,
'metadata_provider', external_ids.metadata_provider,
'metadata_id', external_ids.metadata_id,
'is_prime', external_ids.is_prime
))
from external_ids
where external_ids.content_id = content.id
having count(external_ids.id) > 0), json('null')) as external_ids ";
}

impl From<ExternalIdsQueryJson> for ExternalIdMetadata {
    fn from(
        ExternalIdsQueryJson {
            metadata_provider,
            metadata_id,
            ..
        }: ExternalIdsQueryJson,
    ) -> Self {
        Self {
            provider: metadata_provider,
            id: metadata_id,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct DbMovieQuery {
    #[sqlx(flatten)]
    pub movie: db::DbMovie,
    #[sqlx(flatten)]
    pub content: db::DbContent,
    #[sqlx(flatten, default)]
    pub history: db::DbHistory,
    #[sqlx(json, default, nullish)]
    pub cast: Option<Vec<CastQueryJson>>,
    #[sqlx(json, default, nullish)]
    pub external_ids: Option<Vec<ExternalIdsQueryJson>>,
}

impl From<DbMovieQuery> for Movie {
    fn from(
        DbMovieQuery {
            movie,
            content,
            history,
            cast,
            external_ids,
        }: DbMovieQuery,
    ) -> Self {
        Self {
            metadata_id: movie.id.unwrap().to_string(),
            metadata_provider: MetadataProvider::Local,
            poster: content.poster,
            backdrop: movie.backdrop,
            plot: content.plot,
            release_date: content.release_date,
            runtime: Some(Duration::from_secs(movie.duration as u64).into()),
            title: content.title,
            cast: cast.map(|c| c.into_iter().map(Into::into).collect()),
            external_ids: external_ids.map(|ids| ids.into_iter().map(Into::into).collect()),
            locale_metadata: content.original_title.zip(content.original_language).map(
                |(original_title, original_language)| LocaleMetadata {
                    original_title,
                    original_language,
                },
            ),
            local: Some(LocalMovieData {
                id: movie.id.unwrap(),
                history: history.id.map(|id| History {
                    id,
                    time: history.time,
                    is_finished: history.is_finished,
                    update_time: history.update_time.unwrap(),
                }),
            }),
        }
    }
}

impl DbMovieQuery {
    pub fn build(builder: &mut DbQueryBuilder) {
        builder.push(format_args!(
            "select {content}, {history}, {movie}, {actors}, {external_ids}
            from movies
            join content on content.id = movies.content_id
            join actors on actors.id = movies.content_id
            left join history on history.content_id = content.id",
            content = db::DbContent::SQL,
            history = db::DbHistory::SQL,
            movie = db::DbMovie::SQL,
            actors = CastQueryJson::SQL_JSON_AGGR,
            external_ids = ExternalIdsQueryJson::SQL_JSON_AGGR,
        ));
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct DbHistoryQuery {
    #[sqlx(flatten)]
    pub content: db::DbContent,
    #[sqlx(flatten)]
    pub history: db::DbHistory,
    #[sqlx(flatten, default)]
    pub episode: db::DbEpisode,
    #[sqlx(default)]
    pub show_id: i64,
    #[sqlx(default)]
    pub season_number: i64,
    #[sqlx(default)]
    pub show_title: String,
    #[sqlx(flatten, default)]
    pub movie: db::DbMovie,
    pub runtime: i64,
}

impl DbHistoryQuery {
    pub fn build(cursor: Option<i64>, limit: i64, builder: &mut DbQueryBuilder) {
        builder.push(format_args!(
            "select {content}, {history}, {movie}, {episode},
            coalesce(episodes.duration, movies.duration) as runtime,
            seasons.show_id, seasons.number as season_number, show_content.title as show_title
            from history
            join content on content.id = history.content_id
            left join movies on movies.content_id = content.id
            left join episodes on episodes.content_id = content.id
            left join seasons on seasons.id = episodes.season_id
            left join shows on shows.id = seasons.show_id
            left join content as show_content on show_content.id = shows.content_id ",
            content = db::DbContent::SQL,
            history = db::DbHistory::SQL,
            movie = db::DbMovie::SQL,
            episode = db::DbEpisode::SQL,
        ));
        if let Some(cursor) = cursor {
            builder
                .push("where history.update_time < datetime(")
                .push_bind(cursor)
                .push(", 'unixepoch') ");
        }
        builder
            .push("order by history.update_time desc limit ")
            .push_bind(limit);
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct DbActorsQuery {
    #[sqlx(flatten)]
    pub actor: db::DbActor,
}

impl DbActorsQuery {
    pub fn build(builder: &mut DbQueryBuilder<'_>) {
        builder.push(format_args!(
            "select {actor} from actors",
            actor = DbActor::SQL,
        ));
    }
}

impl From<DbActorsQuery> for Actor {
    fn from(DbActorsQuery { actor }: DbActorsQuery) -> Self {
        Self {
            metadata_id: actor.metadata_id,
            metadata_provider: actor.metadata_provider,
            local: Some(local_actor::LocalActorData {
                id: actor.id.unwrap(),
            }),
            name: actor.name,
            poster: actor.poster,
            imdb_id: actor.imdb_id,
            character: None,
        }
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct DbEpisodeQuery {
    season_number: i64,
    #[sqlx(flatten)]
    pub episode: db::DbEpisode,
    #[sqlx(flatten)]
    pub content: db::DbContent,
    #[sqlx(json, default, nullish)]
    pub cast: Option<Vec<CastQueryJson>>,
    #[sqlx(flatten, default)]
    pub history: db::DbHistory,
    #[sqlx(flatten, default)]
    pub intro: db::DbIntro,
}

impl DbEpisodeQuery {
    pub fn build(builder: &mut DbQueryBuilder) {
        builder.push(format_args!(
            "select {episode}, {content}, {history}, {intro}, {cast},
            seasons.number as season_number
            from episodes
            join content on content.id = episodes.content_id
            left join history on history.content_id = episodes.content_id
            left join intros on intros.episode_id = episodes.id
            join seasons on seasons.show_id = episodes.season_id
            ",
            episode = db::DbEpisode::SQL,
            content = db::DbContent::SQL,
            history = db::DbHistory::SQL,
            intro = db::DbIntro::SQL,
            cast = CastQueryJson::SQL_JSON_AGGR,
        ));
    }
}

impl From<DbEpisodeQuery> for Episode {
    fn from(
        DbEpisodeQuery {
            season_number,
            episode,
            content,
            cast,
            history,
            intro,
        }: DbEpisodeQuery,
    ) -> Self {
        let locale_metadata = content.original_language.zip(content.original_title).map(
            |(original_language, original_title)| LocaleMetadata {
                original_language,
                original_title,
            },
        );

        Episode {
            metadata_id: episode.id.unwrap().to_string(),
            metadata_provider: MetadataProvider::Local,
            poster: content.poster,
            plot: content.plot,
            release_date: content.release_date,
            title: content.title,
            number: episode.number as usize,
            runtime: Some(std::time::Duration::from_secs(episode.duration as u64).into()),
            cast: cast.map(|c| c.into_iter().map(Into::into).collect()),
            season_number: season_number as usize,
            local: Some(LocalEpisodeData {
                id: episode.id.unwrap(),
                intro: intro.id.map(|_| Intro {
                    start_sec: intro.start_sec,
                    end_sec: intro.end_sec,
                }),
                history: history.id.map(|id| History {
                    id,
                    time: history.time,
                    is_finished: history.is_finished,
                    update_time: history.update_time.unwrap().into(),
                }),
            }),
        }
    }
}
