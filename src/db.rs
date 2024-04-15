use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use serde::Serialize;
use sqlx::{sqlite::SqlitePoolOptions, Error, FromRow, Sqlite, SqlitePool};

use crate::{
    app_state::AppError,
    metadata::{
        ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, MetadataImage,
        MetadataProvider, MovieMetadata, MovieMetadataProvider, SeasonMetadata, ShowMetadata,
        ShowMetadataProvider,
    },
};

fn path_to_url(path: &Path) -> String {
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
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await?;
        sqlx::query!(
r#"CREATE TABLE IF NOT EXISTS shows (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    title TEXT NOT NULL, 
                                    release_date TEXT,
                                    poster TEXT,
                                    blur_data TEXT,
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

CREATE TABLE IF NOT EXISTS seasons (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    show_id INTEGER NOT NULL,
                                    number INTEGER NOT NULL,
                                    release_date TEXT,
                                    plot TEXT,
                                    poster TEXT,
                                    blur_data TEXT,
                                    FOREIGN KEY (show_id) REFERENCES shows (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS episodes (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    video_id INTEGER NOT NULL UNIQUE,
                                    season_id INTEGER NOT NULL,
                                    title TEXT NOT NULL, 
                                    number INTEGER NOT NULL,
                                    plot TEXT,
                                    poster TEXT,
                                    blur_data TEXT,
                                    release_date TEXT,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE,
                                    FOREIGN KEY (season_id) REFERENCES seasons (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS movies (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    title TEXT NOT NULL,
                                    blur_data TEXT,
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

CREATE TABLE IF NOT EXISTS videos (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    path TEXT NOT NULL UNIQUE,
                                    resources_folder TEXT NOT NULL UNIQUE,
                                    size INTEGER NOT NULL,
                                    duration INTEGER NOT NULL,
                                    scan_date DATETIME DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE IF NOT EXISTS subtitles (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    language TEXT NOT NULL,
                                    hash TEXT NOT NULL,
                                    path TEXT NOT NULL,
                                    size INTEGER NOT NULL,
                                    video_id INTEGER NOT NULL,
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS external_ids (id INTEGER PRIMARY KEY AUTOINCREMENT,
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
                                    FOREIGN KEY (movie_id) REFERENCES movies (id) ON DELETE CASCADE);"#)
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
        DELETE FROM external_ids;
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
            blur_data, backdrop, plot, video_id)
            VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id;",
            movie.title,
            movie.release_date,
            movie.poster,
            movie.blur_data,
            movie.backdrop,
            movie.plot,
            movie.video_id
        );
        query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_show(&self, show: DbShow) -> Result<i64, Error> {
        let query = sqlx::query!(
            "INSERT OR IGNORE INTO shows 
            (title, release_date, poster, blur_data, backdrop, plot)
            VALUES (?, ?, ?, ?, ?, ?) RETURNING id;",
            show.title,
            show.release_date,
            show.poster,
            show.blur_data,
            show.backdrop,
            show.plot,
        );

        query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_season(&self, season: DbSeason) -> Result<i64, Error> {
        let query = sqlx::query!(
            "INSERT OR IGNORE INTO seasons
            (show_id, number, release_date, plot, poster, blur_data)
            VALUES (?, ?, ?, ?, ?, ?) RETURNING id;",
            season.show_id,
            season.number,
            season.release_date,
            season.plot,
            season.poster,
            season.blur_data,
        );

        query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_episode(&self, episode: DbEpisode) -> Result<i64, Error> {
        let episode_query = sqlx::query!(
            "INSERT OR IGNORE INTO episodes
            (video_id, season_id, title, number, plot, release_date, poster, blur_data)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id;",
            episode.video_id,
            episode.season_id,
            episode.title,
            episode.number,
            episode.plot,
            episode.release_date,
            episode.poster,
            episode.blur_data
        );

        episode_query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_video(&self, db_video: DbVideo) -> Result<i64, Error> {
        let video_query = sqlx::query!(
            "INSERT INTO videos
            (path, resources_folder, size, duration)
            VALUES (?, ?, ?, ?) RETURNING id;",
            db_video.path,
            db_video.resources_folder,
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

    pub async fn remove_video(&self, id: i64) -> Result<(), Error> {
        sqlx::query!("DELETE FROM videos WHERE id = ?;", id)
            .execute(&self.pool)
            .await?;

        if let Ok(episode) =
            sqlx::query!(r#"SELECT id as "id!" FROM episodes WHERE video_id = ?"#, id)
                .fetch_one(&self.pool)
                .await
        {
            return self.remove_episode(episode.id).await;
        }

        if let Ok(_movie) = sqlx::query!(r#"SELECT id as "id!" FROM movies WHERE video_id = ?"#, id)
            .fetch_one(&self.pool)
            .await
        {
            // TODO: remove movie
        }

        Ok(())
    }

    pub async fn remove_video_by_path(&self, path: &PathBuf) -> Result<(), Error> {
        let str_path = path.to_str().unwrap();
        let id = sqlx::query!(r#"SELECT id as "id!" FROM videos WHERE path = ?"#, str_path)
            .fetch_one(&self.pool)
            .await?
            .id;
        self.remove_video(id).await?;
        Ok(())
    }

    pub async fn remove_episode(&self, id: i64) -> Result<(), Error> {
        let delete_episode_result = sqlx::query!(
            "DELETE FROM episodes WHERE id = ? RETURNING season_id, video_id",
            id
        )
        .fetch_one(&self.pool)
        .await?;
        let season_id = delete_episode_result.season_id;
        sqlx::query!(
            "DELETE FROM videos WHERE id = ? ",
            delete_episode_result.video_id
        )
        .execute(&self.pool)
        .await?;

        let siblings_count = sqlx::query!(
            "SELECT COUNT(*) as count FROM episodes WHERE season_id = ?",
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
        let delete_result = sqlx::query!("DELETE FROM seasons WHERE id = ? RETURNING show_id", id)
            .fetch_one(&self.pool)
            .await?;
        let show_id = delete_result.show_id;
        let siblings_count = sqlx::query!(
            "SELECT COUNT(*) as count FROM seasons WHERE show_id = ?",
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

    pub async fn remove_show(&self, id: i64) -> Result<(), Error> {
        let query = sqlx::query!("DELETE FROM shows WHERE id = ?", id);
        query.execute(&self.pool).await?;
        Ok(())
    }

    pub async fn update_show_metadata(&self, id: i64, metadata: ShowMetadata) -> Result<(), Error> {
        let db_show = metadata.into_db_show().await;
        let q = sqlx::query!(
            "UPDATE shows SET
                            title = ?, 
                            release_date = ?,
                            poster = ?,
                            blur_data =?,
                            backdrop = ?,
                            plot = ?
            WHERE id = ?",
            db_show.title,
            db_show.release_date,
            db_show.poster,
            db_show.blur_data,
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
        let db_season = metadata.into_db_season(show_id).await;
        let q = sqlx::query!(
            "UPDATE seasons SET
                               show_id = ?,
                               number = ?,
                               release_date = ?,
                               plot = ?,
                               poster = ?,
                               blur_data = ?,
                               show_id = ?
            WHERE id = ?",
            db_season.show_id,
            db_season.number,
            db_season.release_date,
            db_season.plot,
            db_season.poster,
            db_season.blur_data,
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
        let blur_data = if let Some(poster) = &metadata.poster {
            poster.generate_blur_data().await.ok()
        } else {
            None
        };
        let number = metadata.number as i32;
        let poster = metadata.poster.map(|p| p.as_str().to_string());
        let q = sqlx::query!(
            "UPDATE episodes SET
                                season_id = ?,
                                title = ?, 
                                number = ?,
                                plot = ?,
                                poster = ?,
                                blur_data = ?,
                                release_date = ?
            WHERE id = ?",
            season_id,
            metadata.title,
            number,
            metadata.plot,
            poster,
            blur_data,
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
                    .map(|x| x.parse().unwrap())
                    .collect();
                ShowMetadata {
                    metadata_id: show.id.unwrap().to_string(),
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
        let seasons = show
            .seasons
            .split(',')
            .map(|x| x.parse().unwrap())
            .collect();
        Ok(ShowMetadata {
            metadata_id: show.id.unwrap().to_string(),
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
                    .map(|x| x.parse().unwrap())
                    .collect();
                ShowMetadata {
                    metadata_id: show.id.unwrap().to_string(),
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
            r#"SELECT episodes.*, seasons.number as "season_number", videos.duration FROM episodes
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

impl Into<MovieMetadata> for DbMovie {
    fn into(self) -> MovieMetadata {
        let poster = self.poster.map(|p| MetadataImage::new(p.parse().unwrap()));
        let backdrop = self
            .backdrop
            .map(|b| MetadataImage::new(b.parse().unwrap()));

        MovieMetadata {
            metadata_id: self.id.unwrap().to_string(),
            metadata_provider: MetadataProvider::Local,
            poster,
            backdrop,
            plot: self.plot,
            release_date: self.release_date,
            title: self.title,
        }
    }
}

impl Into<ExternalIdMetadata> for DbExternalId {
    fn into(self) -> ExternalIdMetadata {
        ExternalIdMetadata {
            provider: MetadataProvider::from_str(&self.metadata_provider).unwrap(),
            id: self.metadata_id,
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
    pub blur_data: Option<String>,
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
    pub blur_data: Option<String>,
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
    pub blur_data: Option<String>,
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
    pub blur_data: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbVideo {
    pub id: Option<i64>,
    pub path: String,
    pub resources_folder: String,
    pub size: i64,
    pub duration: i64,
    pub scan_date: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbSubtitles {
    pub id: Option<i64>,
    pub language: String,
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub video_id: i64,
}

#[derive(Debug, Clone, FromRow, Serialize, Default)]
pub struct DbExternalId {
    pub id: Option<i64>,
    pub metadata_provider: String,
    pub metadata_id: String,
    pub show_id: Option<i64>,
    pub season_id: Option<i64>,
    pub episode_id: Option<i64>,
    pub movie_id: Option<i64>,
    pub is_prime: i64,
}
