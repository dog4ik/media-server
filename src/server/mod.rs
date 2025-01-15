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

pub mod server_api;
pub mod torrent_api;

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct SerdeDuration {
    pub secs: u64,
    pub nanos: u32,
}

#[derive(OpenApi)]
#[openapi(
    paths(
        server_api::all_local_shows,
        server_api::local_episode,
        server_api::local_episode_by_video_id,
        server_api::local_movie_by_video_id,
        server_api::all_local_movies,
        server_api::external_to_local_id,
        server_api::external_ids,
        server_api::get_movie,
        server_api::fix_show_metadata,
        server_api::fix_movie_metadata,
        server_api::fix_metadata,
        server_api::reset_show_metadata,
        server_api::reset_movie_metadata,
        server_api::reset_metadata,
        server_api::alter_movie_metadata,
        server_api::movie_poster,
        server_api::movie_backdrop,
        server_api::get_show,
        server_api::alter_show_metadata,
        server_api::show_poster,
        server_api::show_backdrop,
        server_api::get_season,
        server_api::season_poster,
        server_api::alter_season_metadata,
        server_api::get_episode,
        server_api::alter_episode_metadata,
        server_api::episode_poster,
        server_api::get_all_variants,
        server_api::contents_video,
        server_api::get_video_by_id,
        server_api::remove_video,
        server_api::pull_video_subtitle,
        server_api::previews,
        server_api::generate_previews,
        server_api::delete_previews,
        server_api::transcode_video,
        server_api::watch,
        server_api::watch_episode,
        server_api::watch_movie,
        server_api::remove_variant,
        server_api::all_history,
        server_api::update_video_history,
        server_api::remove_video_history,
        server_api::clear_history,
        server_api::video_history,
        server_api::remove_history_item,
        server_api::update_history,
        server_api::suggest_movies,
        server_api::suggest_shows,
        server_api::search_torrent,
        server_api::search_content,
        server_api::get_trending_shows,
        server_api::get_trending_movies,
        server_api::server_configuration,
        server_api::server_version,
        server_api::update_server_configuration,
        server_api::reset_server_configuration,
        server_api::server_capabilities,
        server_api::order_providers,
        server_api::get_providers_order,
        server_api::latest_log,
        server_api::transcode_tasks,
        server_api::cancel_transcode_task,
        server_api::previews_tasks,
        server_api::cancel_previews_task,
        server_api::progress,
        server_api::reconciliate_lib,
        server_api::clear_db,
        server_api::create_transcode_stream,
        server_api::transcode_stream_manifest,
        server_api::transcoded_segment,
        server_api::browse_directory,
        server_api::parent_directory,
        server_api::root_dirs,
        server_api::detect_intros,
        server_api::intro_detection_tasks,
        server_api::video_content_metadata,
        server_api::delete_episode,
        server_api::delete_season,
        server_api::delete_show,
        server_api::delete_movie,
        torrent_api::all_torrents,
        torrent_api::set_file_priority,
        torrent_api::resolve_magnet_link,
        torrent_api::parse_torrent_file,
        torrent_api::open_torrent,
        torrent_api::open_torrent_file,
        torrent_api::torrent_state,
        torrent_api::updates,
        torrent_api::delete_torrent,
        torrent_api::output_location,
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
            server_api::DetailedVideo,
            server_api::DetailedVideoTrack,
            server_api::DetailedAudioTrack,
            server_api::DetailedSubtitleTrack,
            server_api::DetailedVariant,
            server_api::MovieHistory,
            server_api::ShowHistory,
            server_api::VideoContentMetadata,
            server_api::Intro,
            server_api::ProviderOrder,
            server_api::UpdateHistoryPayload,
            server_api::ShowSuggestion,
            server_api::MovieHistory,
            server_api::ProviderType,
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
            torrent::DownloadProgress,
            torrent::TorrentProgress,
            torrent::PeerStateChange,
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
