use std::path::PathBuf;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use axum_extra::headers::Range;
use axum_extra::TypedHeader;
use serde::Serialize;
use sqlx::FromRow;

use crate::ffmpeg::{FFprobeAudioStream, FFprobeVideoStream};
use crate::library::{AudioCodec, Resolution, Summary, VideoCodec};
use crate::{app_state::AppState, db::Db};

use super::{
    EpisodeQuery, IdQuery, LanguageQuery, NumberQuery, PageQuery, SeasonQuery, VariantQuery,
};

fn sqlx_err_wrap(err: sqlx::Error) -> StatusCode {
    match err {
        sqlx::Error::RowNotFound => StatusCode::NOT_FOUND,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(Debug, Serialize, FromRow)]
pub struct DetailedShow {
    pub seasons_count: i64,
    pub episodes_count: i64,
    pub id: i64,
    pub title: String,
    pub release_date: String,
    pub poster: Option<String>,
    pub blur_data: Option<String>,
    pub backdrop: Option<String>,
    pub rating: f64,
    pub plot: String,
    pub original_language: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct DetailedSeason {
    pub episodes_count: i64,
    pub id: i64,
    pub show_id: i64,
    pub number: i64,
    pub release_date: String,
    pub plot: String,
    pub rating: f64,
    pub poster: Option<String>,
    pub blur_data: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct DetailedEpisode {
    pub duration: Duration,
    #[sqlx(default)]
    pub previews_amount: i64,
    pub subtitles_amount: i32,
    pub id: i64,
    pub video_id: i64,
    pub season_id: i64,
    pub title: String,
    pub number: i64,
    pub plot: String,
    pub release_date: String,
    pub rating: f64,
    pub poster: String,
    pub blur_data: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct DetailedVideo {
    pub id: i64,
    pub path: PathBuf,
    pub hash: String,
    pub local_title: String,
    pub size: u64,
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
    pub variants: Vec<DetailedVariant>,
    pub scan_date: String,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DetailedVariant {
    pub id: String,
    pub video_id: i64,
    pub path: PathBuf,
    pub hash: String,
    pub size: i64,
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DetailedAudioTrack {
    pub is_default: bool,
    pub sample_rate: String,
    pub channels: i32,
    pub profile: Option<String>,
    pub codec: AudioCodec,
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DetailedVideoTrack {
    pub is_default: bool,
    pub resolution: Resolution,
    pub profile: String,
    pub level: i32,
    pub bitrate: usize,
    pub framerate: f64,
    pub codec: VideoCodec,
}

impl DetailedVideoTrack {
    pub fn from_video_stream(stream: FFprobeVideoStream<'_>, bitrate: usize) -> Self {
        DetailedVideoTrack {
            is_default: stream.is_default(),
            resolution: stream.resoultion(),
            profile: stream.profile.to_string(),
            level: stream.level,
            bitrate,
            framerate: stream.framerate(),
            codec: stream.codec(),
        }
    }
}

impl Into<DetailedAudioTrack> for FFprobeAudioStream<'_> {
    fn into(self) -> DetailedAudioTrack {
        DetailedAudioTrack {
            is_default: self.disposition.default == 1,
            sample_rate: self.sample_rate.to_string(),
            channels: self.channels,
            profile: self.profile.map(|x| x.to_string()),
            codec: self.codec(),
        }
    }
}

pub async fn previews(
    Query(video_id): Query<IdQuery>,
    Query(number): Query<NumberQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let AppState { library, db, .. } = state;
    let video = sqlx::query!("SELECT * FROM videos WHERE id = ?", video_id.id)
        .fetch_one(&db.pool)
        .await
        .map_err(sqlx_err_wrap)?;
    let video_path = PathBuf::from(video.path);
    let file = {
        let library = library.lock().unwrap();
        library.find_library_file(&video_path)
    };
    if let Some(file) = file {
        return Ok(file.serve_previews(number.number).await);
    } else {
        return Err(StatusCode::NOT_FOUND);
    };
}

pub async fn subtitles(
    Query(video_id): Query<IdQuery>,
    Query(lang): Query<LanguageQuery>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, StatusCode> {
    let AppState { library, db, .. } = state;
    let video = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id.id)
        .fetch_one(&db.pool)
        .await
        .map_err(sqlx_err_wrap)?;
    let video_path = PathBuf::from(video.path);
    let file = {
        let library = library.lock().unwrap();
        library.find_library_file(&video_path)
    };
    if let Some(file) = file {
        return Ok(file.serve_subs(lang.lang).await);
    } else {
        return Err(StatusCode::NOT_FOUND);
    };
}

pub async fn watch(
    Query(video_id): Query<IdQuery>,
    variant: Option<Query<VariantQuery>>,
    State(state): State<AppState>,
    range: Option<TypedHeader<Range>>,
) -> Result<impl IntoResponse, StatusCode> {
    let AppState { library, db, .. } = state;
    let video = sqlx::query!("SELECT path FROM videos WHERE id = ?", video_id.id)
        .fetch_one(&db.pool)
        .await
        .map_err(sqlx_err_wrap)?;
    let path = PathBuf::from(video.path);
    let file = {
        let library = library.lock().unwrap();
        library.find_source(&path).map(|x| x.clone())
    }
    .ok_or(StatusCode::NOT_FOUND)?;
    if let Some(Query(VariantQuery { variant })) = variant {
        let file = file.find_variant(&variant).ok_or(StatusCode::NOT_FOUND)?;
        return Ok(file.serve(range).await);
    }
    return Ok(file.origin.serve(range).await);
}

pub async fn get_summary(State(state): State<AppState>) -> Json<Vec<Summary>> {
    let library = state.library.lock().unwrap();
    return Json(library.get_summary());
}

pub async fn get_all_shows(
    Query(q): Query<PageQuery>,
    State(db): State<Db>,
) -> Result<Json<Vec<DetailedShow>>, StatusCode> {
    const PAGE_SIZE: i32 = 20;
    let page = (q.page.unwrap_or(1) - 1).max(0) as i32;
    let offset = page * PAGE_SIZE;
    let shows = sqlx::query_as!(DetailedShow,
        r#"SELECT shows.id as "id!", shows.title as "title!", shows.release_date as "release_date!", shows.poster,
        shows.blur_data, shows.backdrop,
        shows.rating as "rating!", shows.plot as "plot!", shows.original_language as "original_language!",
        (SELECT COUNT(*) FROM seasons WHERE seasons.show_id = shows.id) AS "seasons_count!: i32",
        (SELECT COUNT(*) FROM episodes
            WHERE episodes.season_id IN ( SELECT id FROM seasons WHERE seasons.show_id = shows.id)
        ) AS "episodes_count!: i32"
        FROM shows LIMIT ? OFFSET ?;"#,
        PAGE_SIZE,
        offset
    )
    .fetch_all(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    return Ok(Json(shows));
}

pub async fn get_show_by_id(
    Query(q): Query<IdQuery>,
    State(db): State<Db>,
) -> Result<Json<DetailedShow>, StatusCode> {
    let show = sqlx::query_as!(DetailedShow,
        r#"SELECT shows.id as "id!", shows.title as "title!", shows.release_date as "release_date!", shows.poster,
        shows.blur_data, shows.backdrop,
        shows.rating as "rating!", shows.plot as "plot!", shows.original_language as "original_language!",
        (SELECT COUNT(*) as seasons_count FROM seasons WHERE seasons.show_id = shows.id) as "seasons_count!: i64",
        (SELECT COUNT(*) as episodes_count FROM episodes
            WHERE episodes.season_id IN ( SELECT id FROM seasons WHERE seasons.show_id = shows.id)
        ) as "episodes_count!: i64"
        FROM shows WHERE shows.id = ?;"#,
        q.id,
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    return Ok(Json(show));
}

pub async fn get_seasons(
    Query(query): Query<IdQuery>,
    State(db): State<Db>,
) -> Result<Json<Vec<DetailedSeason>>, StatusCode> {
    let seasons = sqlx::query_as!(
        DetailedSeason,
        r#"SELECT id as "id!", release_date as "release_date!", 
        poster, blur_data, number as "number!", show_id as "show_id!",
        rating as "rating!", plot as "plot!",
        (SELECT COUNT(*) as episodes_count FROM episodes WHERE episodes.season_id = seasons.id) as "episodes_count!: i64"
        FROM seasons
        WHERE show_id = ? ORDER BY number ASC;"#,
        query.id
    )
    .fetch_all(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    return Ok(Json(seasons));
}

pub async fn get_season_by_id(
    Query(query): Query<IdQuery>,
    State(db): State<Db>,
) -> Result<Json<DetailedSeason>, StatusCode> {
    let season = sqlx::query_as!(
        DetailedSeason,
        r#"SELECT seasons.id as "id!", seasons.release_date as "release_date!", 
        seasons.poster, seasons.blur_data, seasons.number as "number!", seasons.show_id as "show_id!",
        seasons.rating as "rating!", seasons.plot as "plot!", COUNT(episodes.id) AS episodes_count FROM shows
        JOIN seasons ON seasons.show_id = shows.id JOIN episodes ON seasons.id = episodes.season_id
        WHERE seasons.id = ?;"#,
        query.id
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    return Ok(Json(season));
}

pub async fn get_season(
    Query(show_id): Query<IdQuery>,
    Query(season): Query<SeasonQuery>,
    State(db): State<Db>,
) -> Result<Json<DetailedSeason>, StatusCode> {
    let seasons = sqlx::query_as!(
        DetailedSeason,
        r#"SELECT seasons.id as "id!", seasons.release_date as "release_date!", 
        seasons.poster, seasons.blur_data, seasons.number as "number!", seasons.show_id as "show_id!",
        seasons.rating as "rating!", seasons.plot as "plot!", COUNT(episodes.id) AS episodes_count FROM shows
        JOIN seasons ON seasons.show_id = shows.id JOIN episodes ON seasons.id = episodes.season_id
        WHERE shows.id = ? AND seasons.number = ?;"#,
        show_id.id,
        season.season
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    return Ok(Json(seasons));
}

pub async fn get_episodes(
    Query(show_id): Query<IdQuery>,
    Query(season): Query<SeasonQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<DetailedEpisode>>, StatusCode> {
    let AppState { library, db, .. } = state;
    let db_episodes = sqlx::query!(
        r#"SELECT episodes.id as "id!", episodes.title as "title!", episodes.release_date as "release_date!", 
        episodes.poster as "poster!", episodes.blur_data, episodes.number as "number!", episodes.video_id as "video_id!",
        episodes.season_id as "season_id!", episodes.rating as "rating!",
        episodes.plot as "plot!", videos.path as "path!",
        (SELECT COUNT(*) FROM subtitles WHERE subtitles.video_id = episodes.video_id) as "subtitles_amount!: i32"
        FROM episodes
        JOIN seasons ON seasons.id = episodes.season_id
        JOIN videos ON episodes.video_id = videos.id
        LEFT JOIN subtitles ON videos.id = subtitles.video_id
        WHERE seasons.show_id = ? AND seasons.number = ? ORDER BY episodes.number ASC"#,
        show_id.id,
        season.season
    )
    .fetch_all(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;
    let library = library.lock().unwrap();
    let episodes: Vec<DetailedEpisode> = db_episodes
        .into_iter()
        .filter_map(|db_episode| {
            if let Some(file) = library.find_source(&PathBuf::from(db_episode.path)) {
                let previews_amount = file.previews_count();
                Some(DetailedEpisode {
                    duration: file.duration(),
                    previews_amount: previews_amount as i64,
                    subtitles_amount: db_episode.subtitles_amount,
                    id: db_episode.id,
                    video_id: db_episode.video_id,
                    season_id: db_episode.season_id,
                    title: db_episode.title,
                    number: db_episode.number,
                    plot: db_episode.plot,
                    release_date: db_episode.release_date,
                    rating: db_episode.rating,
                    poster: db_episode.poster,
                    blur_data: db_episode.blur_data,
                })
            } else {
                None
            }
        })
        .collect();

    return Ok(Json(episodes));
}

pub async fn get_season_episodes_by_id(
    Query(query): Query<IdQuery>,
    State(state): State<AppState>,
) -> Result<Json<Vec<DetailedEpisode>>, StatusCode> {
    let AppState { library, db, .. } = state;
    let episodes = sqlx::query!(
        r#"SELECT episodes.id as "id!", episodes.title as "title!", episodes.release_date as "release_date!", 
        episodes.poster as "poster!", episodes.blur_data, episodes.number as "number!", episodes.video_id as "video_id!",
        episodes.season_id as "season_id!",  episodes.rating as "rating!",
        episodes.plot as "plot!", videos.path as "path!",
        (SELECT COUNT(*) FROM subtitles WHERE subtitles.video_id = episodes.video_id) as "subtitles_amount!: i32"
        FROM episodes
        JOIN seasons ON seasons.id = episodes.season_id
        JOIN videos ON episodes.video_id = videos.id
        LEFT JOIN subtitles ON videos.id = subtitles.video_id
        WHERE episodes.season_id = ? ORDER BY episodes.number"#,
        query.id
    )
    .fetch_all(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    let library = library.lock().unwrap();
    let episodes: Vec<DetailedEpisode> = episodes
        .into_iter()
        .filter_map(|db_episode| {
            if let Some(file) = library.find_source(&PathBuf::from(db_episode.path)) {
                let previews_amount = file.previews_count();
                Some(DetailedEpisode {
                    duration: file.duration(),
                    previews_amount: previews_amount as i64,
                    subtitles_amount: db_episode.subtitles_amount,
                    id: db_episode.id,
                    video_id: db_episode.video_id,
                    season_id: db_episode.season_id,
                    title: db_episode.title,
                    number: db_episode.number,
                    plot: db_episode.plot,
                    release_date: db_episode.release_date,
                    rating: db_episode.rating,
                    poster: db_episode.poster,
                    blur_data: db_episode.blur_data,
                })
            } else {
                None
            }
        })
        .collect();
    return Ok(Json(episodes));
}

pub async fn get_episode(
    Query(show_id): Query<IdQuery>,
    Query(season): Query<SeasonQuery>,
    Query(episode): Query<EpisodeQuery>,
    State(state): State<AppState>,
) -> Result<Json<DetailedEpisode>, StatusCode> {
    let AppState { library, db, .. } = state;
    let db_episode = sqlx::query!(
        r#"SELECT episodes.id as "id!", episodes.title as "title!", episodes.release_date as "release_date!", 
        episodes.poster as "poster!", episodes.blur_data, episodes.number as "number!", episodes.video_id as "video_id!",
        episodes.season_id as "season_id!", episodes.rating as "rating!",
        episodes.plot as "plot!", videos.path as "path!",
        COUNT(subtitles.id) as "subtitles_amount!"
        FROM episodes
        JOIN seasons ON seasons.id = episodes.season_id
        JOIN shows ON seasons.show_id = shows.id
        JOIN videos ON episodes.video_id = videos.id
        LEFT JOIN subtitles ON videos.id = subtitles.video_id
        WHERE shows.id = ? AND seasons.number = ? AND episodes.number = ?;"#,
        show_id.id,
        season.season,
        episode.episode
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    let library = library.lock().unwrap();
    if let Some(file) = library.find_source(&PathBuf::from(db_episode.path.clone())) {
        let previews_amount = file.previews_count();
        let episode = DetailedEpisode {
            duration: file.duration(),
            previews_amount: previews_amount as i64,
            subtitles_amount: db_episode.subtitles_amount,
            id: db_episode.id,
            video_id: db_episode.video_id,
            season_id: db_episode.season_id,
            title: db_episode.title,
            number: db_episode.number,
            plot: db_episode.plot,
            release_date: db_episode.release_date,
            rating: db_episode.rating,
            poster: db_episode.poster,
            blur_data: db_episode.blur_data,
        };
        return Ok(Json(episode));
    } else {
        return Err(StatusCode::NOT_FOUND);
    }
}

pub async fn get_episode_by_id(
    Query(query): Query<IdQuery>,
    State(state): State<AppState>,
) -> Result<Json<DetailedEpisode>, StatusCode> {
    let AppState { library, db, .. } = state;
    let db_episode = sqlx::query!(
        r#"SELECT episodes.id as "id!", episodes.title as "title!", episodes.release_date as "release_date!", 
        episodes.poster as "poster!", episodes.blur_data, episodes.number as "number!", episodes.video_id as "video_id!",
        episodes.season_id as "season_id!", episodes.rating as "rating!",
        episodes.plot as "plot!", videos.path as "path!",
        COUNT(subtitles.id) as "subtitles_amount!"
        FROM episodes
        JOIN seasons ON seasons.id = episodes.season_id
        JOIN videos ON episodes.video_id = videos.id
        LEFT JOIN subtitles ON videos.id = subtitles.video_id
        WHERE episodes.id = ?"#,
        query.id
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;

    let library = library.lock().unwrap();
    if let Some(file) = library.find_source(&PathBuf::from(db_episode.path)) {
        let previews_amount = file.previews_count();
        let episode = DetailedEpisode {
            duration: file.duration(),
            previews_amount: previews_amount as i64,
            subtitles_amount: db_episode.subtitles_amount,
            id: db_episode.id,
            video_id: db_episode.video_id,
            season_id: db_episode.season_id,
            title: db_episode.title,
            number: db_episode.number,
            plot: db_episode.plot,
            release_date: db_episode.release_date,
            rating: db_episode.rating,
            poster: db_episode.poster,
            blur_data: db_episode.blur_data,
        };
        return Ok(Json(episode));
    } else {
        return Err(StatusCode::NOT_FOUND);
    }
}

pub async fn get_video_by_id(
    Query(query): Query<IdQuery>,
    State(state): State<AppState>,
) -> Result<Json<DetailedVideo>, StatusCode> {
    let AppState { library, db, .. } = state;
    let db_video = sqlx::query!(
        "SELECT hash, scan_date, path FROM videos WHERE id = ?",
        query.id
    )
    .fetch_one(&db.pool)
    .await
    .map_err(sqlx_err_wrap)?;
    let (title, source) = {
        let library = library.lock().unwrap();
        let file = library.find(db_video.path).ok_or(StatusCode::NOT_FOUND)?;
        (file.title(), file.source().clone())
    };

    let detailed_variants = source
        .variants
        .iter()
        .map(|v| {
            let id = v
                .path
                .file_stem()
                .expect("file to have stem like {size}.{hash}");
            DetailedVariant {
                id: id.to_string_lossy().to_string(),
                video_id: query.id,
                path: v.path.clone(),
                // TODO: cache hash
                hash: "".to_string(),
                size: v.file_size() as i64,
                duration: v.duration(),
                video_tracks: v
                    .video_streams()
                    .into_iter()
                    .map(|s| DetailedVideoTrack::from_video_stream(s, v.bitrate()))
                    .collect(),
                audio_tracks: v.audio_streams().into_iter().map(|s| s.into()).collect(),
            }
        })
        .collect();

    let date = db_video.scan_date.expect("scan date always defined");
    let detailed_episode = DetailedVideo {
        id: query.id,
        path: source.source_path().to_path_buf(),
        hash: db_video.hash,
        local_title: title,
        size: source.origin.file_size(),
        duration: source.duration(),
        variants: detailed_variants,
        scan_date: date.to_string(),
        video_tracks: source
            .origin
            .video_streams()
            .into_iter()
            .map(|s| DetailedVideoTrack::from_video_stream(s, source.origin.bitrate()))
            .collect(),
        audio_tracks: source
            .origin
            .audio_streams()
            .into_iter()
            .map(|s| s.into())
            .collect(),
    };
    Ok(Json(detailed_episode))
}
