use crate::{
    api::api_data::local_show::{LocalShowData, Show},
    db::{self, DbHistory, DbQueryBuilder},
    metadata::{LocaleMetadata, MetadataImage, MetadataProvider},
};

#[derive(sqlx::FromRow, Debug)]
pub struct DbShowData {
    pub id: i64,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub plot: Option<String>,
    pub episode_count: i64,
    pub seasons: String,
    pub release_date: Option<String>,
    pub title: String,
    pub original_language: Option<String>,
    pub original_title: Option<String>,
}

impl From<DbShowData> for Show {
    fn from(show: DbShowData) -> Self {
        let poster = show.poster.map(|p| MetadataImage::new(p.parse().unwrap()));
        let backdrop = show
            .backdrop
            .map(|b| MetadataImage::new(b.parse().unwrap()));
        let locale_metadata = show.original_language.zip(show.original_title).map(
            |(original_language, original_title)| LocaleMetadata {
                original_language,
                original_title,
            },
        );

        let mut seasons: Vec<_> = show
            .seasons
            .split(',')
            .filter_map(|x| x.parse().ok())
            .collect();
        seasons.sort_unstable();
        Show {
            metadata_id: show.id.to_string(),
            metadata_provider: MetadataProvider::Local,
            poster,
            backdrop,
            plot: show.plot,
            episodes_amount: Some(show.episode_count as usize),
            seasons: Some(seasons),
            release_date: show.release_date,
            title: show.title,
            locale_metadata,
            local: Some(LocalShowData { id: show.id }),
        }
    }
}

impl DbShowData {
    pub const SQL_SELECT: &str = " shows.id, shows.backdrop,
content.title, content.plot, content.poster, content.release_date,
content.original_language, content.original_title,
(SELECT GROUP_CONCAT(seasons.number) FROM seasons WHERE seasons.show_id = shows.id) as seasons,
(SELECT COUNT(episodes.id) FROM episodes JOIN seasons ON episodes.season_id = seasons.id WHERE seasons.show_id = shows.id) as episode_count ";
}

#[derive(Debug)]
pub struct DbMovieData {
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    pub runtime: Option<String>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
    pub history: Option<DbHistory>,
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
