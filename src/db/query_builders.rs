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
    db::{self, DbActor, DbQueryBuilder},
    metadata::{ExternalIdMetadata, Genre, LocaleMetadata, MetadataProvider},
};

#[derive(sqlx::FromRow, Debug)]
pub struct DbShowQuery {
    pub episode_count: i64,
    pub seasons: String,
    #[sqlx(flatten)]
    pub show: db::DbShow,
    #[sqlx(flatten)]
    pub metadata: db::DbMetadata,
    #[sqlx(json, default, nullish)]
    pub cast: Option<Vec<CastQueryJson>>,
    #[sqlx(json, default, nullish)]
    pub external_ids: Option<Vec<ExternalIdsQueryJson>>,
    #[sqlx(json, default, nullish)]
    pub genres: Option<Vec<GenreQueryJson>>,
}

impl DbShowQuery {
    pub fn build(builder: &mut DbQueryBuilder) {
        builder.push(format_args!(
            "select {show}, {metadata}, {cast}, {external_ids}, {genres},
            (select count(episodes.id) from episodes join seasons on episodes.season_id = seasons.id where seasons.show_id = shows.id) as episode_count,
            (select group_concat(seasons.number) from seasons where seasons.show_id = shows.id) as seasons
            from shows
            join metadata on metadata.id = shows.metadata_id",
            show = db::DbShow::SQL,
            metadata = db::DbMetadata::SQL,
            cast = CastQueryJson::SQL_JSON_AGGR,
            external_ids = ExternalIdsQueryJson::SQL_JSON_AGGR,
            genres = GenreQueryJson::SQL_JSON_AGGR,
        ));
    }
}

impl From<DbShowQuery> for Show {
    fn from(
        DbShowQuery {
            episode_count,
            seasons,
            show,
            metadata,
            cast,
            external_ids,
            genres,
        }: DbShowQuery,
    ) -> Self {
        let locale_metadata = metadata.original_language.zip(metadata.original_title).map(
            |(original_language, original_title)| LocaleMetadata {
                original_language,
                original_title,
            },
        );

        let mut seasons: Vec<_> = seasons.split(',').filter_map(|x| x.parse().ok()).collect();
        seasons.sort_unstable();
        Show {
            provider_id: show.id.unwrap().to_string(),
            provider: MetadataProvider::Local,
            poster: metadata.poster,
            backdrop: show.backdrop,
            plot: metadata.plot,
            episodes_amount: Some(episode_count as usize),
            seasons: Some(seasons),
            release_date: metadata.release_date,
            title: metadata.title,
            locale_metadata,
            cast: cast.map(|c| c.into_iter().map(Into::into).collect()),
            external_ids: external_ids.map(|ids| ids.into_iter().map(Into::into).collect()),
            genres: genres.map(|gs| {
                gs.into_iter()
                    .filter_map(|g| Genre::try_from(g).ok())
                    .collect()
            }),
            local: Some(LocalShowData {
                id: show.id.unwrap(),
                metadata_id: metadata.id.unwrap(),
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
    pub external_metadata_provider: MetadataProvider,
    pub external_metadata_id: String,
}

impl CastQueryJson {
    pub const SQL_JSON_AGGR: &str = "coalesce((select json_group_array(json_object(
'id', actors.id,
'name', actors.name,
'poster', actors.poster,
'character', roles.character,
'imdb_id', actors.imdb_id,
'external_metadata_provider', actors.external_metadata_provider,
'external_metadata_id', actors.external_metadata_id
))
from roles
join actors on actors.id = roles.actor_id
where roles.metadata_id = metadata.id
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
            external_metadata_provider,
            external_metadata_id,
        }: CastQueryJson,
    ) -> Self {
        Self {
            name,
            poster,
            metadata_id: external_metadata_id,
            metadata_provider: external_metadata_provider,
            imdb_id,
            character,
            local: Some(local_actor::LocalActorData { id }),
        }
    }
}

#[derive(Debug, serde::Deserialize)]
pub struct ExternalIdsQueryJson {
    pub id: i64,
    pub external_provider: MetadataProvider,
    pub external_id: String,
    pub is_prime: i64,
}

impl ExternalIdsQueryJson {
    pub const SQL_JSON_AGGR: &str = "coalesce((select json_group_array(json_object(
'id', external_ids.id,
'external_provider', external_ids.external_provider,
'external_id', external_ids.external_id,
'is_prime', external_ids.is_prime
))
from external_ids
where external_ids.metadata_id = metadata.id
having count(external_ids.id) > 0), json('null')) as external_ids ";
}

impl From<ExternalIdsQueryJson> for ExternalIdMetadata {
    fn from(
        ExternalIdsQueryJson {
            external_provider,
            external_id,
            ..
        }: ExternalIdsQueryJson,
    ) -> Self {
        Self {
            provider: external_provider,
            id: external_id,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
#[serde(transparent)]
pub struct GenreQueryJson {
    pub genre_id: i64,
}

impl GenreQueryJson {
    pub const SQL_JSON_AGGR: &str = "coalesce((select json_group_array(content_genres.genre_id) \
from content_genres \
where content_genres.metadata_id = metadata.id \
having count(content_genres.id) > 0), json('null')) as genres ";
}

impl TryFrom<GenreQueryJson> for Genre {
    type Error = anyhow::Error;
    fn try_from(g: GenreQueryJson) -> Result<Self, Self::Error> {
        Genre::try_from(g.genre_id)
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct DbMovieQuery {
    #[sqlx(flatten)]
    pub movie: db::DbMovie,
    #[sqlx(flatten)]
    pub metadata: db::DbMetadata,
    #[sqlx(flatten, default)]
    pub history: db::DbHistory,
    #[sqlx(json, default, nullish)]
    pub cast: Option<Vec<CastQueryJson>>,
    #[sqlx(json, default, nullish)]
    pub external_ids: Option<Vec<ExternalIdsQueryJson>>,
    #[sqlx(json, default, nullish)]
    pub genres: Option<Vec<GenreQueryJson>>,
}

impl From<DbMovieQuery> for Movie {
    fn from(
        DbMovieQuery {
            movie,
            metadata,
            history,
            cast,
            external_ids,
            genres,
        }: DbMovieQuery,
    ) -> Self {
        Self {
            provider_id: movie.id.unwrap().to_string(),
            provider: MetadataProvider::Local,
            poster: metadata.poster,
            backdrop: movie.backdrop,
            plot: metadata.plot,
            release_date: metadata.release_date,
            runtime: Some(Duration::from_secs(movie.duration as u64).into()),
            title: metadata.title,
            cast: cast.map(|c| c.into_iter().map(Into::into).collect()),
            external_ids: external_ids.map(|ids| ids.into_iter().map(Into::into).collect()),
            genres: genres.map(|gs| {
                gs.into_iter()
                    .filter_map(|g| Genre::try_from(g).ok())
                    .collect()
            }),
            locale_metadata: metadata.original_title.zip(metadata.original_language).map(
                |(original_title, original_language)| LocaleMetadata {
                    original_title,
                    original_language,
                },
            ),
            local: Some(LocalMovieData {
                id: movie.id.unwrap(),
                metadata_id: metadata.id.unwrap(),
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
            "select {metadata}, {history}, {movie}, {actors}, {external_ids}, {genres}
            from movies
            join metadata on metadata.id = movies.metadata_id
            left join history on history.metadata_id = metadata.id",
            metadata = db::DbMetadata::SQL,
            history = db::DbHistory::SQL,
            movie = db::DbMovie::SQL,
            actors = CastQueryJson::SQL_JSON_AGGR,
            external_ids = ExternalIdsQueryJson::SQL_JSON_AGGR,
            genres = GenreQueryJson::SQL_JSON_AGGR,
        ));
    }
}

#[derive(Debug, sqlx::FromRow)]
pub struct DbHistoryQuery {
    #[sqlx(flatten)]
    pub metadata: db::DbMetadata,
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
            "select {metadata}, {history}, {movie}, {episode},
            coalesce(episodes.duration, movies.duration) as runtime,
            seasons.show_id, seasons.number as season_number, show_metadata.title as show_title
            from history
            join metadata on metadata.id = history.metadata_id
            left join movies on movies.metadata_id = metadata.id
            left join episodes on episodes.metadata_id = metadata.id
            left join seasons on seasons.id = episodes.season_id
            left join shows on shows.id = seasons.show_id
            left join metadata as show_metadata on show_metadata.id = shows.metadata_id ",
            metadata = db::DbMetadata::SQL,
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
            metadata_id: actor.external_metadata_id,
            metadata_provider: actor.external_metadata_provider,
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
    pub metadata: db::DbMetadata,
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
            "select {episode}, {metadata}, {history}, {intro}, {cast},
            seasons.number as season_number
            from episodes
            join metadata on metadata.id = episodes.metadata_id
            left join history on history.metadata_id = episodes.metadata_id
            left join intros on intros.episode_id = episodes.id
            join seasons on seasons.id = episodes.season_id
            ",
            episode = db::DbEpisode::SQL,
            metadata = db::DbMetadata::SQL,
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
            metadata,
            cast,
            history,
            intro,
        }: DbEpisodeQuery,
    ) -> Self {
        Episode {
            provider_id: episode.id.unwrap().to_string(),
            provider: MetadataProvider::Local,
            poster: metadata.poster,
            plot: metadata.plot,
            release_date: metadata.release_date,
            title: metadata.title,
            number: episode.number as usize,
            runtime: Some(std::time::Duration::from_secs(episode.duration as u64).into()),
            cast: cast.map(|c| c.into_iter().map(Into::into).collect()),
            season_number: season_number as usize,
            local: Some(LocalEpisodeData {
                id: episode.id.unwrap(),
                metadata_id: metadata.id.unwrap(),
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
