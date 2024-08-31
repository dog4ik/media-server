use std::{path::Path, str::FromStr, time::Duration};

use serde::Serialize;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Error, FromRow, Sqlite, SqlitePool,
};

use crate::{
    app_state::AppError,
    library::assets::{self, AssetDir},
    metadata::{
        ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, MetadataImage,
        MetadataProvider, MovieMetadata, MovieMetadataProvider, SeasonMetadata, ShowMetadata,
        ShowMetadataProvider,
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

#[derive(Debug, Clone)]
pub struct Db {
    pub pool: SqlitePool,
}

impl Db {
    pub async fn connect(path: impl AsRef<Path>) -> Result<Self, sqlx::Error> {
        let url = path_to_url(path.as_ref());
        let options = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .busy_timeout(Duration::from_secs(10));
        let pool = SqlitePoolOptions::new()
            .max_connections(30)
            .connect_with(options)
            .await?;
        sqlx::query!(
r#"CREATE TABLE IF NOT EXISTS shows (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    title TEXT NOT NULL, 
                                    release_date TEXT,
                                    poster TEXT,
                                    backdrop TEXT,
                                    plot TEXT);

CREATE VIRTUAL TABLE IF NOT EXISTS shows_fts_idx USING fts5(title, plot, content='shows', content_rowid='id');
CREATE TRIGGER IF NOT EXISTS shows_tbl_ai AFTER INSERT ON shows BEGIN
  INSERT INTO shows_fts_idx(rowid, title, plot) VALUES (new.id, new.title, new.plot);
END;
CREATE TRIGGER IF NOT EXISTS shows_tbl_ad AFTER DELETE ON shows BEGIN
  INSERT INTO shows_fts_idx(shows_fts_idx, rowid, title, plot) VALUES('delete', old.id, old.title, old.plot);
END;
CREATE TRIGGER IF NOT EXISTS shows_tbl_au AFTER UPDATE ON shows BEGIN
  INSERT INTO shows_fts_idx(shows_fts_idx, rowid, title, plot) VALUES('delete', old.id, old.title, old.plot);
  INSERT INTO shows_fts_idx(rowid, title, plot) VALUES (new.id, new.title, new.plot);
END;

CREATE TABLE IF NOT EXISTS seasons (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    show_id INTEGER NOT NULL,
                                    number INTEGER NOT NULL,
                                    release_date TEXT,
                                    plot TEXT,
                                    poster TEXT,
                                    FOREIGN KEY (show_id) REFERENCES shows (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS episodes (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    video_id INTEGER NOT NULL UNIQUE,
                                    season_id INTEGER NOT NULL,
                                    title TEXT NOT NULL, 
                                    number INTEGER NOT NULL,
                                    plot TEXT,
                                    poster TEXT,
                                    release_date TEXT,
                                    FOREIGN KEY (video_id) REFERENCES videos (id),
                                    FOREIGN KEY (season_id) REFERENCES seasons (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS movies (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    title TEXT NOT NULL,
                                    backdrop TEXT,
                                    plot TEXT,
                                    poster TEXT,
                                    release_date TEXT,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);

CREATE VIRTUAL TABLE IF NOT EXISTS movies_fts_idx USING fts5(title, plot, content='movies', content_rowid='id');
CREATE TRIGGER IF NOT EXISTS movies_tbl_ai AFTER INSERT ON movies BEGIN
  INSERT INTO movies_fts_idx(rowid, title, plot) VALUES (new.id, new.title, new.plot);
END;
CREATE TRIGGER IF NOT EXISTS movies_tbl_ad AFTER DELETE ON movies BEGIN
  INSERT INTO movies_fts_idx(movies_fts_idx, rowid, title, plot) VALUES('delete', old.id, old.title, old.plot);
END;
CREATE TRIGGER IF NOT EXISTS movies_tbl_au AFTER UPDATE ON movies BEGIN
  INSERT INTO movies_fts_idx(movies_fts_idx, rowid, title, plot) VALUES('delete', old.id, old.title, old.plot);
  INSERT INTO movies_fts_idx(rowid, title, plot) VALUES (new.id, new.title, new.plot);
END;

CREATE TABLE IF NOT EXISTS videos (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT, 
                                    path TEXT NOT NULL UNIQUE,
                                    size INTEGER NOT NULL,
                                    duration INTEGER NOT NULL,
                                    scan_date DATETIME DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE IF NOT EXISTS subtitles (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    language TEXT NOT NULL,
                                    hash TEXT NOT NULL,
                                    path TEXT NOT NULL,
                                    size INTEGER NOT NULL,
                                    video_id INTEGER NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS history (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    time INTEGER NOT NULL,
                                    is_finished BOOL NOT NULL,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    update_time DATETIME DEFAULT CURRENT_TIMESTAMP NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS external_ids (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    metadata_provider TEXT NOT NULL,
                                    metadata_id TEXT NOT NULL,
                                    show_id INTEGER,
                                    season_id INTEGER,
                                    episode_id INTEGER,
                                    movie_id INTEGER,
                                    is_prime INTEGER NOT NULL,
                                    FOREIGN KEY (show_id) REFERENCES shows (id) ON DELETE CASCADE,
                                    FOREIGN KEY (season_id) REFERENCES seasons (id) ON DELETE CASCADE,
                                    FOREIGN KEY (episode_id) REFERENCES episodes (id) ON DELETE CASCADE,
                                    FOREIGN KEY (movie_id) REFERENCES movies (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS episode_intro (id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    start_sec INTEGER NOT NULL,
                                    end_sec INTEGER NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);"#)
        .execute(&pool)
        .await
        .unwrap();

        Ok(Self { pool })
    }

    pub async fn clear(&self) -> Result<(), sqlx::Error> {
        sqlx::query::<Sqlite>(
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
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_movie(&self, movie: DbMovie) -> Result<i64, Error> {
        let query = sqlx::query!(
            "INSERT OR IGNORE INTO movies 
            (title, release_date, poster,
            backdrop, plot, video_id)
            VALUES (?, ?, ?, ?, ?, ?) RETURNING id;",
            movie.title,
            movie.release_date,
            movie.poster,
            movie.backdrop,
            movie.plot,
            movie.video_id
        );
        query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_show(&self, show: DbShow) -> Result<i64, Error> {
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

        query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_season(&self, season: DbSeason) -> Result<i64, Error> {
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

        query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_episode(&self, episode: DbEpisode) -> Result<i64, Error> {
        let episode_query = sqlx::query!(
            "INSERT OR IGNORE INTO episodes
            (video_id, season_id, title, number, plot, release_date, poster)
            VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id;",
            episode.video_id,
            episode.season_id,
            episode.title,
            episode.number,
            episode.plot,
            episode.release_date,
            episode.poster,
        );

        episode_query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_video(&self, db_video: DbVideo) -> Result<i64, Error> {
        tracing::debug!("Inserting new video: {}", db_video.path);
        let video_query = sqlx::query!(
            "INSERT INTO videos
            (path, size, duration)
            VALUES (?, ?, ?) RETURNING id;",
            db_video.path,
            db_video.size,
            db_video.duration
        );
        video_query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_subtitles(&self, db_subtitles: DbSubtitles) -> Result<i64, Error> {
        let subtitles_query = sqlx::query!(
            "INSERT INTO subtitles
            (language, hash, path, size, video_id)
            VALUES (?, ?, ?, ?, ?) RETURNING id;",
            db_subtitles.language,
            db_subtitles.hash,
            db_subtitles.path,
            db_subtitles.size,
            db_subtitles.video_id
        );
        subtitles_query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_history(&self, db_history: DbHistory) -> Result<i64, Error> {
        let history_query = sqlx::query!(
            "INSERT INTO history
            (time, is_finished, video_id)
            VALUES (?, ?, ?) RETURNING id;",
            db_history.time,
            db_history.is_finished,
            db_history.video_id
        );
        history_query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_external_id(&self, db_external_id: DbExternalId) -> Result<i64, Error> {
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
        subtitles_query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_intro(&self, intro: DbEpisodeIntro) -> Result<i64, Error> {
        let subtitles_query = sqlx::query!(
            "INSERT INTO episode_intro
            (video_id, start_sec, end_sec)
            VALUES (?, ?, ?) RETURNING id;",
            intro.video_id,
            intro.start_sec,
            intro.end_sec,
        );
        subtitles_query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn external_to_local_id(
        &self,
        external_id: &str,
        metadata_provider: MetadataProvider,
    ) -> Result<DbExternalId, Error> {
        let metadata_provider = metadata_provider.to_string();
        let external_id = sqlx::query_as!(
            DbExternalId,
            "SELECT * from external_ids WHERE metadata_provider = ? AND metadata_id = ?;",
            metadata_provider,
            external_id,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(external_id)
    }

    /// Get local id by external ids
    pub async fn external_to_local_ids(
        &self,
        external_ids: &Vec<ExternalIdMetadata>,
    ) -> Option<DbExternalId> {
        for external_id in external_ids {
            if let Ok(external_id) = self
                .external_to_local_id(&external_id.id, external_id.provider)
                .await
            {
                return Some(external_id);
            }
        }
        None
    }

    pub async fn remove_video(&self, id: i64) -> Result<(), Error> {
        tracing::debug!(id, "Removing video");
        let remove_query = sqlx::query!("DELETE FROM videos WHERE id = ?;", id);

        if let Ok(episode) = sqlx::query!(r#"SELECT id FROM episodes WHERE video_id = ?"#, id)
            .fetch_one(&self.pool)
            .await
        {
            let _ = self.remove_episode(episode.id).await;
            remove_query.execute(&self.pool).await?;
            return Ok(());
        }

        if let Ok(movie) = sqlx::query!(r#"SELECT id FROM movies WHERE video_id = ?"#, id)
            .fetch_one(&self.pool)
            .await
        {
            let _ = self.remove_movie(movie.id).await;
            remove_query.execute(&self.pool).await?;
            return Ok(());
        }

        remove_query.execute(&self.pool).await?;

        Ok(())
    }

    pub async fn remove_episode(&self, id: i64) -> Result<(), Error> {
        tracing::debug!(id, "Removing episode");
        let delete_episode_result = sqlx::query!(
            "DELETE FROM episodes WHERE id = ? RETURNING season_id, video_id",
            id
        )
        .fetch_one(&self.pool)
        .await?;

        let episode_assets = assets::EpisodeAssetsDir::new(id);
        if let Err(e) = episode_assets.delete_dir().await {
            tracing::warn!("Failed to clean up episode directory: {e}")
        };

        let season_id = delete_episode_result.season_id;

        let siblings_count = sqlx::query!(
            "SELECT COUNT(*) AS count FROM episodes WHERE season_id = ?",
            season_id
        )
        .fetch_one(&self.pool)
        .await?
        .count;
        tracing::debug!("Removed episode siblings count: {}", siblings_count);
        if siblings_count == 0 {
            self.remove_season(season_id).await?;
        }
        Ok(())
    }

    pub async fn remove_season(&self, id: i64) -> Result<(), Error> {
        tracing::debug!(id, "Removing season");
        let delete_result = sqlx::query!("DELETE FROM seasons WHERE id = ? RETURNING show_id", id)
            .fetch_one(&self.pool)
            .await?;

        let season_assets = assets::SeasonAssetsDir::new(id);
        if let Err(e) = season_assets.delete_dir().await {
            tracing::warn!("Failed to clean up season directory: {e}")
        };

        let show_id = delete_result.show_id;
        let siblings_count = sqlx::query!(
            "SELECT COUNT(*) AS count FROM seasons WHERE show_id = ?",
            show_id
        )
        .fetch_one(&self.pool)
        .await?
        .count;
        if siblings_count == 0 {
            self.remove_show(delete_result.show_id).await?;
        }
        Ok(())
    }

    /// Relies on `ON DELETE CASCADE` to remove show's seasons and episodes.
    /// This causes seasons/episodes assets to become orphaned and not cleaned up
    pub async fn remove_show(&self, id: i64) -> Result<(), Error> {
        tracing::debug!(id, "Removing show");
        let query = sqlx::query!("DELETE FROM shows WHERE id = ?", id);
        query.execute(&self.pool).await?;

        let show_assets = assets::ShowAssetsDir::new(id);
        if let Err(e) = show_assets.delete_dir().await {
            tracing::warn!("Failed to clean up show directory: {e}")
        };

        Ok(())
    }

    pub async fn remove_movie(&self, id: i64) -> Result<(), Error> {
        tracing::debug!(id, "Removing movie");
        let query = sqlx::query!("DELETE FROM movies WHERE id = ?", id);
        query.execute(&self.pool).await?;

        let movie_assets = assets::MovieAssetsDir::new(id);
        if let Err(e) = movie_assets.delete_dir().await {
            tracing::warn!("Failed to clean up movie directory: {e}")
        };

        Ok(())
    }

    pub async fn update_show_metadata(&self, id: i64, metadata: ShowMetadata) -> Result<(), Error> {
        let db_show = metadata.into_db_show();
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
        q.fetch_one(&self.pool).await?;
        Ok(())
    }

    pub async fn update_season_metadata(
        &self,
        id: i64,
        show_id: i64,
        metadata: SeasonMetadata,
    ) -> Result<(), Error> {
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
        q.fetch_one(&self.pool).await?;
        Ok(())
    }

    pub async fn update_episode_metadata(
        &self,
        id: i64,
        season_id: i32,
        metadata: EpisodeMetadata,
    ) -> Result<(), Error> {
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
        q.fetch_one(&self.pool).await?;
        Ok(())
    }

    pub async fn all_movies(&self) -> anyhow::Result<Vec<MovieMetadata>> {
        let movies = sqlx::query_as!(DbMovie, "SELECT movies.* FROM movies")
            .fetch_all(&self.pool)
            .await?;
        Ok(movies.into_iter().map(|movie| movie.into()).collect())
    }

    pub async fn all_shows(&self) -> anyhow::Result<Vec<ShowMetadata>> {
        let shows = sqlx::query!(r#"SELECT shows.*,
            (SELECT GROUP_CONCAT(seasons.number) FROM seasons WHERE seasons.show_id = shows.id) as "seasons!: String",
            (SELECT COUNT(*) FROM episodes JOIN seasons ON episodes.season_id = seasons.id WHERE seasons.show_id = shows.id) as "episodes_count!: i64"
            FROM shows
            "#)
            .fetch_all(&self.pool)
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

    pub async fn get_movie(&self, id: i64) -> Result<MovieMetadata, AppError> {
        let movie = sqlx::query_as!(DbMovie, "SELECT movies.* FROM movies WHERE id = ?", id)
            .fetch_one(&self.pool)
            .await?;
        Ok(movie.into())
    }

    pub async fn get_show(&self, show_id: i64) -> Result<ShowMetadata, AppError> {
        let show = sqlx::query!(r#"SELECT shows.*,
            (SELECT GROUP_CONCAT(seasons.number) FROM seasons WHERE seasons.show_id = shows.id) as "seasons!: String",
            (SELECT COUNT(*) FROM episodes JOIN seasons ON episodes.season_id = seasons.id WHERE seasons.show_id = shows.id) as "episodes_count!: i64"
            FROM shows WHERE id = ?"#, show_id)
            .fetch_one(&self.pool)
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

    pub async fn get_season(
        &self,
        show_id: i64,
        season: usize,
    ) -> Result<SeasonMetadata, AppError> {
        let season = season as i64;
        let season = sqlx::query!(
            r#"SELECT seasons.*
            FROM seasons 
            JOIN shows ON shows.id = seasons.show_id
            WHERE shows.id = ? AND seasons.number = ?"#,
            show_id,
            season
        )
        .fetch_one(&self.pool)
        .await?;

        let episodes: Vec<_> = sqlx::query!(
            "SELECT episodes.*, videos.duration FROM episodes 
JOIN videos ON videos.id = episodes.video_id
WHERE season_id = ? ORDER BY number ASC",
            season.id
        )
        .fetch_all(&self.pool)
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

    pub async fn get_episode(
        &self,
        show_id: i64,
        season: usize,
        episode: usize,
    ) -> Result<EpisodeMetadata, AppError> {
        let season = season as i64;
        let episode = episode as i64;
        let episode = sqlx::query!(
            "SELECT episodes.*, seasons.number as season_number, videos.duration FROM episodes
            JOIN seasons ON seasons.id = episodes.season_id
            JOIN shows ON shows.id = seasons.show_id
            JOIN videos ON videos.id = episodes.video_id
            WHERE shows.id = ? AND seasons.number = ? AND episodes.number = ?;",
            show_id,
            season,
            episode
        )
        .fetch_one(&self.pool)
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

    pub async fn get_episode_by_id(&self, episode_id: i64) -> Result<EpisodeMetadata, AppError> {
        let episode = sqlx::query!(
            r#"SELECT episodes.*, seasons.number AS season_number FROM episodes 
            JOIN seasons ON seasons.id = episodes.season_id
            WHERE episodes.id = ?;"#,
            episode_id,
        )
        .fetch_one(&self.pool)
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

    pub async fn search_movie(&self, query: &str) -> Result<Vec<MovieMetadata>, AppError> {
        let query = query.trim().to_lowercase();
        let movies = sqlx::query_as!(
            DbMovie,
            "SELECT movies.* FROM movies_fts_idx JOIN movies ON movies.id = movies_fts_idx.rowid WHERE movies_fts_idx = ?",
            query
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(movies.into_iter().map(|movie| movie.into()).collect())
    }

    pub async fn search_show(&self, query: &str) -> Result<Vec<ShowMetadata>, AppError> {
        let query = query.trim().to_lowercase();
        let shows = sqlx::query!(
            r#"SELECT shows.*,
            (SELECT GROUP_CONCAT(seasons.number) FROM seasons WHERE seasons.show_id = shows.id) as "seasons!: String",
            (SELECT COUNT(*) FROM episodes JOIN seasons ON episodes.season_id = seasons.id WHERE seasons.show_id = shows.id) as "episodes_count!: i64"
            FROM shows_fts_idx JOIN shows ON shows.id = shows_fts_idx.rowid 
            WHERE shows_fts_idx = ?"#,
            query
        )
        .fetch_all(&self.pool)
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

    pub async fn search_episode(&self, query: &str) -> anyhow::Result<Vec<EpisodeMetadata>> {
        let query = query.trim().to_lowercase();
        let episodes = sqlx::query!(
            r#"SELECT episodes.*, seasons.number AS season_number, videos.duration FROM episodes
            JOIN seasons ON seasons.id = episodes.season_id
            JOIN videos ON videos.id = episodes.video_id
            WHERE title = ? COLLATE NOCASE"#,
            query
        )
        .fetch_all(&self.pool)
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

#[axum::async_trait]
impl ShowMetadataProvider for Db {
    async fn show(&self, show_id: &str) -> Result<ShowMetadata, AppError> {
        self.get_show(show_id.parse()?).await
    }

    async fn season(&self, show_id: &str, season: usize) -> Result<SeasonMetadata, AppError> {
        self.get_season(show_id.parse()?, season).await
    }

    async fn episode(
        &self,
        show_id: &str,
        season: usize,
        episode: usize,
    ) -> Result<EpisodeMetadata, AppError> {
        self.get_episode(show_id.parse()?, season, episode).await
    }

    fn provider_identifier(&self) -> &'static str {
        "local"
    }
}

#[axum::async_trait]
impl MovieMetadataProvider for Db {
    async fn movie(
        &self,
        movie_metadata_id: &str,
    ) -> Result<crate::metadata::MovieMetadata, AppError> {
        self.get_movie(movie_metadata_id.parse()?).await
    }

    fn provider_identifier(&self) -> &'static str {
        "local"
    }
}

#[axum::async_trait]
impl DiscoverMetadataProvider for Db {
    async fn multi_search(
        &self,
        query: &str,
    ) -> Result<Vec<crate::metadata::MetadataSearchResult>, AppError> {
        use rand::seq::SliceRandom;
        let movies = self.search_movie(query).await?;
        let shows = self.search_show(query).await?;
        let mut out = Vec::with_capacity(movies.len() + shows.len());
        out.extend(movies.into_iter().map(|m| m.into()));
        out.extend(shows.into_iter().map(|m| m.into()));
        let mut rng = rand::thread_rng();
        out.shuffle(&mut rng);
        Ok(out)
    }

    async fn show_search(&self, query: &str) -> Result<Vec<ShowMetadata>, AppError> {
        self.search_show(query).await
    }

    async fn movie_search(
        &self,
        query: &str,
    ) -> Result<Vec<crate::metadata::MovieMetadata>, AppError> {
        self.search_movie(query).await
    }

    async fn external_ids(
        &self,
        content_id: &str,
        content_hint: ContentType,
    ) -> Result<Vec<ExternalIdMetadata>, AppError> {
        let db_ids = match content_hint {
            ContentType::Movie => {
                sqlx::query_as!(
                    DbExternalId,
                    "SELECT * FROM external_ids WHERE movie_id = ?",
                    content_id
                )
                .fetch_all(&self.pool)
                .await
            }
            ContentType::Show => {
                sqlx::query_as!(
                    DbExternalId,
                    "SELECT * FROM external_ids WHERE show_id = ?",
                    content_id
                )
                .fetch_all(&self.pool)
                .await
            }
        }?;
        Ok(db_ids.into_iter().map(|i| i.into()).collect())
    }

    fn provider_identifier(&self) -> &'static str {
        "local"
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

//Types

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbShow {
    pub id: Option<i64>,
    pub title: String,
    pub release_date: Option<String>,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub plot: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbSeason {
    pub id: Option<i64>,
    pub show_id: i64,
    pub number: i64,
    pub release_date: Option<String>,
    pub plot: Option<String>,
    pub poster: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbMovie {
    pub id: Option<i64>,
    pub video_id: i64,
    pub title: String,
    pub plot: Option<String>,
    pub poster: Option<String>,
    pub release_date: Option<String>,
    pub backdrop: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbEpisode {
    pub id: Option<i64>,
    pub video_id: i64,
    pub season_id: i64,
    pub title: String,
    pub number: i64,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    pub poster: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbVideo {
    pub id: Option<i64>,
    pub path: String,
    pub size: i64,
    pub duration: i64,
    pub scan_date: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbSubtitles {
    pub id: Option<i64>,
    pub language: Option<String>,
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub video_id: i64,
}

#[derive(Debug, Clone, FromRow, Serialize, utoipa::ToSchema)]
pub struct DbHistory {
    #[schema(value_type = i64)]
    pub id: Option<i64>,
    pub time: i64,
    pub is_finished: bool,
    pub update_time: time::OffsetDateTime,
    pub video_id: i64,
}

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

#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbEpisodeIntro {
    pub id: Option<i64>,
    pub video_id: i64,
    pub start_sec: i64,
    pub end_sec: i64,
}
