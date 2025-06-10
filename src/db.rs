use std::{ops::Deref, path::Path, str::FromStr, time::Duration};

use serde::Serialize;
use sqlx::{
    Acquire, Error, FromRow, Pool, Sqlite, SqliteConnection, SqlitePool, Transaction,
    migrate::{MigrateError, Migrator},
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use torrent::DownloadParams;

use crate::{
    app_state::AppError,
    config,
    library::assets::{self, AssetDir},
    metadata::{
        ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, FetchParams,
        MetadataImage, MetadataProvider, MovieMetadata, MovieMetadataProvider, SeasonMetadata,
        ShowMetadata, ShowMetadataProvider,
    },
};

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
        DELETE FROM shows;
        DELETE FROM seasons;
        DELETE FROM episodes;
        DELETE FROM movies;
        DELETE FROM videos;
        DELETE FROM subtitles;
        DELETE FROM history;
        DELETE FROM external_ids;
        DELETE FROM episode_intro;
        ",
            )
            .execute(&mut *conn)
            .await?;
            Ok(())
        }
    }

    fn insert_movie(
        self,
        movie: DbMovie,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = sqlx::query!(
                "INSERT OR IGNORE INTO movies 
            (title, release_date, poster,
            backdrop, plot, duration)
            VALUES (?, ?, ?, ?, ?, ?) RETURNING id;",
                movie.title,
                movie.release_date,
                movie.poster,
                movie.backdrop,
                movie.plot,
                movie.duration,
            );
            query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_show(
        self,
        show: &DbShow,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = sqlx::query!(
                "INSERT OR IGNORE INTO shows 
            (title, release_date, poster, backdrop, plot)
            VALUES (?, ?, ?, ?, ?) RETURNING id;",
                show.title,
                show.release_date,
                show.poster,
                show.backdrop,
                show.plot,
            );

            query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_season(
        self,
        season: DbSeason,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = sqlx::query!(
                "INSERT OR IGNORE INTO seasons
            (show_id, number, release_date, plot, poster)
            VALUES (?, ?, ?, ?, ?) RETURNING id;",
                season.show_id,
                season.number,
                season.release_date,
                season.plot,
                season.poster,
            );

            query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_episode(
        self,
        episode: &DbEpisode,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let episode_query = sqlx::query!(
                "INSERT OR IGNORE INTO episodes
            (season_id, title, number, plot, release_date, poster, duration)
            VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id;",
                episode.season_id,
                episode.title,
                episode.number,
                episode.plot,
                episode.release_date,
                episode.poster,
                episode.duration,
            );

            episode_query.fetch_one(&mut *conn).await.map(|x| x.id)
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
            (path, size, movie_id, episode_id, is_prime)
            VALUES (?, ?, ?, ?, ?) RETURNING id;",
                db_video.path,
                db_video.size,
                db_video.movie_id,
                db_video.episode_id,
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
            (time, is_finished, video_id)
            VALUES (?, ?, ?) RETURNING id;",
                db_history.time,
                db_history.is_finished,
                db_history.video_id
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
            let subtitles_query = sqlx::query!(
                "INSERT INTO external_ids
            (metadata_provider, metadata_id, show_id, season_id, episode_id, movie_id, is_prime)
            VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id;",
                db_external_id.metadata_provider,
                db_external_id.metadata_id,
                db_external_id.show_id,
                db_external_id.season_id,
                db_external_id.episode_id,
                db_external_id.movie_id,
                db_external_id.is_prime,
            );
            subtitles_query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_intro(
        self,
        intro: DbEpisodeIntro,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::trace!("Inserting intro for video {}", intro.video_id);
            let subtitles_query = sqlx::query!(
                "INSERT INTO episode_intro
            (video_id, start_sec, end_sec)
            VALUES (?, ?, ?) RETURNING id;",
                intro.video_id,
                intro.start_sec,
                intro.end_sec,
            );
            subtitles_query.fetch_one(&mut *conn).await.map(|x| x.id)
        }
    }

    fn insert_torrent(
        self,
        torrent: DbTorrent,
    ) -> impl std::future::Future<Output = Result<i64, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let query = sqlx::query!(
                "INSERT OR IGNORE INTO torrents
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
                "INSERT OR IGNORE INTO torrent_files
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

    fn external_to_local_id(
        self,
        external_id: &str,
        metadata_provider: MetadataProvider,
    ) -> impl std::future::Future<Output = Result<DbExternalId, Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let metadata_provider = metadata_provider.to_string();
            let external_id = sqlx::query_as!(
                DbExternalId,
                "SELECT * from external_ids WHERE metadata_provider = ? AND metadata_id = ?;",
                metadata_provider,
                external_id,
            )
            .fetch_one(&mut *conn)
            .await?;
            Ok(external_id)
        }
    }

    /// Get local id by external ids
    fn external_to_local_ids(
        self,
        external_ids: &Vec<ExternalIdMetadata>,
    ) -> impl std::future::Future<Output = Option<DbExternalId>> + Send {
        async move {
            let mut conn = self.acquire().await.ok()?;
            for external_id in external_ids {
                if let Ok(external_id) = conn
                    .external_to_local_id(&external_id.id, external_id.provider)
                    .await
                {
                    return Some(external_id);
                }
            }
            None
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
                        "SELECT * FROM external_ids WHERE movie_id = ?",
                        content_id
                    )
                    .fetch_all(&mut *conn)
                    .await
                }
                ContentType::Show => {
                    sqlx::query_as!(
                        DbExternalId,
                        "SELECT * FROM external_ids WHERE show_id = ?",
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
            let remove_query = sqlx::query!(
                "DELETE FROM videos WHERE id = ? RETURNING episode_id, movie_id;",
                id
            );
            let remove_result = remove_query.fetch_one(&mut *conn).await?;

            if let Some(episode_id) = remove_result.episode_id {
                let sibling_videos = sqlx::query!(
                    "SELECT COUNT(*) AS count FROM videos WHERE episode_id = ?",
                    episode_id
                )
                .fetch_one(&mut *conn)
                .await?;
                if sibling_videos.count == 0 {
                    let _ = conn.remove_episode(episode_id).await;
                }
                return Ok(());
            }

            if let Some(movie_id) = remove_result.movie_id {
                let sibling_videos = sqlx::query!(
                    "SELECT COUNT(*) AS count FROM videos WHERE movie_id = ?",
                    movie_id
                )
                .fetch_one(&mut *conn)
                .await?;
                if sibling_videos.count == 0 {
                    let _ = conn.remove_movie(movie_id).await;
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
            let delete_episode_result =
                sqlx::query!("DELETE FROM episodes WHERE id = ? RETURNING season_id", id)
                    .fetch_one(&mut *conn)
                    .await?;

            let season_id = delete_episode_result.season_id;

            let siblings_count = sqlx::query!(
                "SELECT COUNT(*) AS count FROM episodes WHERE season_id = ?",
                season_id
            )
            .fetch_one(&mut *conn)
            .await?
            .count;
            tracing::debug!("Removed episode siblings count: {}", siblings_count);
            if siblings_count == 0 {
                conn.remove_season(season_id).await?;
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
            let delete_result =
                sqlx::query!("DELETE FROM seasons WHERE id = ? RETURNING show_id", id)
                    .fetch_one(&mut *conn)
                    .await?;

            let show_id = delete_result.show_id;
            let siblings_count = sqlx::query!(
                "SELECT COUNT(*) AS count FROM seasons WHERE show_id = ?",
                show_id
            )
            .fetch_one(&mut *conn)
            .await?
            .count;
            if siblings_count == 0 {
                conn.remove_show(delete_result.show_id).await?;
            }

            let season_assets = assets::SeasonAssetsDir::new(id);
            if let Err(e) = season_assets.delete_dir().await {
                tracing::warn!("Failed to clean up season directory: {e}")
            };
            Ok(())
        }
    }

    /// Relies on `ON DELETE CASCADE` to remove show's seasons and episodes.
    /// This causes seasons/episodes assets to become orphaned and not cleaned up
    fn remove_show(self, id: i64) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            tracing::debug!(id, "Removing show");
            let query = sqlx::query!("DELETE FROM shows WHERE id = ?", id);

            query.execute(&mut *conn).await?;

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
            let query = sqlx::query!("DELETE FROM movies WHERE id = ?", id);

            query.execute(&mut *conn).await?;

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
            let query = sqlx::query!("DELETE FROM episode_intro WHERE id = ?", id);

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

    fn update_show_metadata(
        self,
        id: i64,
        metadata: ShowMetadata,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let db_show = DbShow::from(metadata);
            let q = sqlx::query!(
                "UPDATE shows SET
                            title = ?, 
                            release_date = ?,
                            poster = ?,
                            backdrop = ?,
                            plot = ?
            WHERE id = ?",
                db_show.title,
                db_show.release_date,
                db_show.poster,
                db_show.backdrop,
                db_show.plot,
                id
            );
            q.execute(&mut *conn).await?;
            Ok(())
        }
    }

    fn update_season_metadata(
        self,
        id: i64,
        show_id: i64,
        metadata: SeasonMetadata,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let db_season = metadata.into_db_season(show_id);
            let q = sqlx::query!(
                "UPDATE seasons SET
                               show_id = ?,
                               number = ?,
                               release_date = ?,
                               plot = ?,
                               poster = ?,
                               show_id = ?
            WHERE id = ?",
                db_season.show_id,
                db_season.number,
                db_season.release_date,
                db_season.plot,
                db_season.poster,
                db_season.show_id,
                id
            );
            q.execute(&mut *conn).await?;
            Ok(())
        }
    }

    fn update_episode_metadata(
        self,
        id: i64,
        season_id: i32,
        metadata: EpisodeMetadata,
    ) -> impl std::future::Future<Output = Result<(), Error>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let number = metadata.number as i32;
            let poster = metadata.poster.map(|p| p.as_str().to_string());
            let q = sqlx::query!(
                "UPDATE episodes SET
                                season_id = ?,
                                title = ?, 
                                number = ?,
                                plot = ?,
                                poster = ?,
                                release_date = ?
            WHERE id = ?",
                season_id,
                metadata.title,
                number,
                metadata.plot,
                poster,
                metadata.release_date,
                id
            );
            q.execute(&mut *conn).await?;
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

    fn update_video_episode_id(
        self,
        video_id: i64,
        episode_id: i64,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            sqlx::query!(
                "UPDATE videos SET episode_id = ? WHERE id = ?",
                episode_id,
                video_id,
            )
            .execute(&mut *conn)
            .await?;
            Ok(())
        }
    }

    fn update_video_movie_id(
        self,
        video_id: i64,
        movie_id: i64,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            sqlx::query!(
                "UPDATE videos SET movie_id = ? WHERE id = ?",
                movie_id,
                video_id,
            )
            .execute(&mut *conn)
            .await?;
            Ok(())
        }
    }

    fn all_movies(
        self,
        limit: impl Into<Option<i64>>,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<MovieMetadata>>> {
        async move {
            let limit = limit.into().unwrap_or(DEFAULT_LIMIT);
            let mut conn = self.acquire().await?;
            let movies = sqlx::query_as!(DbMovie, "SELECT movies.* FROM movies LIMIT ?", limit)
                .fetch_all(&mut *conn)
                .await?;
            Ok(movies.into_iter().map(Into::into).collect())
        }
    }

    fn all_shows(
        self,
        limit: impl Into<Option<i64>>,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<ShowMetadata>>> + Send {
        let limit = limit.into().unwrap_or(DEFAULT_LIMIT);
        async move {
            let mut conn = self.acquire().await?;
            let shows = sqlx::query!(r#"SELECT shows.*,
            (SELECT GROUP_CONCAT(seasons.number) FROM seasons WHERE seasons.show_id = shows.id) as "seasons!: String",
            (SELECT COUNT(*) FROM episodes JOIN seasons ON episodes.season_id = seasons.id WHERE seasons.show_id = shows.id) as "episodes_count!: i64"
            FROM shows LIMIT ?;"#, limit)
            .fetch_all(&mut *conn)
            .await?;
            Ok(shows
                .into_iter()
                .map(|show| {
                    let poster = show.poster.map(|p| MetadataImage::new(p.parse().unwrap()));
                    let backdrop = show
                        .backdrop
                        .map(|b| MetadataImage::new(b.parse().unwrap()));
                    let seasons = show
                        .seasons
                        .split(',')
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    ShowMetadata {
                        metadata_id: show.id.to_string(),
                        metadata_provider: MetadataProvider::Local,
                        poster,
                        backdrop,
                        plot: show.plot,
                        episodes_amount: Some(show.episodes_count as usize),
                        seasons: Some(seasons),
                        release_date: show.release_date,
                        title: show.title,
                    }
                })
                .collect())
        }
    }

    fn get_movie(
        self,
        id: i64,
    ) -> impl std::future::Future<Output = Result<MovieMetadata, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let movie = sqlx::query_as!(DbMovie, "SELECT movies.* FROM movies WHERE id = ?", id)
                .fetch_one(&mut *conn)
                .await?;
            Ok(movie.into())
        }
    }

    fn get_show(
        self,
        show_id: i64,
    ) -> impl std::future::Future<Output = Result<ShowMetadata, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let show = sqlx::query!(r#"SELECT shows.*,
            (SELECT GROUP_CONCAT(seasons.number) FROM seasons WHERE seasons.show_id = shows.id) as "seasons!: String",
            (SELECT COUNT(*) FROM episodes JOIN seasons ON episodes.season_id = seasons.id WHERE seasons.show_id = shows.id) as "episodes_count!: i64"
            FROM shows WHERE id = ?"#, show_id)
            .fetch_one(&mut *conn)
            .await?;
            let poster = show.poster.map(|p| MetadataImage::new(p.parse().unwrap()));
            let backdrop = show
                .backdrop
                .map(|b| MetadataImage::new(b.parse().unwrap()));
            let mut seasons: Vec<_> = show
                .seasons
                .split(',')
                .filter_map(|x| x.parse().ok())
                .collect();
            seasons.sort_unstable();
            Ok(ShowMetadata {
                metadata_id: show.id.to_string(),
                metadata_provider: MetadataProvider::Local,
                poster,
                backdrop,
                plot: show.plot,
                episodes_amount: Some(show.episodes_count as usize),
                seasons: Some(seasons),
                release_date: show.release_date,
                title: show.title,
            })
        }
    }

    /// Local season episodes with local video id
    fn get_local_season_episodes(
        self,
        show_id: i64,
        season: usize,
    ) -> impl std::future::Future<Output = Result<Vec<DbEpisode>, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let season = season as i64;
            let episodes = sqlx::query_as!(
                DbEpisode,
                r#"SELECT episodes.* FROM episodes
            JOIN seasons ON seasons.id = episodes.season_id
            WHERE seasons.show_id = ? AND seasons.number = ? ORDER BY episodes.number;"#,
                show_id,
                season,
            )
            .fetch_all(&mut *conn)
            .await?;

            Ok(episodes)
        }
    }

    fn get_season(
        self,
        show_id: i64,
        season: usize,
    ) -> impl std::future::Future<Output = Result<SeasonMetadata, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let season = season as i64;
            let season = sqlx::query!(
                r#"SELECT * FROM seasons WHERE seasons.show_id = ? AND seasons.number = ?"#,
                show_id,
                season
            )
            .fetch_one(&mut *conn)
            .await?;

            let episodes: Vec<_> = sqlx::query!(
                "SELECT * FROM episodes WHERE season_id = ? ORDER BY number ASC",
                season.id
            )
            .fetch_all(&mut *conn)
            .await?
            .into_iter()
            .map(|db_episode| EpisodeMetadata {
                metadata_id: db_episode.id.to_string(),
                metadata_provider: MetadataProvider::Local,
                release_date: db_episode.release_date,
                number: db_episode.number as usize,
                title: db_episode.title,
                plot: db_episode.plot,
                season_number: season.number as usize,
                runtime: Some(Duration::from_secs(db_episode.duration as u64)),
                poster: db_episode
                    .poster
                    .map(|x| MetadataImage::new(x.parse().unwrap())),
            })
            .collect();

            let poster = season
                .poster
                .map(|p| MetadataImage::new(p.parse().unwrap()));

            Ok(SeasonMetadata {
                metadata_id: season.id.to_string(),
                metadata_provider: MetadataProvider::Local,
                release_date: season.release_date,
                plot: season.plot,
                episodes,
                poster,
                number: season.number as usize,
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

    fn get_episode(
        self,
        show_id: i64,
        season: usize,
        episode: usize,
    ) -> impl std::future::Future<Output = Result<EpisodeMetadata, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let season = season as i64;
            let episode = episode as i64;
            let episode = sqlx::query!(
                "SELECT episodes.*, seasons.number as season_number FROM episodes
            JOIN seasons ON seasons.id = episodes.season_id
            JOIN shows ON shows.id = seasons.show_id
            WHERE shows.id = ? AND seasons.number = ? AND episodes.number = ?;",
                show_id,
                season,
                episode
            )
            .fetch_one(&mut *conn)
            .await?;

            let poster = episode
                .poster
                .map(|p| MetadataImage::new(p.parse().unwrap()));

            Ok(EpisodeMetadata {
                metadata_id: episode.id.to_string(),
                metadata_provider: MetadataProvider::Local,
                release_date: episode.release_date,
                plot: episode.plot,
                poster,
                number: episode.number as usize,
                title: episode.title,
                runtime: Some(Duration::from_secs(episode.duration as u64)),
                season_number: episode.season_number as usize,
            })
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
    ) -> impl std::future::Future<Output = Result<EpisodeMetadata, AppError>> + Send {
        async move {
            let mut conn = self.acquire().await?;
            let episode = sqlx::query!(
                r#"SELECT episodes.*, seasons.number AS season_number FROM episodes 
            JOIN seasons ON seasons.id = episodes.season_id
            WHERE episodes.id = ?;"#,
                episode_id,
            )
            .fetch_one(&mut *conn)
            .await?;

            let poster = episode
                .poster
                .map(|p| MetadataImage::new(p.parse().unwrap()));

            Ok(EpisodeMetadata {
                metadata_id: episode.id.to_string(),
                metadata_provider: MetadataProvider::Local,
                release_date: episode.release_date,
                plot: episode.plot,
                poster,
                number: episode.number as usize,
                title: episode.title,
                runtime: None,
                season_number: episode.season_number as usize,
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
            let query = query.trim().to_lowercase();
            let movies = sqlx::query_as!(
            DbMovie,
            "SELECT movies.* FROM movies_fts_idx JOIN movies ON movies.id = movies_fts_idx.rowid WHERE movies_fts_idx = ?",
            query
        )
        .fetch_all(&mut *conn)
        .await?;
            Ok(movies.into_iter().map(Into::into).collect())
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
            r#"SELECT shows.*,
            (SELECT GROUP_CONCAT(seasons.number) FROM seasons WHERE seasons.show_id = shows.id) as "seasons!: String",
            (SELECT COUNT(*) FROM episodes JOIN seasons ON episodes.season_id = seasons.id WHERE seasons.show_id = shows.id) as "episodes_count!: i64"
            FROM shows_fts_idx JOIN shows ON shows.id = shows_fts_idx.rowid 
            WHERE shows_fts_idx MATCH ?"#,
            query
        )
        .fetch_all(&mut *conn)
        .await?;
            Ok(shows
                .into_iter()
                .map(|show| {
                    let poster = show.poster.map(|p| MetadataImage::new(p.parse().unwrap()));
                    let backdrop = show
                        .backdrop
                        .map(|b| MetadataImage::new(b.parse().unwrap()));
                    let seasons = show
                        .seasons
                        .split(',')
                        .filter_map(|x| x.parse().ok())
                        .collect();
                    ShowMetadata {
                        metadata_id: show.id.to_string(),
                        metadata_provider: MetadataProvider::Local,
                        poster,
                        backdrop,
                        plot: show.plot,
                        episodes_amount: Some(show.episodes_count as usize),
                        seasons: Some(seasons),
                        release_date: show.release_date,
                        title: show.title,
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
                r#"SELECT episodes.*, seasons.number AS season_number FROM episodes
            JOIN seasons ON seasons.id = episodes.season_id
            JOIN videos ON videos.episode_id = episodes.id
            WHERE title = ? COLLATE NOCASE"#,
                query
            )
            .fetch_all(&mut *conn)
            .await?;
            Ok(episodes
                .into_iter()
                .map(|episode| {
                    let poster = episode
                        .poster
                        .map(|p| MetadataImage::new(p.parse().unwrap()));
                    EpisodeMetadata {
                        metadata_id: episode.id.to_string(),
                        metadata_provider: MetadataProvider::Local,
                        release_date: episode.release_date,
                        number: episode.number as usize,
                        title: episode.title,
                        plot: episode.plot,
                        season_number: episode.season_number as usize,
                        runtime: Some(Duration::from_secs(episode.duration as u64)),
                        poster,
                    }
                })
                .collect())
        }
    }
}

impl<'a> DbActions<'a> for &'a mut Transaction<'static, Sqlite> {}
impl<'a> DbActions<'a> for &'a Pool<Sqlite> {}
impl<'a> DbActions<'a> for &'a mut SqliteConnection {}

pub type DbTransaction = Transaction<'static, Sqlite>;

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

#[async_trait::async_trait]
impl ShowMetadataProvider for Db {
    async fn show(
        &self,
        show_id: &str,
        _fetch_params: FetchParams,
    ) -> Result<ShowMetadata, AppError> {
        self.pool.get_show(show_id.parse()?).await
    }

    async fn season(
        &self,
        show_id: &str,
        season: usize,
        _fetch_params: FetchParams,
    ) -> Result<SeasonMetadata, AppError> {
        self.pool.get_season(show_id.parse()?, season).await
    }

    async fn episode(
        &self,
        show_id: &str,
        season: usize,
        episode: usize,
        _fetch_params: FetchParams,
    ) -> Result<EpisodeMetadata, AppError> {
        self.pool
            .get_episode(show_id.parse()?, season, episode)
            .await
    }

    fn provider_identifier(&self) -> MetadataProvider {
        MetadataProvider::Local
    }
}

#[async_trait::async_trait]
impl MovieMetadataProvider for Db {
    async fn movie(
        &self,
        movie_metadata_id: &str,
        _fetch_params: FetchParams,
    ) -> Result<crate::metadata::MovieMetadata, AppError> {
        self.pool.get_movie(movie_metadata_id.parse()?).await
    }

    fn provider_identifier(&self) -> MetadataProvider {
        MetadataProvider::Local
    }
}

#[async_trait::async_trait]
impl DiscoverMetadataProvider for Db {
    async fn multi_search(
        &self,
        query: &str,
        _fetch_params: FetchParams,
    ) -> Result<Vec<crate::metadata::MetadataSearchResult>, AppError> {
        use rand::seq::SliceRandom;
        let (movies, shows) =
            tokio::try_join!(self.pool.search_movie(query), self.pool.search_show(query))?;
        let mut out = Vec::with_capacity(movies.len() + shows.len());
        out.extend(movies.into_iter().map(Into::into));
        out.extend(shows.into_iter().map(Into::into));
        let mut rng = rand::rng();
        out.shuffle(&mut rng);
        Ok(out)
    }

    async fn show_search(
        &self,
        query: &str,
        _fetch_params: FetchParams,
    ) -> Result<Vec<ShowMetadata>, AppError> {
        self.pool.search_show(query).await
    }

    async fn movie_search(
        &self,
        query: &str,
        _fetch_params: FetchParams,
    ) -> Result<Vec<crate::metadata::MovieMetadata>, AppError> {
        self.pool.search_movie(query).await
    }

    async fn external_ids(
        &self,
        content_id: &str,
        content_hint: ContentType,
    ) -> Result<Vec<ExternalIdMetadata>, AppError> {
        self.get_external_ids(content_id.parse()?, content_hint)
            .await
    }

    fn provider_identifier(&self) -> MetadataProvider {
        MetadataProvider::Local
    }
}

impl From<DbMovie> for MovieMetadata {
    fn from(val: DbMovie) -> Self {
        let poster = val.poster.map(|p| MetadataImage::new(p.parse().unwrap()));
        let backdrop = val.backdrop.map(|b| MetadataImage::new(b.parse().unwrap()));

        MovieMetadata {
            metadata_id: val.id.unwrap().to_string(),
            metadata_provider: MetadataProvider::Local,
            poster,
            backdrop,
            plot: val.plot,
            release_date: val.release_date,
            runtime: None,
            title: val.title,
        }
    }
}

impl From<DbExternalId> for ExternalIdMetadata {
    fn from(val: DbExternalId) -> Self {
        ExternalIdMetadata {
            provider: MetadataProvider::from_str(&val.metadata_provider).unwrap(),
            id: val.metadata_id,
        }
    }
}

// Types for each table in the local database

/// `shows` table simply holds information for specific tv show
///
/// Note that it will not be deleted ucascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbShow {
    pub id: Option<i64>,
    pub title: String,
    pub release_date: Option<String>,
    /// Url that we get from information provider.
    ///
    /// Note that it is not local poster url.
    pub poster: Option<String>,
    /// Url that we get from information provider.
    ///
    /// Backdrop is the 16/9 high canvas that can be used as the background
    ///
    /// Note that it is not local backdrop url.
    pub backdrop: Option<String>,
    pub plot: Option<String>,
}

/// `seasons` table simply holds information for specific season.
///
/// Note that it will not be deleted using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbSeason {
    pub id: Option<i64>,
    pub show_id: i64,
    pub number: i64,
    pub release_date: Option<String>,
    pub plot: Option<String>,
    /// Url that we get from information provider.
    ///
    /// Note that it is not local url.
    pub poster: Option<String>,
}

/// `movies` table simply holds information for specific movie
///
/// Note that it will not be removed using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbMovie {
    pub id: Option<i64>,
    pub title: String,
    pub plot: Option<String>,
    /// Url that we get from information provider.
    /// Note that it is not local poster url.
    pub poster: Option<String>,
    pub release_date: Option<String>,
    pub duration: i64,
    pub backdrop: Option<String>,
}

/// `episodes` table simply holds information for specific episode
///
/// Note that it will not be removed using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbEpisode {
    pub id: Option<i64>,
    pub season_id: i64,
    pub title: String,
    pub number: i64,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    pub duration: i64,
    /// Url that we get from information provider.
    ///
    /// Note that it is not local poster url.
    pub poster: Option<String>,
}

/// `videos` table tracks every local video we have in the library.
/// Note that it is not guaranteed that the video will be available on the drive.
/// Videos are the core of the media server. This table is _synced_ during the "library refresh"
///
/// Note that it will not be removed using cascade because related assets must be cleaned up
/// manually.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbVideo {
    pub id: Option<i64>,
    pub path: String,
    pub is_prime: bool,
    pub size: i64,
    pub movie_id: Option<i64>,
    pub episode_id: Option<i64>,
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
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbSubtitles {
    pub id: Option<i64>,
    pub language: Option<String>,
    /// This is a path "reference" on subtitles file specified by user.
    /// When this field is present, subtitles are not stored in the server's assets directory.
    pub external_path: Option<String>,
    pub video_id: i64,
}

/// `history` table simply holds history for each video file in the library
///
/// Usually removed with video using cascade delete
#[derive(Debug, Clone, FromRow, Serialize, utoipa::ToSchema)]
pub struct DbHistory {
    #[schema(value_type = i64)]
    pub id: Option<i64>,
    pub time: i64,
    pub is_finished: bool,
    pub update_time: time::OffsetDateTime,
    pub video_id: i64,
}

/// `external_ids` table maps content to external movie/show metadata provider ids.
/// For example it can connect tmdb ID to specific local tv show.
/// This is useful to crossmatch local library against different providers.
///
/// Usually removed with it's _parent_ using cascade delete
#[derive(Debug, Clone, FromRow, Serialize, Default, utoipa::ToSchema)]
pub struct DbExternalId {
    #[schema(value_type = i64)]
    pub id: Option<i64>,
    pub metadata_provider: String,
    pub metadata_id: String,
    pub show_id: Option<i64>,
    pub season_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub is_prime: i64,
}

/// `episode_intros` table simply stores detected intros for a specific video
///
/// Usually removed with the video using cascade delete
#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbEpisodeIntro {
    pub id: Option<i64>,
    pub video_id: i64,
    pub start_sec: i64,
    pub end_sec: i64,
}

/// `torrents` table holds currently active torrents.
///
/// Torrents can be in any state.
/// This is used to resume torrents after server restart.
#[derive(Debug, Clone, FromRow, Serialize)]
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
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbTorrentFile {
    pub id: Option<i64>,
    pub torrent_id: i64,
    pub priority: i64,
    pub idx: i64,
    pub relative_path: String,
}

/// `system_id` table stores the single row: global `system_id`.
/// It is incremented using SQL triggers every time any information in library (movies, shows, seasons,
/// episodes) changes.
/// This is only used in UPnP [content_directory service implementation](crate::upnp::content_directory)
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbSystemId {
    pub id: i64,
}

/// `upnp_uuid` table stores the single row: `uuid`.
/// This uuid created once during database initialization and used during UPnP announces.
#[derive(Debug, Clone, FromRow)]
pub struct DbUpnpUuid {
    pub uuid: uuid::Uuid,
}
