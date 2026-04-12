use std::{ops::Deref, path::Path, str::FromStr, time::Duration};

use serde::Serialize;
use sqlx::{
    Acquire, Error, Execute, FromRow, Pool, QueryBuilder, Sqlite, SqliteConnection, SqlitePool,
    Transaction,
    migrate::{MigrateError, Migrator},
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use torrent::DownloadParams;

use crate::{
    api::{
        api_data::{
            api_types::{self, History},
            local_movie::{LocalMovieData, Movie},
            local_show::{Episode, LocalEpisodeData, LocalSeasonData, LocalShowData, Season, Show},
        },
        server::Intro,
    },
    app_state::AppError,
    config,
    db::query_builders::{DbEpisodeQuery, DbMovieQuery},
    library::assets::{self, AssetDir},
    metadata::{
        ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, FetchParams,
        LocaleMetadata, MetadataProvider, MovieMetadata, ShowMetadata,
    },
};

pub mod query_builders;

fn path_to_url(path: &Path) -> String {
    #[allow(unused_mut)]
    let mut path = path.to_string_lossy().to_string();
    #[cfg(target_os = "windows")]
    {
        let stupid_pattern = r#"\\?"#;
        if path.starts_with(stupid_pattern) {
            path = path.replace(stupid_pattern, "");
        };
        path = path.replace(r#"\"#, "/")
    }
    format!("sqlite://{}", path)
}

#[derive(Debug, Default)]
pub struct ContentFetchParams {
    pub take: Option<i64>,
    pub cursor: Option<String>,
    pub search: Option<String>,
    /// List of actor id's
    pub actors: Option<Vec<i64>>,
}

impl ContentFetchParams {
    pub fn build<'a>(&'a self, content_type: DbContentType, builder: &mut DbQueryBuilder<'a>) {
        let content_table = content_type.table_name();
        if let Some(actors) = &self.actors {
            builder
                .push(" join roles on roles.content_id = content.id ")
                .push("where roles.actor_id in (");
            let mut separated = builder.separated(", ");
            for actor in actors {
                separated.push_bind(actor);
            }
            builder.push(")");
        }

        if let Some(cursor) = &self.cursor {
            if self.actors.is_none() {
                builder.push(" where ");
            } else {
                builder.push(" and ");
            }
            builder
                .push(format_args!("{content_table}.id < "))
                .push_bind(cursor);
        }

        if let Some(search) = &self.search {
            if self.actors.is_none() && self.cursor.is_none() {
                builder.push(" where ");
            } else {
                builder.push(" and ");
            }
            builder
                .push("content.title like ")
                .push_bind(format!("%{search}%"));
        }

        builder
            .push(&format_args!(" order by {content_table}.id desc limit "))
            .push_bind(self.take.unwrap_or(50));
    }
}

pub const DEFAULT_LIMIT: i64 = 50;

/// All database queries and mutations
// NOTE: This might not be the best way to share queries between `Pool`, `Transaction`, and `Connection`,
// but it's the best I could come up with.
pub trait DbActions<'a>: Acquire<'a, Database = Sqlite> + Send
where
    Self: Sized,
{
    fn clear(self) -> impl std::future::Future<Output = Result<(), sqlx::Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            sqlx::query!(
                "
        DELETE FROM content;
        DELETE FROM videos;
        DELETE FROM subtitles;
        ",
            )
            .execute(&mut *conn)
            .await?;
            Ok(())
        }
    }

    /// Insert a movie.
    fn insert_movie(
        self,
        movie: &DbMovie,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let movie_id = sqlx::query!(
                "INSERT INTO movies (content_id, backdrop, duration) VALUES (?, ?, ?) RETURNING id;",
                movie.content_id,
                movie.backdrop,
                movie.duration,
            )
            .fetch_one(&mut *conn)
            .await?
            .id;
            Ok(movie_id)
        }
    }

    /// Insert a show.
    fn insert_content(
        self,
        content: &DbContent,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let content_id = sqlx::query!(
                "INSERT INTO content (content_type, title, release_date, poster, plot, original_language, original_title)
                VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id;",
                content.content_type,
                content.title,
                content.release_date,
                content.poster,
                content.plot,
                content.original_language,
                content.original_title,
            )
            .fetch_one(&mut *conn)
            .await?
            .id;
            Ok(content_id)
        }
    }

    /// Insert a show.
    fn insert_show(
        self,
        show: &DbShow,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let show_id = sqlx::query!(
                "INSERT INTO shows (content_id, backdrop) VALUES (?, ?) RETURNING id;",
                show.content_id,
                show.backdrop,
            )
            .fetch_one(&mut *conn)
            .await?
            .id;
            Ok(show_id)
        }
    }

    /// Insert a season.
    fn insert_season(
        self,
        season: DbSeason,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let season_id = sqlx::query!(
                "INSERT INTO seasons (show_id, number, content_id) VALUES (?, ?, ?) RETURNING id;",
                season.show_id,
                season.number,
                season.content_id,
            )
            .fetch_one(&mut *conn)
            .await?
            .id;
            Ok(season_id)
        }
    }

    /// Insert an episode. Returns episode id.
    fn insert_episode(
        self,
        episode: &DbEpisode,
    ) -> impl std::future::Future<Output = sqlx::Result<i64>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let episode_id = sqlx::query!(
                "INSERT INTO episodes (season_id, number, duration, content_id)
                VALUES (?, ?, ?, ?) RETURNING id;",
                episode.season_id,
                episode.number,
                episode.duration,
                episode.content_id,
            )
            .fetch_one(&mut *conn)
            .await?
            .id;
            Ok(episode_id)
        }
    }

    /// Insert an actor. Returns actor id.
    fn insert_actor(
        self,
        actor: &DbActor,
    ) -> impl std::future::Future<Output = sqlx::Result<i64>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            sqlx::query_scalar!(
                "INSERT INTO actors (name, metadata_id, metadata_provider, imdb_id, poster)
                VALUES (?, ?, ?, ?, ?) RETURNING id;",
                actor.name,
                actor.metadata_id,
                actor.metadata_provider,
                actor.imdb_id,
                actor.poster,
            )
            .fetch_one(&mut *conn)
            .await
        }
    }

    /// Insert a role. Returns role id.
    fn insert_role(
        self,
        role: &DbRole,
    ) -> impl std::future::Future<Output = sqlx::Result<i64>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            sqlx::query_scalar!(
                "INSERT INTO roles (actor_id, content_id, character)
                VALUES (?, ?, ?) RETURNING id;",
                role.actor_id,
                role.content_id,
                role.character,
            )
            .fetch_one(&mut *conn)
            .await
        }
    }

    fn insert_video(
        self,
        db_video: DbVideo,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::debug!("Inserting new video: {}", db_video.path);
            let video_query = sqlx::query!(
                "INSERT INTO videos
            (path, size, content_id, is_prime)
            VALUES (?, ?, ?, ?) RETURNING id;",
                db_video.path,
                db_video.size,
                db_video.content_id,
                db_video.is_prime,
            );
            video_query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_subtitles(
        self,
        db_subtitles: &DbSubtitles,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let subtitles_query = sqlx::query!(
                "INSERT INTO subtitles
            (language, external_path, video_id)
            VALUES (?, ?, ?) RETURNING id;",
                db_subtitles.language,
                db_subtitles.external_path,
                db_subtitles.video_id
            );
            subtitles_query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_history(
        self,
        db_history: DbHistory,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let history_query = sqlx::query!(
                "INSERT INTO history
            (time, is_finished, content_id, update_time)
            VALUES (?, ?, ?, ?) RETURNING id;",
                db_history.time,
                db_history.is_finished,
                db_history.content_id,
                db_history.update_time,
            );
            history_query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_external_id(
        self,
        db_external_id: DbExternalId,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = sqlx::query_scalar!(
                "INSERT INTO external_ids
            (metadata_provider, metadata_id, content_id, is_prime)
            VALUES (?, ?, ?, ?) RETURNING id;",
                db_external_id.metadata_provider,
                db_external_id.metadata_id,
                db_external_id.content_id,
                db_external_id.is_prime,
            );
            query.fetch_one(&mut *conn).await
        }
    }

    fn insert_intro(
        self,
        intro: DbIntro,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::trace!("Inserting intro for episode {}", intro.episode_id);
            let query = sqlx::query!(
                "INSERT OR REPLACE INTO intros
            (episode_id, start_sec, end_sec)
            VALUES (?, ?, ?) RETURNING id;",
                intro.episode_id,
                intro.start_sec,
                intro.end_sec,
            );
            query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_torrent(
        self,
        torrent: DbTorrent,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = sqlx::query!(
                "INSERT INTO torrents
            (info_hash, bitfield, trackers, save_location, bencoded_info)
            VALUES (?, ?, ?, ?, ?) RETURNING id;",
                torrent.info_hash,
                torrent.bitfield,
                torrent.trackers,
                torrent.save_location,
                torrent.bencoded_info,
            );

            query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_torrent_file(
        self,
        file: DbTorrentFile,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = sqlx::query!(
                "INSERT INTO torrent_files
            (torrent_id, relative_path, priority, idx)
            VALUES (?, ?, ?, ?) RETURNING id;",
                file.torrent_id,
                file.relative_path,
                file.priority,
                file.idx,
            );

            query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn get_external_ids(
        self,
        content_id: i64,
        content_hint: ContentType,
    ) -> impl std::future::Future<Output = Result<Vec<ExternalIdMetadata>, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;

            let db_ids = match content_hint {
                ContentType::Movie => {
                    sqlx::query_as!(
                        DbExternalId,
                        "SELECT external_ids.* FROM external_ids
                        JOIN movies ON movies.content_id = external_ids.content_id
                        WHERE movies.id = ?",
                        content_id
                    )
                    .fetch_all(&mut *conn)
                    .await
                }
                ContentType::Show => {
                    sqlx::query_as!(
                        DbExternalId,
                        "SELECT external_ids.* FROM external_ids
                        JOIN shows ON shows.content_id = external_ids.content_id
                        WHERE shows.id = ?",
                        content_id
                    )
                    .fetch_all(&mut *conn)
                    .await
                }
            }?;
            Ok(db_ids.into_iter().map(|i| i.into()).collect())
        }
    }

    fn remove_video(self, id: i64) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::debug!(id, "Removing video");
            let remove_result =
                sqlx::query!("DELETE FROM videos WHERE id = ? RETURNING content_id;", id)
                    .fetch_one(&mut *conn)
                    .await?;

            let Some(content_id) = remove_result.content_id else {
                return Ok(());
            };

            // Check if it was an episode
            if let Some(episode_row) = sqlx::query!(
                "SELECT id, season_id FROM episodes WHERE content_id = ?",
                content_id
            )
            .fetch_optional(&mut *conn)
            .await?
            {
                let sibling_count = sqlx::query!(
                    "SELECT COUNT(*) AS count FROM videos WHERE content_id = ?",
                    content_id
                )
                .fetch_one(&mut *conn)
                .await?
                .count;
                if sibling_count == 0 {
                    conn.remove_episode(episode_row.id).await?;
                }
                return Ok(());
            }

            // Check if it was a movie
            if let Some(movie_row) =
                sqlx::query!("SELECT id FROM movies WHERE content_id = ?", content_id)
                    .fetch_optional(&mut *conn)
                    .await?
            {
                let sibling_count = sqlx::query!(
                    "SELECT COUNT(*) AS count FROM videos WHERE content_id = ?",
                    content_id
                )
                .fetch_one(&mut *conn)
                .await?
                .count;
                if sibling_count == 0 {
                    conn.remove_movie(movie_row.id).await?;
                }
                return Ok(());
            }

            Ok(())
        }
    }

    fn remove_episode(
        self,
        id: i64,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::debug!(id, "Removing episode");
            let episode = sqlx::query!(
                "SELECT season_id, content_id FROM episodes WHERE id = ?",
                id
            )
            .fetch_one(&mut *conn)
            .await?;

            // Deleting content cascades to episodes row + intros + sets videos.content_id to NULL
            sqlx::query!("DELETE FROM content WHERE id = ?", episode.content_id)
                .execute(&mut *conn)
                .await?;

            let siblings_count = sqlx::query!(
                "SELECT COUNT(*) AS count FROM episodes WHERE season_id = ?",
                episode.season_id
            )
            .fetch_one(&mut *conn)
            .await?
            .count;
            tracing::debug!("Removed episode siblings count: {}", siblings_count);
            if siblings_count == 0 {
                conn.remove_season(episode.season_id).await?;
            }

            let episode_assets = assets::EpisodeAssetsDir::new(id);
            if let Err(e) = episode_assets.delete_dir().await {
                tracing::warn!("Failed to clean up episode directory: {e}")
            };
            Ok(())
        }
    }

    fn remove_season(self, id: i64) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::debug!(id, "Removing season");
            let season = sqlx::query!("SELECT show_id, content_id FROM seasons WHERE id = ?", id)
                .fetch_one(&mut *conn)
                .await?;

            // Delete all episode content rows first (cascade: episodes + intros)
            let episode_content_ids: Vec<i64> =
                sqlx::query!("SELECT content_id FROM episodes WHERE season_id = ?", id)
                    .fetch_all(&mut *conn)
                    .await?
                    .into_iter()
                    .map(|r| r.content_id)
                    .collect();

            for content_id in episode_content_ids {
                sqlx::query!("DELETE FROM content WHERE id = ?", content_id)
                    .execute(&mut *conn)
                    .await?;
            }

            // Delete season's content row (cascades to seasons row)
            sqlx::query!("DELETE FROM content WHERE id = ?", season.content_id)
                .execute(&mut *conn)
                .await?;

            let show_id = season.show_id;
            let siblings_count = sqlx::query!(
                "SELECT COUNT(*) AS count FROM seasons WHERE show_id = ?",
                show_id
            )
            .fetch_one(&mut *conn)
            .await?
            .count;
            if siblings_count == 0 {
                conn.remove_show(show_id).await?;
            }

            let season_assets = assets::SeasonAssetsDir::new(id);
            if let Err(e) = season_assets.delete_dir().await {
                tracing::warn!("Failed to clean up season directory: {e}")
            };
            Ok(())
        }
    }

    /// Deletes show and all its seasons/episodes by removing their content rows.
    fn remove_show(self, id: i64) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::debug!(id, "Removing show");

            let show_content_id = sqlx::query!("SELECT content_id FROM shows WHERE id = ?", id)
                .fetch_one(&mut *conn)
                .await?
                .content_id;

            // Collect all episode content_ids
            let episode_content_ids: Vec<i64> = sqlx::query!(
                "SELECT episodes.content_id FROM episodes
                JOIN seasons ON seasons.id = episodes.season_id
                WHERE seasons.show_id = ?",
                id
            )
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|r| r.content_id)
            .collect();

            // Collect all season content_ids
            let season_content_ids: Vec<i64> =
                sqlx::query!("SELECT content_id FROM seasons WHERE show_id = ?", id)
                    .fetch_all(&mut *conn)
                    .await?
                    .into_iter()
                    .map(|r| r.content_id)
                    .collect();

            // Delete episode content rows first (cascade: episodes + intros)
            for content_id in episode_content_ids {
                sqlx::query!("DELETE FROM content WHERE id = ?", content_id)
                    .execute(&mut *conn)
                    .await?;
            }

            // Delete season content rows (cascade: seasons)
            for content_id in season_content_ids {
                sqlx::query!("DELETE FROM content WHERE id = ?", content_id)
                    .execute(&mut *conn)
                    .await?;
            }

            // Delete show content row (cascade: shows)
            sqlx::query!("DELETE FROM content WHERE id = ?", show_content_id)
                .execute(&mut *conn)
                .await?;

            let show_assets = assets::ShowAssetsDir::new(id);
            if let Err(e) = show_assets.delete_dir().await {
                tracing::warn!("Failed to clean up show directory: {e}")
            };

            Ok(())
        }
    }

    fn remove_movie(self, id: i64) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::debug!(id, "Removing movie");
            // Delete via content_id - cascades to movies row
            sqlx::query!(
                "DELETE FROM content WHERE id = (SELECT content_id FROM movies WHERE id = ?)",
                id
            )
            .execute(&mut *conn)
            .await?;

            let movie_assets = assets::MovieAssetsDir::new(id);
            if let Err(e) = movie_assets.delete_dir().await {
                tracing::warn!("Failed to clean up movie directory: {e}")
            };

            Ok(())
        }
    }

    fn remove_intro(self, id: i64) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::debug!(id, "Removing intro");
            let query = sqlx::query!("DELETE FROM intros WHERE id = ?", id);

            query.execute(&mut *conn).await?;
            Ok(())
        }
    }

    fn remove_torrent(
        self,
        info_hash: &[u8],
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = sqlx::query!("DELETE FROM torrents WHERE info_hash = ?", info_hash);
            query.execute(&mut *conn).await?;
            Ok(())
        }
    }

    fn update_subtitles(
        self,
        id: i64,
        subtitles: DbSubtitles,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            sqlx::query!(
                "UPDATE subtitles SET language = ?, video_id = ?, external_path = ? WHERE id = ?",
                subtitles.language,
                subtitles.video_id,
                subtitles.external_path,
                id,
            )
            .execute(&mut *conn)
            .await?;
            Ok(())
        }
    }

    fn update_torrent_by_info_hash(
        self,
        info_hash: &[u8],
        bitfield: &[u8],
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let q = sqlx::query!(
                "UPDATE torrents SET bitfield = ? WHERE info_hash = ?",
                bitfield,
                info_hash,
            );
            q.execute(&mut *conn).await?;
            Ok(())
        }
    }

    fn update_video_content_id(
        self,
        video_id: i64,
        content_id: i64,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            sqlx::query!(
                "UPDATE videos SET content_id = ? WHERE id = ?",
                content_id,
                video_id,
            )
            .execute(&mut *conn)
            .await?;
            Ok(())
        }
    }

    fn all_movies(
        self,
        params: ContentFetchParams,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<Movie>>> {
        async move {
            let mut conn = self.acquire().await?;
            let mut query = DbQueryBuilder::default();
            DbMovieQuery::build(&mut query);
            params.build(DbContentType::Movie, &mut query);
            Ok(query
                .build_query_as::<DbMovieQuery>()
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(Into::into)
                .collect())
        }
    }

    fn all_shows(
        self,
        params: ContentFetchParams,
    ) -> impl std::future::Future<Output = sqlx::Result<Vec<Show>>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let mut query = DbQueryBuilder::default();
            query_builders::DbShowQuery::build(&mut query);
            params.build(DbContentType::Show, &mut query);
            tracing::debug!(sql = %query.sql(), "All shows sql query");

            Ok(query
                .build_query_as::<query_builders::DbShowQuery>()
                .fetch_all(&mut *conn)
                .await?
                .into_iter()
                .map(Into::into)
                .collect())
        }
    }

    fn get_movie(self, id: i64) -> impl std::future::Future<Output = sqlx::Result<Movie>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let mut query = DbQueryBuilder::default();
            query_builders::DbMovieQuery::build(&mut query);
            query.push(" where movies.id = ").push_bind(id);
            tracing::debug!(sql = %query.sql(), "Get movie sql query");

            query
                .build_query_as::<query_builders::DbMovieQuery>()
                .fetch_one(&mut *conn)
                .await
                .map(Into::into)
        }
    }

    fn get_show(
        self,
        show_id: i64,
    ) -> impl std::future::Future<Output = sqlx::Result<Show>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let mut builder = DbQueryBuilder::default();
            query_builders::DbShowQuery::build(&mut builder);
            builder
                .push(" where shows.id = ")
                .push_bind(show_id)
                .build_query_as::<query_builders::DbShowQuery>()
                .fetch_one(&mut *conn)
                .await
                .map(Into::into)
        }
    }

    fn get_season(
        self,
        show_id: i64,
        season: usize,
    ) -> impl std::future::Future<Output = sqlx::Result<Season>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let season = season as i64;
            let season_row = sqlx::query!(
                r#"SELECT seasons.id, seasons.number, seasons.content_id,
                content.title, content.plot, content.poster, content.release_date
                FROM seasons
                JOIN content ON content.id = seasons.content_id
                WHERE seasons.show_id = ? AND seasons.number = ?"#,
                show_id,
                season
            )
            .fetch_one(&mut *conn)
            .await?;

            let episodes: Vec<_> = sqlx::query!(
                r#"SELECT episodes.id, episodes.number, episodes.duration, episodes.season_id,
                content.title, content.plot, content.poster, content.release_date,
                seasons.number AS season_number,
                history.id as "history_id?", history.is_finished, history.time as history_time, history.update_time as history_update_time,
                intros.id as "intro_id?", intros.start_sec as intro_start, intros.end_sec as intro_end
                FROM episodes
                JOIN seasons ON seasons.id = episodes.season_id
                JOIN content ON content.id = episodes.content_id
                LEFT JOIN intros ON intros.episode_id = episodes.id
                LEFT JOIN history ON history.content_id = episodes.content_id
                WHERE episodes.season_id = ? ORDER BY episodes.number ASC"#,
                season_row.id
            )
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|db_episode| {
                let local = LocalEpisodeData {
                    id: db_episode.id,
                    history: db_episode.history_id.map(|id| api_types::History {
                        id,
                        time: db_episode.history_time.map(Into::into).unwrap(),
                        is_finished: db_episode.is_finished.unwrap(),
                        update_time: db_episode.history_update_time.map(Into::into).unwrap(),
                    }),
                    intro: db_episode
                        .intro_start
                        .zip(db_episode.intro_end)
                        .map(|(start_sec, end_sec)| Intro { start_sec, end_sec }),
                };
                Episode {
                    metadata_id: db_episode.id.to_string(),
                    metadata_provider: MetadataProvider::Local,
                    release_date: db_episode.release_date,
                    number: db_episode.number as usize,
                    title: db_episode.title,
                    plot: db_episode.plot,
                    season_number: db_episode.season_number as usize,
                    runtime: Some(Duration::from_secs(db_episode.duration as u64).into()),
                    poster: db_episode
                        .poster,
                    cast: None,
                    local: Some(local),
                }
            })
            .collect();

            Ok(Season {
                metadata_id: season_row.id.to_string(),
                metadata_provider: MetadataProvider::Local,
                release_date: season_row.release_date,
                plot: season_row.plot,
                episodes,
                poster: season_row.poster,
                title: Some(season_row.title),
                number: season_row.number as usize,
                local: Some(LocalSeasonData { id: season_row.id }),
            })
        }
    }

    fn get_season_id(
        self,
        show_id: i64,
        season: usize,
    ) -> impl std::future::Future<Output = Result<i64, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let season = season as i64;
            let season = sqlx::query!(
                "SELECT seasons.id FROM seasons
            WHERE seasons.show_id = ? AND seasons.number = ?;",
                show_id,
                season,
            )
            .fetch_one(&mut *conn)
            .await?;

            Ok(season.id)
        }
    }

    fn lookup_actor_id(
        self,
        metadata_provider: MetadataProvider,
        metadata_id: &str,
        imdb_id: Option<&str>,
    ) -> impl std::future::Future<Output = sqlx::Result<Option<i64>>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            sqlx::query_scalar!(
                "select actors.id from actors
where (actors.metadata_provider = ? and actors.metadata_id = ?) or (? is not null and actors.imdb_id = ?)",
                metadata_provider,
                metadata_id,
                imdb_id,
                imdb_id
            )
            .fetch_optional(&mut *conn)
            .await
        }
    }

    fn get_episode(
        self,
        show_id: i64,
        season: usize,
        episode: usize,
    ) -> impl std::future::Future<Output = sqlx::Result<Episode>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let season = season as i64;
            let episode = episode as i64;
            let mut query = DbQueryBuilder::default();
            DbEpisodeQuery::build(&mut query);
            let q = query
                .push(" where seasons.show_id = ")
                .push_bind(show_id)
                .push(" and seasons.number = ")
                .push_bind(season)
                .push(" and episodes.number = ")
                .push_bind(episode)
                .build_query_as::<DbEpisodeQuery>();
            dbg!(q.sql());
            q.fetch_one(&mut *conn).await.map(Into::into)
        }
    }

    fn get_episode_id(
        self,
        show_id: i64,
        season: usize,
        episode: usize,
    ) -> impl std::future::Future<Output = Result<i64, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let season = season as i64;
            let episode = episode as i64;
            let episode = sqlx::query!(
                "SELECT episodes.id FROM episodes
            JOIN seasons ON seasons.id = episodes.season_id
            WHERE seasons.show_id = ? AND seasons.number = ? AND episodes.number = ?;",
                show_id,
                season,
                episode
            )
            .fetch_one(&mut *conn)
            .await?;

            Ok(episode.id)
        }
    }

    fn get_episode_by_id(
        self,
        episode_id: i64,
    ) -> impl std::future::Future<Output = Result<Episode, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let episode = sqlx::query!(
                r#"SELECT episodes.id, episodes.number, episodes.duration,
                content.title, content.plot, content.poster, content.release_date,
                seasons.number AS season_number,
                history.id as "history_id?", history.is_finished, history.time as history_time, history.update_time as history_update_time,
                intros.id as "intro_id?", intros.start_sec as intro_start, intros.end_sec as intro_end
                FROM episodes
                JOIN seasons ON seasons.id = episodes.season_id
                JOIN content ON content.id = episodes.content_id
                LEFT JOIN intros ON intros.episode_id = episodes.id
                LEFT JOIN history ON history.content_id = episodes.content_id
                WHERE episodes.id = ?;"#,
                episode_id,
            )
            .fetch_one(&mut *conn)
            .await?;

            let local = LocalEpisodeData {
                id: episode.id,
                history: episode.history_id.map(|id| api_types::History {
                    id,
                    time: episode.history_time.unwrap(),
                    is_finished: episode.is_finished.unwrap(),
                    update_time: episode.history_update_time.map(Into::into).unwrap(),
                }),
                intro: episode
                    .intro_start
                    .zip(episode.intro_end)
                    .map(|(start_sec, end_sec)| Intro { start_sec, end_sec }),
            };

            Ok(Episode {
                metadata_id: episode.id.to_string(),
                metadata_provider: MetadataProvider::Local,
                release_date: episode.release_date,
                plot: episode.plot,
                poster: episode.poster,
                number: episode.number as usize,
                title: episode.title,
                runtime: None,
                season_number: episode.season_number as usize,
                cast: None,
                local: Some(local),
            })
        }
    }

    fn get_system_id(self) -> impl std::future::Future<Output = Result<i64, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let system_id = sqlx::query_as!(DbSystemId, "SELECT id from system_id")
                .fetch_one(&mut *conn)
                .await?;
            Ok(system_id.id)
        }
    }

    fn get_uuid(self) -> impl std::future::Future<Output = Result<uuid::Uuid, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let db_upnp_uuid: DbUpnpUuid = sqlx::query_as(r#"SELECT uuid FROM upnp_uuid"#)
                .fetch_one(&mut *conn)
                .await?;

            Ok(db_upnp_uuid.uuid)
        }
    }

    fn all_torrents(
        self,
        limit: i64,
    ) -> impl std::future::Future<Output = Result<Vec<DbTorrent>, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let torrents = sqlx::query_as!(DbTorrent, "SELECT * FROM torrents LIMIT ?", limit)
                .fetch_all(&mut *conn)
                .await?;
            Ok(torrents)
        }
    }

    fn get_torrent_by_info_hash(
        self,
        info_hash: &[u8; 20],
    ) -> impl std::future::Future<Output = Result<DbTorrent, AppError>> + Send {
        async move {
            let info_hash = &info_hash[..];
            let mut conn = self.acquire().await?;
            let torrent = sqlx::query_as!(
                DbTorrent,
                "SELECT * FROM torrents WHERE torrents.info_hash = ?;",
                info_hash,
            )
            .fetch_one(&mut *conn)
            .await?;
            Ok(torrent)
        }
    }

    fn torrent_files(
        self,
        torrent_id: i64,
    ) -> impl std::future::Future<Output = Result<Vec<DbTorrentFile>, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let torrents = sqlx::query_as!(
                DbTorrentFile,
                "SELECT * FROM torrent_files WHERE torrent_id = ?",
                torrent_id
            )
            .fetch_all(&mut *conn)
            .await?;
            Ok(torrents)
        }
    }

    fn search_movie(
        self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<Vec<MovieMetadata>, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = format!("\"{}\"", query.trim().to_lowercase());
            let movies = sqlx::query!(
                r#"SELECT movies.id, movies.backdrop, movies.duration, movies.content_id,
                content.title, content.plot, content.poster, content.release_date,
                content.original_language, content.original_title
                FROM content_fts
                JOIN content ON content.rowid = content_fts.rowid
                JOIN movies ON movies.content_id = content.id
                WHERE content_fts MATCH ? AND content.content_type = 'movie'"#,
                query
            )
            .fetch_all(&mut *conn)
            .await?;

            Ok(movies
                .into_iter()
                .map(|m| {
                    let locale_metadata = m.original_language.zip(m.original_title).map(
                        |(original_language, original_title)| LocaleMetadata {
                            original_language,
                            original_title,
                        },
                    );
                    MovieMetadata {
                        metadata_id: m.id.to_string(),
                        metadata_provider: MetadataProvider::Local,
                        poster: m.poster,
                        backdrop: m.backdrop,
                        plot: m.plot,
                        release_date: m.release_date,
                        runtime: Some(Duration::from_secs(m.duration as u64).into()),
                        title: m.title,
                        locale_metadata,
                        cast: None,
                        external_ids: None,
                    }
                })
                .collect())
        }
    }

    fn search_show(
        self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<Vec<ShowMetadata>, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = format!("\"{}\"", query.trim().to_lowercase());
            let shows = sqlx::query!(
                r#"SELECT shows.id, shows.backdrop,
                content.title, content.plot, content.poster, content.release_date,
                content.original_language, content.original_title,
                (SELECT GROUP_CONCAT(seasons.number) FROM seasons WHERE seasons.show_id = shows.id) as "seasons!: String",
                (SELECT COUNT(*) FROM episodes JOIN seasons ON episodes.season_id = seasons.id WHERE seasons.show_id = shows.id) as "episodes_count!: i64"
                FROM content_fts
                JOIN content ON content.rowid = content_fts.rowid
                JOIN shows ON shows.content_id = content.id
                WHERE content_fts MATCH ? AND content.content_type = 'show'"#,
                query
            )
            .fetch_all(&mut *conn)
            .await?;
            Ok(shows
                .into_iter()
                .map(|show| {
                    let seasons = show
                        .seasons
                        .split(',')
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    let locale_metadata = show.original_title.zip(show.original_language).map(
                        |(original_title, original_language)| LocaleMetadata {
                            original_title,
                            original_language,
                        },
                    );
                    ShowMetadata {
                        metadata_id: show.id.to_string(),
                        metadata_provider: MetadataProvider::Local,
                        poster: show.poster,
                        backdrop: show.backdrop,
                        plot: show.plot,
                        episodes_amount: Some(show.episodes_count as usize),
                        seasons: Some(seasons),
                        release_date: show.release_date,
                        title: show.title,
                        locale_metadata,
                        cast: None,
                        external_ids: None,
                    }
                })
                .collect())
        }
    }

    fn search_episode(
        self,
        query: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<EpisodeMetadata>>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = query.trim().to_lowercase();
            let episodes = sqlx::query!(
                r#"SELECT episodes.id, episodes.season_id, episodes.number, episodes.duration,
                content.title, content.plot, content.poster, content.release_date,
                seasons.number AS season_number FROM episodes
                JOIN seasons ON seasons.id = episodes.season_id
                JOIN content ON content.id = episodes.content_id
                JOIN videos ON videos.content_id = episodes.content_id
                WHERE content.title = ? COLLATE NOCASE"#,
                query
            )
            .fetch_all(&mut *conn)
            .await?;
            Ok(episodes
                .into_iter()
                .map(|episode| EpisodeMetadata {
                    metadata_id: episode.id.to_string(),
                    metadata_provider: MetadataProvider::Local,
                    release_date: episode.release_date,
                    number: episode.number as usize,
                    title: episode.title,
                    plot: episode.plot,
                    season_number: episode.season_number as usize,
                    runtime: Some(Duration::from_secs(episode.duration as u64).into()),
                    poster: episode.poster,
                    cast: None,
                })
                .collect())
        }
    }

    /// external to local show id
    fn crossreference_show(
        self,
        provider: MetadataProvider,
        metadata_id: &str,
    ) -> impl std::future::Future<Output = sqlx::Result<Option<i64>>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let provider = provider.to_string();
            let show_id = sqlx::query!(
                r#"SELECT shows.id as "show_id!" FROM external_ids
                JOIN shows ON shows.content_id = external_ids.content_id
                WHERE external_ids.metadata_provider = ? AND external_ids.metadata_id = ?"#,
                provider,
                metadata_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .map(|r| r.show_id);
            Ok(show_id)
        }
    }

    /// external to local movie id
    fn crossreference_movie(
        self,
        provider: MetadataProvider,
        metadata_id: &str,
    ) -> impl std::future::Future<Output = sqlx::Result<Option<i64>>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let provider = provider.to_string();
            let movie_id = sqlx::query!(
                r#"SELECT movies.id as "movie_id!" FROM external_ids
                JOIN movies ON movies.content_id = external_ids.content_id
                WHERE external_ids.metadata_provider = ? AND external_ids.metadata_id = ?"#,
                provider,
                metadata_id
            )
            .fetch_optional(&mut *conn)
            .await?
            .map(|r| r.movie_id);
            Ok(movie_id)
        }
    }
}

impl<'a> DbActions<'a> for &'a mut Transaction<'static, Sqlite> {}
impl<'a> DbActions<'a> for &'a Pool<Sqlite> {}
impl<'a> DbActions<'a> for &'a mut SqliteConnection {}

pub type DbTransaction = Transaction<'static, Sqlite>;
pub type DbQueryBuilder<'a> = QueryBuilder<'a, Sqlite>;

/// Database connection pool
#[derive(Debug, Clone)]
pub struct Db {
    pub pool: SqlitePool,
}

impl Deref for Db {
    type Target = SqlitePool;
    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

static MIGRATOR: Migrator = sqlx::migrate!();

impl Db {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        let url = path_to_url(path.as_ref());
        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(30)
            .connect_with(options.clone())
            .await?;
        match MIGRATOR.run(&pool).await {
            Ok(_) => (),
            Err(MigrateError::VersionMismatch(_)) => {
                // I hope this code will never run
                let path = &config::APP_RESOURCES.database_path;
                tokio::fs::remove_file(path).await?;
                tracing::error!("Failed to validate some of the migrations, doing database reset!");
                config::AppResources::initiate()?;
                let pool = SqlitePoolOptions::new()
                    .max_connections(30)
                    .connect_with(options)
                    .await?;
                MIGRATOR.run(&pool).await?;
                return Ok(Self { pool });
            }
            Err(e) => return Err(e.into()),
        };

        let uuid = uuid::Uuid::new_v4();
        sqlx::query!(
            "INSERT OR IGNORE INTO upnp_uuid (id, uuid) VALUES (0, ?);",
            uuid,
        )
        .execute(&pool)
        .await
        .expect("insert upnp uuid");

        Ok(Self { pool })
    }
}

impl From<DbExternalId> for ExternalIdMetadata {
    fn from(val: DbExternalId) -> Self {
        ExternalIdMetadata {
            provider: val.metadata_provider,
            id: val.metadata_id,
        }
    }
}

// Types for each table in the local database

#[derive(sqlx::Type, Debug, Clone, Serialize)]
#[sqlx(rename_all = "lowercase")]
pub enum DbContentType {
    Movie,
    Show,
    Season,
    Episode,
}

impl DbContentType {
    pub fn table_name(&self) -> &'static str {
        match self {
            DbContentType::Movie => "movies",
            DbContentType::Show => "shows",
            DbContentType::Season => "seasons",
            DbContentType::Episode => "episodes",
        }
    }
}

/// `content` is a shared table that holds common information between movies, shows, seasons and
/// episodes
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbContent {
    #[sqlx(rename = "content_table_id")]
    pub id: Option<i64>,
    pub content_type: DbContentType,
    pub title: String,
    pub original_language: Option<String>,
    pub original_title: Option<String>,
    pub release_date: Option<String>,
    /// Url that we get from information provider.
    ///
    /// Note that it is not local poster url.
    pub poster: Option<String>,
    pub plot: Option<String>,
}

impl DbContent {
    pub const SQL: &str =
        "content.id as content_table_id, content.content_type, content.title, content.original_language,
    content.original_title, content.release_date, content.poster, content.plot";
}

/// `shows` table holds information for specific tv show
///
/// Note that it will not be deleted using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbShow {
    #[sqlx(rename = "show_table_id")]
    pub id: Option<i64>,
    #[sqlx(rename = "show_content_id")]
    pub content_id: i64,
    /// Url that we get from information provider.
    ///
    /// Backdrop is the 16/9 high canvas that can be used as the background
    ///
    /// Note that it is not local backdrop url.
    #[sqlx(rename = "show_backdrop")]
    pub backdrop: Option<String>,
}

impl DbShow {
    pub const SQL: &str = "shows.id as show_table_id, shows.content_id as show_content_id, shows.backdrop as show_backdrop";
}

/// `seasons` table holds information for specific season.
///
/// Note that it will not be deleted using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbSeason {
    #[sqlx(rename = "season_table_id")]
    pub id: Option<i64>,
    #[sqlx(rename = "season_content_id")]
    pub content_id: i64,
    #[sqlx(rename = "season_show_id")]
    pub show_id: i64,
    #[sqlx(rename = "season_number")]
    pub number: i64,
}

impl DbSeason {
    pub const SQL: &str = "seasons.id as season_table_id, seasons.content_id as season_content_id, seasons.show_id as season_show_id, seasons.number as season_number";
}

/// `movies` table holds information for specific movie
///
/// Note that it will not be removed using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbMovie {
    #[sqlx(rename = "movie_table_id")]
    pub id: Option<i64>,
    #[sqlx(rename = "movie_content_id")]
    pub content_id: i64,
    #[sqlx(rename = "movie_duration")]
    pub duration: i64,
    #[sqlx(rename = "movie_backdrop")]
    pub backdrop: Option<String>,
}

impl DbMovie {
    const SQL: &str = "movies.id as movie_table_id, movies.content_id as movie_content_id,
    movies.duration as movie_duration, movies.backdrop as movie_backdrop";
}

/// `episodes` table holds information for specific episode
///
/// Note that it will not be removed using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbEpisode {
    #[sqlx(rename = "episodes_table_id")]
    pub id: Option<i64>,
    #[sqlx(rename = "episodes_content_id")]
    pub content_id: i64,
    pub season_id: i64,
    #[sqlx(rename = "episodes_number")]
    pub number: i64,
    #[sqlx(rename = "episodes_duration")]
    pub duration: i64,
}

impl DbEpisode {
    pub const SQL: &str =
        "episodes.id as episodes_table_id, episodes.content_id as episodes_content_id,
    season_id, episodes.number as episodes_number, episodes.duration as episodes_duration";
}

/// `videos` table tracks every local video we have in the library.
/// Note that it is not guaranteed that the video will be available on the drive.
/// Videos are the core of the media server. This table is _synced_ during the "library refresh"
///
/// Note that it will not be removed using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbVideo {
    pub id: Option<i64>,
    pub path: String,
    pub is_prime: bool,
    pub size: i64,
    pub content_id: Option<i64>,
    pub scan_date: String,
}

/// `subtitles` table is used for tracking subtitles assets.
///
/// If `external_path` is not null, then subtitles are external, meaning they are not managed by the server.
/// For example user can reference subtitles file from a specific directory without need for server to
/// save it in assets directory.
/// Otherwise all subtitles in this table are stored inside assets directory.
///
/// Note that subtitles inside the video containers are not tracked by the database
///
/// Usually removed with video using delete cascade
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbSubtitles {
    pub id: Option<i64>,
    pub language: Option<String>,
    /// This is a path "reference" on subtitles file specified by user.
    /// When this field is present, subtitles are not stored in the server's assets directory.
    pub external_path: Option<String>,
    pub video_id: i64,
}

/// `history` table holds watch history for each content item in the library
///
/// Usually removed with content using cascade delete
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbHistory {
    #[sqlx(rename = "history_id")]
    pub id: Option<i64>,
    pub time: i64,
    pub is_finished: bool,
    #[sqlx(default)]
    pub update_time: Option<crate::OffsetDateTime>,
    #[sqlx(rename = "history_content_id")]
    pub content_id: i64,
}

impl DbHistory {
    pub const SQL: &str = r#" history.id as history_id, history.time, history.is_finished,
    history.update_time, history.content_id as history_content_id "#;
}

/// `external_ids` table maps content to external movie/show metadata provider ids.
/// For example it can connect tmdb ID to specific local tv show.
/// This is useful to crossmatch local library against different providers.
///
/// Usually removed with it's _parent_ using cascade delete
#[derive(Debug, Clone, FromRow, Serialize, utoipa::ToSchema, Default)]
pub struct DbExternalId {
    #[schema(value_type = i64)]
    pub id: Option<i64>,
    pub metadata_provider: MetadataProvider,
    pub metadata_id: String,
    pub content_id: Option<i64>,
    pub is_prime: i64,
}

/// `intros` table stores detected intros for a specific episode
///
/// Usually removed with the episode using cascade delete
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbIntro {
    #[sqlx(rename = "intros_table_id")]
    pub id: Option<i64>,
    #[sqlx(rename = "intros_episode_id")]
    pub episode_id: i64,
    pub start_sec: i64,
    pub end_sec: i64,
}

impl DbIntro {
    pub const SQL: &str = "intros.id as intros_table_id, intros.episode_id as intros_episode_id, intros.start_sec, intros.end_sec";
}

/// `actors` table stores information about every actor
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbActor {
    #[sqlx(rename = "actor_id")]
    pub id: Option<i64>,
    #[sqlx(rename = "actor_name")]
    pub name: String,
    #[sqlx(rename = "actor_imdb_id")]
    pub imdb_id: Option<String>,
    #[sqlx(rename = "actor_metadata_id")]
    pub metadata_id: String,
    #[sqlx(rename = "actor_metadata_provider")]
    pub metadata_provider: MetadataProvider,
    #[sqlx(rename = "actor_poster")]
    pub poster: Option<String>,
}

impl DbActor {
    pub const SQL: &str = "actors.id as actor_id, actors.name as actor_name, actors.imdb_id as actor_imdb_id, actors.metadata_id as actor_metadata_id,
actors.metadata_provider as actor_metadata_provider, actors.poster as actor_poster";
}

/// `role` table stores information about role that is played by actor
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbRole {
    #[sqlx(rename = "role_id")]
    pub id: Option<i64>,
    #[sqlx(rename = "role_actor_id")]
    pub actor_id: i64,
    #[sqlx(rename = "role_content_id")]
    pub content_id: i64,
    pub character: Option<String>,
}

impl DbRole {
    pub const SQL: &str =
        "id as role_id, actor_id as role_actor_id, content_id as role_content_id, character";
}

/// `torrents` table holds currently active torrents.
///
/// Torrents can be in any state.
/// This is used to resume torrents after server restart.
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbTorrent {
    pub id: Option<i64>,
    pub bencoded_info: Vec<u8>,
    pub trackers: String,
    pub save_location: String,
    pub info_hash: Vec<u8>,
    pub bitfield: Vec<u8>,
    pub added_at: Option<time::OffsetDateTime>,
}

impl From<DownloadParams> for DbTorrent {
    fn from(params: DownloadParams) -> Self {
        let tracker_list = params.trackers;
        let bitfield = params.bitfield.0;
        let trackers: Vec<String> = tracker_list.iter().map(ToString::to_string).collect();
        let trackers = trackers.join(",");
        Self {
            id: None,
            // TODO: avoid copy
            bencoded_info: params.info.as_bytes().to_vec(),
            trackers,
            save_location: params
                .save_location
                .to_owned()
                .to_string_lossy()
                .to_string(),
            info_hash: params.info.hash().to_vec(),
            bitfield,
            added_at: None,
        }
    }
}

/// `torrent_files` table stores information about every file inside particular torrent download.
///
/// Usually removed with parent torrent using cascade deletes
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbTorrentFile {
    pub id: Option<i64>,
    pub torrent_id: i64,
    pub content_id: Option<i64>,
    pub priority: i64,
    pub idx: i64,
    pub relative_path: String,
}

/// `system_id` table stores the single row: global `system_id`.
/// It is incremented using SQL triggers every time any information in library (movies, shows, seasons,
/// episodes) changes.
/// This is only used in UPnP [content_directory service implementation](crate::upnp::content_directory)
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbSystemId {
    pub id: i64,
}

/// `upnp_uuid` table stores the single row: `uuid`.
/// This uuid created once during database initialization and used during UPnP announces.
#[derive(Debug, Clone, FromRow, Default)]
pub struct DbUpnpUuid {
    pub uuid: uuid::Uuid,
}
