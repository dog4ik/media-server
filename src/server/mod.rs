use crate::app_state;
use crate::config;
use crate::db;
use crate::ffmpeg;
use crate::file_browser;
use crate::library;
use crate::metadata;
use crate::progress;
use crate::torrent;
use crate::torrent_index;
use crate::tracing;
use base64::Engine;
use serde::de::Visitor;
use serde::Deserialize;
use utoipa::OpenApi;

pub mod admin_api;
pub mod public_api;
pub mod torrent_api;

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct SerdeDuration {
    pub secs: u64,
    pub nanos: u32,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        public_api::all_local_shows,
        public_api::local_episode,
        public_api::local_episode_by_video_id,
        public_api::local_movie_by_video_id,
        public_api::all_local_movies,
        public_api::external_to_local_id,
        public_api::external_ids,
        public_api::get_movie,
        admin_api::fix_show_metadata,
        admin_api::fix_movie_metadata,
        admin_api::fix_metadata,
        admin_api::reset_show_metadata,
        admin_api::reset_movie_metadata,
        admin_api::reset_metadata,
        admin_api::alter_movie_metadata,
        public_api::movie_poster,
        public_api::movie_backdrop,
        public_api::get_show,
        admin_api::alter_show_metadata,
        public_api::show_poster,
        public_api::show_backdrop,
        public_api::get_season,
        public_api::season_poster,
        admin_api::alter_season_metadata,
        public_api::get_episode,
        admin_api::alter_episode_metadata,
        public_api::episode_poster,
        public_api::get_all_variants,
        public_api::contents_video,
        public_api::get_video_by_id,
        admin_api::remove_video,
        public_api::pull_video_subtitle,
        public_api::previews,
        admin_api::generate_previews,
        admin_api::delete_previews,
        admin_api::transcode_video,
        public_api::watch,
        public_api::watch_episode,
        public_api::watch_movie,
        admin_api::remove_variant,
        public_api::all_history,
        admin_api::update_video_history,
        admin_api::remove_video_history,
        admin_api::clear_history,
        public_api::video_history,
        admin_api::remove_history_item,
        admin_api::update_history,
        public_api::suggest_movies,
        public_api::suggest_shows,
        public_api::search_torrent,
        admin_api::resolve_magnet_link,
        admin_api::parse_torrent_file,
        admin_api::download_torrent,
        public_api::search_content,
        public_api::get_trending_shows,
        public_api::get_trending_movies,
        admin_api::server_configuration,
        admin_api::update_server_configuration,
        admin_api::reset_server_configuration,
        admin_api::server_capabilities,
        admin_api::order_providers,
        admin_api::get_providers_order,
        admin_api::latest_log,
        admin_api::transcode_tasks,
        admin_api::cancel_transcode_task,
        admin_api::previews_tasks,
        admin_api::cancel_previews_task,
        admin_api::progress,
        admin_api::reconciliate_lib,
        admin_api::clear_db,
        admin_api::create_transcode_stream,
        admin_api::transcode_stream_manifest,
        admin_api::transcoded_segment,
        admin_api::browse_directory,
        admin_api::parent_directory,
        admin_api::root_dirs,
        admin_api::detect_intros,
        public_api::video_content_metadata,
        torrent_api::all_torrents,
    ),
    components(
        schemas(
            metadata::MovieMetadata,
            metadata::ShowMetadata,
            metadata::EpisodeMetadata,
            metadata::SeasonMetadata,
            metadata::MetadataProvider,
            metadata::MetadataImage,
            metadata::ExternalIdMetadata,
            metadata::MetadataSearchResult,
            metadata::ContentType,
            metadata::MetadataProvider,
            metadata::Language,
            app_state::AppError,
            app_state::AppErrorKind,
            public_api::DetailedVideo,
            public_api::DetailedVideoTrack,
            public_api::DetailedAudioTrack,
            public_api::DetailedSubtitleTrack,
            public_api::DetailedVariant,
            public_api::MovieHistory,
            public_api::ShowHistory,
            public_api::VideoContentMetadata,
            public_api::Intro,
            admin_api::ProviderOrder,
            admin_api::UpdateHistoryPayload,
            public_api::ShowSuggestion,
            public_api::MovieHistory,
            admin_api::ProviderType,
            torrent::DownloadContentHint,
            torrent::TorrentDownloadPayload,
            torrent::TorrentInfo,
            torrent::TorrentShow,
            torrent::TorrentEpisode,
            torrent::TorrentMovie,
            torrent::TorrentContent,
            torrent::TorrentContents,
            torrent::ResolvedTorrentFile,
            torrent::PendingTorrent,
            progress::Task<ffmpeg::TranscodeJob>,
            progress::Task<ffmpeg::PreviewsJob>,
            progress::VideoTaskKind,
            progress::Notification,
            progress::ProgressStatus<f32>,
            progress::TaskProgress,
            tracing::JsonTracingEvent,
            torrent_index::Torrent,
            db::DbHistory,
            db::DbExternalId,
            library::TranscodePayload,
            library::AudioCodec,
            library::VideoCodec,
            library::SubtitlesCodec,
            library::Resolution,
            config::AppResources,
            config::Capabilities,
            config::Codec,
            config::CodecType,
            config::UtoipaConfigSchema,
            config::ConfigurationApplyResult,
            config::ConfigurationApplyError,
            file_browser::BrowseRootDirs,
            file_browser::BrowseDirectory,
            file_browser::BrowseFile,
            SerdeDuration
        )
    ),
    tags(
        (name = "Configuration", description = "Server configuration options"),
        (name = "Shows", description = "Shows, seasons, episodes operations"),
        (name = "Movies", description = "Movies operations"),
        (name = "Metadata", description = "Metadata operations"),
        (name = "History", description = "History operations"),
        (name = "Logs", description = "Log operations"),
        (name = "Tasks", description = "Tasks operations"),
        (name = "Search", description = "Endopoints for searching content"),
        (name = "Torrent", description = "Torrent client operations"),
        (name = "Transcoding", description = "Live transcoding operations"),
        (name = "Videos", description = ""),
    )
)]
pub struct OpenApiDoc;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct PageQuery {
    pub page: Option<usize>,
}

#[derive(utoipa::IntoParams)]
pub struct CursorQuery {
    pub cursor: Option<String>,
}

impl<'de> Deserialize<'de> for CursorQuery {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct CursorVisitor;
        impl<'v> Visitor<'v> for CursorVisitor {
            type Value = CursorQuery;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(formatter, "base64 encoded string / cursor map")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
                let cursor = engine
                    .decode(v)
                    .ok()
                    .and_then(|v| String::from_utf8(v).ok())
                    .ok_or(E::custom("Failed to decode base64 string"))?;
                Ok(CursorQuery {
                    cursor: Some(cursor),
                })
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'v>,
            {
                while let Some((key, val)) = map.next_entry::<String, String>()? {
                    if key == "cursor" {
                        return self.visit_str(&val);
                    }
                }
                Ok(CursorQuery { cursor: None })
            }
        }
        deserializer.deserialize_map(CursorVisitor)
    }
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct IdQuery {
    pub id: i64,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct SearchQuery {
    pub search: String,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ContentTypeQuery {
    #[param(inline)]
    pub content_type: metadata::ContentType,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct OptionalContentTypeQuery {
    #[param(inline)]
    pub content_type: Option<metadata::ContentType>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ProviderQuery {
    #[param(inline)]
    pub provider: metadata::MetadataProvider,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct VariantQuery {
    pub variant: Option<String>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct StringIdQuery {
    pub id: String,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct SeasonQuery {
    pub season: usize,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct EpisodeQuery {
    pub episode: usize,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct NumberQuery {
    pub number: usize,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct LanguageQuery {
    pub lang: Option<String>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct TakeParam {
    pub take: Option<usize>,
}
