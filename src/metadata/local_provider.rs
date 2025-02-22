use std::{path::PathBuf, sync::Mutex};

use serde::Serialize;

use crate::{
    db::{Db, DbActions, DbHistory},
    ffmpeg::{FFprobeAudioStream, FFprobeSubtitleStream, FFprobeVideoStream},
    library::{
        assets::PreviewsDirAsset, AudioCodec, Library, Resolution, Source, SubtitlesCodec,
        VideoCodec,
    },
    server::SerdeDuration,
};

use super::EpisodeMetadata;

#[derive(Debug, utoipa::ToSchema, Clone)]
pub struct LocalEpisode {
    metadata: EpisodeMetadata,
    history: Option<DbHistory>,
    previews_count: usize,
    intro: Option<Intro>,
    videos: Vec<DetailedVideo>,
}

#[derive(Debug, utoipa::ToSchema, Clone)]
pub struct LocalMovie {
    metadata: EpisodeMetadata,
    history: Option<DbHistory>,
    previews_count: usize,
    videos: Vec<DetailedVideo>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ContentDetails {
    pub previews_count: usize,
    pub videos: Vec<DetailedVideo>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedVideo {
    pub id: String,
    #[schema(value_type = String)]
    pub path: PathBuf,
    pub size: u64,
    #[schema(value_type = SerdeDuration)]
    pub duration: std::time::Duration,
    pub video_tracks: Vec<DetailedVideoTrack>,
    pub audio_tracks: Vec<DetailedAudioTrack>,
    pub subtitle_tracks: Vec<DetailedSubtitleTrack>,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedAudioTrack {
    pub is_default: bool,
    pub sample_rate: String,
    pub channels: i32,
    pub profile: Option<String>,
    pub codec: AudioCodec,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedSubtitleTrack {
    pub is_default: bool,
    pub language: Option<String>,
    pub codec: SubtitlesCodec,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct DetailedVideoTrack {
    pub is_default: bool,
    pub resolution: Resolution,
    pub profile: String,
    pub level: i32,
    pub bitrate: usize,
    pub framerate: f64,
    pub codec: VideoCodec,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Intro {
    start_sec: i64,
    end_sec: i64,
}

impl DetailedVideoTrack {
    pub fn from_video_stream(stream: FFprobeVideoStream<'_>, bitrate: usize) -> Self {
        DetailedVideoTrack {
            is_default: stream.is_default(),
            resolution: stream.resolution(),
            profile: stream.profile.to_string(),
            level: stream.level,
            bitrate,
            framerate: stream.framerate(),
            codec: stream.codec(),
        }
    }
}

impl From<FFprobeAudioStream<'_>> for DetailedAudioTrack {
    fn from(val: FFprobeAudioStream<'_>) -> Self {
        DetailedAudioTrack {
            is_default: val.disposition.default == 1,
            sample_rate: val.sample_rate.to_string(),
            channels: val.channels,
            profile: val.profile.map(|x| x.to_string()),
            codec: val.codec(),
        }
    }
}

impl From<FFprobeSubtitleStream<'_>> for DetailedSubtitleTrack {
    fn from(val: FFprobeSubtitleStream<'_>) -> Self {
        DetailedSubtitleTrack {
            is_default: val.is_default(),
            language: val.language.map(|x| x.to_string()),
            codec: val.codec(),
        }
    }
}

impl DetailedVideo {
    pub async fn from_video(video: &crate::library::Video) -> anyhow::Result<Self> {
        let id = video
            .path()
            .file_stem()
            .expect("file to have stem like {size}.{hash}")
            .to_string_lossy()
            .to_string();
        let metadata = video.metadata().await?;
        Ok(Self {
            id,
            size: video.file_size(),
            duration: metadata.duration(),
            video_tracks: metadata
                .video_streams()
                .into_iter()
                .map(|s| DetailedVideoTrack::from_video_stream(s, metadata.bitrate()))
                .collect(),
            audio_tracks: metadata
                .audio_streams()
                .into_iter()
                .map(Into::into)
                .collect(),
            subtitle_tracks: metadata
                .subtitle_streams()
                .into_iter()
                .map(Into::into)
                .collect(),
            path: video.path().to_path_buf(),
        })
    }
}

impl LocalEpisode {
    pub async fn new(db: Db, library: &Mutex<Library>, local_id: i64) -> anyhow::Result<Self> {
        let episode_metadata = db.get_episode_by_id(local_id).await?;

        let videos = sqlx::query!(
            "SELECT id FROM videos WHERE videos.episode_id = ?",
            local_id
        )
        .fetch_all(&db.pool)
        .await?;

        let details = sqlx::query!(
            r#"SELECT history.time,
        history.id, history.update_time, history.is_finished,
        episode_intro.start_sec, episode_intro.end_sec
        FROM episodes
        JOIN videos ON videos.episode_id = episodes.id
        LEFT JOIN history ON history.video_id = videos.id
        LEFT JOIN episode_intro ON episode_intro.video_id = videos.id
        WHERE episodes.id = ?;"#,
            local_id
        )
        .fetch_one(&db.pool)
        .await?;
        let mut detailed_videos: Vec<DetailedVideo> = Vec::new();

        for video in videos {
            let video = {
                let library = library.lock().unwrap();
                library.get_source(video.id).unwrap().video.clone()
            };
            if let Ok(detailed_video) = DetailedVideo::from_video(&video).await {
                detailed_videos.push(detailed_video);
            }
        }

        let history = details
            .time
            .zip(details.is_finished)
            .zip(details.update_time)
            .map(|((time, is_finished), update_time)| DbHistory {
                id: Some(details.id),
                time,
                is_finished,
                update_time,
                video_id: details.id,
            });

        let intro = details
            .start_sec
            .zip(details.end_sec)
            .map(|(start_sec, end_sec)| Intro { start_sec, end_sec });

        let previews_count = PreviewsDirAsset::new(todo!()).previews_count();

        Ok(Self {
            metadata: episode_metadata,
            videos: detailed_videos,
            previews_count,
            intro,
            history,
        })
    }
}
