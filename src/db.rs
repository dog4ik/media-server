use std::path::PathBuf;

use serde::Serialize;
use sqlx::{sqlite::SqlitePoolOptions, Error, FromRow, Sqlite, SqlitePool};

use crate::metadata_provider::{EpisodeMetadata, SeasonMetadata, ShowMetadata};

#[derive(Debug, Clone)]
pub struct Db {
    pub pool: SqlitePool,
}

impl Db {
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await?;
        sqlx::query!(
r#"CREATE TABLE IF NOT EXISTS shows (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    metadata_id TEXT,
                                    metadata_provider TEXT NOT NULL,
                                    title TEXT NOT NULL, 
                                    release_date TEXT NOT NULL,
                                    poster TEXT,
                                    blur_data TEXT,
                                    backdrop TEXT,
                                    rating FLOAT NOT NULL,
                                    plot TEXT NOT NULL,
                                    original_language TEXT NOT NULL,
                                    UNIQUE (metadata_id, metadata_provider));
CREATE TABLE IF NOT EXISTS seasons (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    metadata_id TEXT,
                                    metadata_provider TEXT NOT NULL,
                                    show_id INTEGER NOT NULL,
                                    number INTEGER NOT NULL,
                                    release_date TEXT NOT NULL,
                                    rating FLOAT NOT NULL,
                                    plot TEXT NOT NULL,
                                    poster TEXT,
                                    blur_data TEXT,
                                    UNIQUE (metadata_id, metadata_provider),
                                    FOREIGN KEY (show_id) REFERENCES shows (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS episodes (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    video_id INTEGER NOT NULL UNIQUE,
                                    metadata_id TEXT,
                                    metadata_provider TEXT NOT NULL,
                                    season_id INTEGER NOT NULL,
                                    title TEXT NOT NULL, 
                                    number INTEGER NOT NULL,
                                    plot TEXT NOT NULL,
                                    poster TEXT NOT NULL,
                                    blur_data TEXT,
                                    release_date TEXT NOT NULL,
                                    rating FLOAT NOT NULL,
                                    UNIQUE (metadata_id, metadata_provider),
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE,
                                    FOREIGN KEY (season_id) REFERENCES seasons (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS movies (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    video_id INTEGER NOT NULL UNIQUE,
                                    metadata_id TEXT,
                                    metadata_provider TEXT NOT NULL,
                                    title TEXT NOT NULL,
                                    blur_data TEXT,
                                    backdrop TEXT,
                                    plot TEXT NOT NULL,
                                    rating FLOAT NOT NULL,
                                    poster TEXT,
                                    original_language TEXT NOT NULL,
                                    release_date TEXT NOT NULL,
                                    UNIQUE (metadata_id, metadata_provider),
                                    FOREIGN KEY (video_id) REFERENCES videos (id) ON DELETE CASCADE);
CREATE TABLE IF NOT EXISTS videos (id INTEGER PRIMARY KEY AUTOINCREMENT, 
                                    path TEXT NOT NULL,
                                    hash TEXT NOT NULL,
                                    local_title TEXT NOT NULL,
                                    size INTEGER NOT NULL,
                                    duration INTEGER NOT NULL,
                                    video_codec TEXT,
                                    audio_codec TEXT,
                                    scan_date DATETIME DEFAULT CURRENT_TIMESTAMP);
CREATE TABLE IF NOT EXISTS subtitles (id INTEGER PRIMARY KEY AUTOINCREMENT,
                                    language TEXT NOT NULL,
                                    hash TEXT NOT NULL,
                                    path TEXT NOT NULL,
                                    size INTEGER NOT NULL,
                                    video_id INTEGER NOT NULL,
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
        ",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_movie(&self, movie: DbMovie) -> Result<i64, Error> {
        let query = sqlx::query!(
            "INSERT OR IGNORE INTO movies 
            (metadata_id, metadata_provider, title, release_date, poster,
            blur_data, backdrop, rating, plot, original_language, video_id)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id;",
            movie.metadata_id,
            movie.metadata_provider,
            movie.title,
            movie.release_date,
            movie.poster,
            movie.blur_data,
            movie.backdrop,
            movie.rating,
            movie.plot,
            movie.original_language,
            movie.video_id
        );
        query.fetch_one(&self.pool).await.map(|x| x.id)
    }

    pub async fn insert_show(&self, show: DbShow) -> Result<i64, Error> {
        let query = sqlx::query!(
            "INSERT OR IGNORE INTO shows 
            (metadata_id, metadata_provider, title, release_date, poster, blur_data, backdrop, rating, plot, original_language)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id;",
            show.metadata_id,
            show.metadata_provider,
            show.title,
            show.release_date,
            show.poster,
            show.blur_data,
            show.backdrop,
            show.rating,
            show.plot,
            show.original_language,
        );
        let res = query.fetch_one(&self.pool).await.map(|x| x.id);

        // NOTE: Need another way to return id if ingored insert
        if let Err(err) = res {
            if let Error::RowNotFound = err {
                sqlx::query!(
                    r#"SELECT id as "id!" FROM shows WHERE metadata_id = ? AND metadata_provider = ?"#,
                    show.metadata_id,
                    show.metadata_provider
                )
                .fetch_one(&self.pool)
                .await
                .map(|x|x.id)
            } else {
                return Err(err);
            }
        } else {
            return res;
        }
    }

    pub async fn insert_season(&self, season: DbSeason) -> Result<i64, Error> {
        let query = sqlx::query!(
            "INSERT OR IGNORE INTO seasons
            (metadata_id, metadata_provider, show_id, number, release_date, rating, plot, poster, blur_data)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id;",
            season.metadata_id,
            season.metadata_provider,
            season.show_id,
            season.number,
            season.release_date,
            season.rating,
            season.plot,
            season.poster,
            season.blur_data,
        );

        let res = query.fetch_one(&self.pool).await.map(|x| x.id);
        if let Err(err) = res {
            if let Error::RowNotFound = err {
                sqlx::query!(
                    r#"SELECT id as "id!" FROM seasons WHERE metadata_id = ? AND metadata_provider = ?"#,
                    season.metadata_id,
                    season.metadata_provider
                )
                .fetch_one(&self.pool)
                .await
                .map(|x|x.id)
            } else {
                return Err(err);
            }
        } else {
            return res;
        }
    }

    pub async fn insert_episode(&self, episode: DbEpisode) -> Result<i64, Error> {
        let episode_query = sqlx::query!(
            "INSERT OR IGNORE INTO episodes
            (video_id, metadata_id, metadata_provider, season_id, title, number, plot, release_date, rating, poster, blur_data)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id;",
            episode.video_id,
            episode.metadata_id,
            episode.metadata_provider,
            episode.season_id,
            episode.title,
            episode.number,
            episode.plot,
            episode.release_date,
            episode.rating,
            episode.poster,
            episode.blur_data
        );

        let res = episode_query.fetch_one(&self.pool).await.map(|x| x.id);

        if let Err(err) = res {
            if let Error::RowNotFound = err {
                sqlx::query!(
                    r#"SELECT id as "id!" FROM episodes WHERE metadata_id = ? AND metadata_provider = ?"#,
                    episode.metadata_id,
                    episode.metadata_provider
                )
                .fetch_one(&self.pool)
                .await
                .map(|x|x.id)
            } else {
                return Err(err);
            }
        } else {
            return res;
        }
    }

    pub async fn insert_video(&self, db_video: DbVideo) -> Result<i64, Error> {
        let video_query = sqlx::query!(
            "INSERT INTO videos
            (path, hash, local_title, size, duration, video_codec, audio_codec)
            VALUES (?, ?, ?, ?, ?, ?, ?) RETURNING id;",
            db_video.path,
            db_video.hash,
            db_video.local_title,
            db_video.size,
            db_video.duration,
            db_video.video_codec,
            db_video.audio_codec,
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

    pub async fn remove_video(&self, id: i64) -> Result<(), Error> {
        sqlx::query!("DELETE FROM videos WHERE videos.id = ?;", id)
            .fetch_one(&self.pool)
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
        let id = sqlx::query!("SELECT id FROM videos WHERE path = ?", str_path)
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
        println!("remove episode siblings count: {}", siblings_count);
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
                            metadata_id = ?,
                            metadata_provider = ?,
                            title = ?, 
                            release_date = ?,
                            poster = ?,
                            blur_data =?,
                            backdrop = ?,
                            rating = ?,
                            plot = ?,
                            original_language = ?
            WHERE id = ?",
            db_show.metadata_id,
            db_show.metadata_provider,
            db_show.title,
            db_show.release_date,
            db_show.poster,
            db_show.blur_data,
            db_show.backdrop,
            db_show.rating,
            db_show.plot,
            db_show.original_language,
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
                               metadata_id = ?,
                               metadata_provider = ?,
                               show_id = ?,
                               number = ?,
                               release_date = ?,
                               rating = ?,
                               plot = ?,
                               poster = ?,
                               blur_data = ?,
                               show_id = ?
            WHERE id = ?",
            db_season.metadata_id,
            db_season.metadata_provider,
            db_season.show_id,
            db_season.number,
            db_season.release_date,
            db_season.rating,
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
        let blur_data = metadata.poster.generate_blur_data().await.ok();
        let provider = metadata.metadata_provider.to_string();
        let number = metadata.number as i32;
        let poster = metadata.poster.as_str().to_string();
        let q = sqlx::query!(
            "UPDATE episodes SET
                                metadata_id = ?,
                                metadata_provider = ?,
                                season_id = ?,
                                title = ?, 
                                number = ?,
                                plot = ?,
                                poster = ?,
                                blur_data = ?,
                                release_date = ?,
                                rating = ?
            WHERE id = ?",
            metadata.metadata_id,
            provider,
            season_id,
            metadata.title,
            number,
            metadata.plot,
            poster,
            blur_data,
            metadata.release_date,
            metadata.rating,
            id
        );
        q.fetch_one(&self.pool).await?;
        Ok(())
    }
}

//Types

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbShow {
    pub id: Option<i64>,
    pub metadata_id: Option<String>,
    pub metadata_provider: String,
    pub title: String,
    pub release_date: String,
    pub poster: Option<String>,
    pub blur_data: Option<String>,
    pub backdrop: Option<String>,
    pub rating: f64,
    pub plot: String,
    pub original_language: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbSeason {
    pub id: Option<i64>,
    pub metadata_id: Option<String>,
    pub metadata_provider: String,
    pub show_id: i64,
    pub number: i64,
    pub release_date: String,
    pub plot: String,
    pub rating: f64,
    pub poster: Option<String>,
    pub blur_data: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbMovie {
    pub id: Option<i64>,
    pub video_id: i64,
    pub original_language: String,
    pub metadata_id: Option<String>,
    pub metadata_provider: String,
    pub title: String,
    pub plot: String,
    pub rating: f64,
    pub poster: Option<String>,
    pub release_date: String,
    pub backdrop: Option<String>,
    pub blur_data: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbEpisode {
    pub id: Option<i64>,
    pub video_id: i64,
    pub metadata_id: Option<String>,
    pub metadata_provider: String,
    pub season_id: i64,
    pub title: String,
    pub number: i64,
    pub plot: String,
    pub release_date: String,
    pub rating: f64,
    pub poster: String,
    pub blur_data: Option<String>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DbVideo {
    pub id: Option<i64>,
    pub path: String,
    pub hash: String,
    pub local_title: String,
    pub size: i64,
    pub duration: i64,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
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
