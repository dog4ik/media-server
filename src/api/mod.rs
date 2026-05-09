use crate::MediaDuration as CrateDuration;
use crate::OffsetDateTime as CrateOffsetDateTime;
use crate::app_state;
use crate::app_state::AppError;
use crate::config;
use crate::db;
use crate::ffmpeg;
use crate::library;
use crate::metadata;
use crate::progress;
use crate::torrent_index;
use crate::watch;
use crate::ws;
use axum::extract::FromRequestParts;
use axum::extract::path;
use axum::extract::rejection::PathRejection;
use axum::http::request::Parts;
use axum_extra::extract::QueryRejection;
use base64::Engine;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde::de::Visitor;
use utoipa::OpenApi;

/// API data types
///
/// This module defines the data types used by the API, as well as the methods required for their construction.
pub mod api_data;
pub mod file_browser;
pub mod history;
pub mod server;
pub mod subtitles;
/// Torrent client specific endpoints
pub mod torrent;

#[derive(OpenApi)]
#[openapi(
    paths(
        server::all_local_shows,
        server::local_episode,
        server::all_local_movies,
        server::external_ids,
        server::get_movie,
        server::fix_show_metadata,
        server::fix_movie_metadata,
        server::fix_metadata,
        server::reset_show_metadata,
        server::reset_movie_metadata,
        server::reset_metadata,
        server::alter_movie_metadata,
        server::movie_poster,
        server::movie_backdrop,
        server::get_show,
        server::alter_show_metadata,
        server::show_poster,
        server::show_backdrop,
        server::get_season,
        server::season_poster,
        server::alter_season_metadata,
        server::get_episode,
        server::alter_episode_metadata,
        server::episode_poster,
        server::get_all_variants,
        server::contents_video,
        server::get_video_by_id,
        server::remove_video,
        server::previews,
        server::generate_previews,
        server::delete_previews,
        server::transcode_video,
        server::watch,
        server::watch_episode,
        server::watch_movie,
        server::remove_variant,
        server::search_torrent,
        server::search_content,
        server::get_trending_shows,
        server::get_trending_movies,
        server::server_configuration,
        server::server_version,
        server::update_server_configuration,
        server::reset_server_configuration,
        server::server_capabilities,
        server::order_providers,
        server::get_providers_order,
        server::latest_log,
        server::transcode_tasks,
        server::cancel_transcode_task,
        server::previews_tasks,
        server::cancel_previews_task,
        server::watch_sessions,
        server::stop_watch_session,
        server::progress,
        server::reconciliate_lib,
        server::clear_db,
        server::start_direct_stream,
        server::start_hls_stream,
        server::hls_manifest,
        server::hls_segment,
        server::hls_init,
        server::detect_intros,
        server::update_video_intro,
        server::delete_season_intros,
        server::delete_episode_intros,
        server::delete_video_intro,
        server::video_intro,
        server::intro_detection_tasks,
        server::video_content_metadata,
        server::delete_episode,
        server::delete_season,
        server::delete_show,
        server::delete_movie,
        server::actor_poster,
        server::actor_list,
        file_browser::browse_directory,
        file_browser::parent_directory,
        file_browser::root_dirs,
        torrent::all_torrents,
        torrent::session_state,
        torrent::set_files_priority,
        torrent::resolve_magnet_link,
        torrent::parse_torrent_file,
        torrent::open_torrent,
        torrent::open_torrent_file,
        torrent::torrent_state,
        torrent::index_magnet_link,
        torrent::updates,
        torrent::delete_torrent,
        torrent::validate_torrent,
        torrent::output_location,
        torrent::batch_action,
        history::all_history,
        history::update_video_history,
        history::remove_video_history,
        history::clear_history,
        history::remove_history_item,
        history::update_history,
        history::suggest_movies,
        history::suggest_shows,
        subtitles::pull_video_subtitle,
        subtitles::upload_subtitles,
        subtitles::delete_subtitles,
        subtitles::get_subtitles,
        subtitles::reference_external_subtitles,
        ws::ws,
    ),
    components(
        schemas(
            metadata::MovieMetadata,
            metadata::ShowMetadata,
            metadata::EpisodeMetadata,
            metadata::SeasonMetadata,
            metadata::MetadataProvider,
            metadata::ExternalIdMetadata,
            metadata::MetadataSearchResult,
            metadata::ContentType,
            metadata::MetadataProvider,
            metadata::Language,
            app_state::AppError,
            app_state::AppErrorKind,
            server::DetailedVideo,
            server::DetailedVideoTrack,
            server::DetailedAudioTrack,
            server::DetailedSubtitleTrack,
            server::DetailedVariant,
            server::VideoContentMetadata,
            server::Intro,
            server::ProviderOrder,
            history::UpdateHistoryPayload,
            history::ShowSuggestion,
            history::MovieHistory,
            crate::torrent::DownloadContentHint,
            crate::torrent::TorrentDownloadPayload,
            crate::torrent::TorrentInfo,
            crate::torrent::TorrentShow,
            crate::torrent::TorrentEpisode,
            crate::torrent::TorrentMovie,
            crate::torrent::TorrentContent,
            crate::torrent::TorrentContents,
            crate::torrent::ResolvedTorrentFile,
            crate::torrent::PendingTorrent,
            crate::torrent::DownloadState,
            crate::torrent::TorrentProgress,
            crate::torrent::PeerStateChange,
            progress::Task<ffmpeg::TranscodeJob>,
            progress::Task<ffmpeg::PreviewsJob>,
            progress::Task<watch::WatchTask>,
            progress::VideoTaskKind,
            progress::Notification,
            progress::ProgressStatus<f32>,
            progress::TaskProgress,
            crate::tracing::JsonTracingEvent,
            torrent_index::Torrent,
            db::DbExternalId,
            library::TranscodePayload,
            library::media::codec::audio::AudioCodec,
            library::media::codec::video::VideoCodec,
            library::media::codec::subtitles::SubtitlesCodec,
            library::media::Resolution,
            config::AppResources,
            config::Capabilities,
            config::Codec,
            config::CodecType,
            config::UtoipaConfigSchema,
            config::ConfigurationApplyResult,
            config::ConfigurationApplyError,
            ws::WsRequest,
            ws::WsMessage,
            CrateDuration,
            CrateOffsetDateTime,
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
        (name = "Watch", description = "Content watching operations"),
        (name = "Videos", description = "Video files operations"),
        (name = "Subtitles", description = "Subtitles operations"),
        (name = "Actors", description = "Actors operations"),
    )
)]
pub struct OpenApiDoc;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct PageQuery {
    pub page: Option<usize>,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ContentFilterQuery {
    #[serde(default)]
    pub actors: Vec<i64>,
    pub search: Option<String>,
    pub take: Option<i64>,
    pub cursor: Option<String>,
}

impl From<ContentFilterQuery> for db::ContentFetchParams {
    fn from(filter: ContentFilterQuery) -> Self {
        db::ContentFetchParams {
            take: filter.take,
            cursor: filter.cursor,
            search: filter.search,
            actors: (!filter.actors.is_empty()).then_some(filter.actors),
        }
    }
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
pub struct UuidQuery {
    pub id: uuid::Uuid,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct OptionalUuidQuery {
    pub id: Option<uuid::Uuid>,
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
pub struct TorrentIndexQuery {
    #[param(inline)]
    pub provider: torrent_index::TorrentIndexIdentifier,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct OptionalTorrentIndexQuery {
    #[param(inline)]
    pub provider: Option<torrent_index::TorrentIndexIdentifier>,
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
pub struct TakeQuery {
    pub take: Option<i64>,
}

/// `Path` extractor wrapper that customizes the error from `axum::extract::Path`
pub struct Path<T>(T);

impl<S, T> FromRequestParts<S> for Path<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(value) => Ok(Self(value.0)),
            Err(rejection) => {
                let error = match rejection {
                    PathRejection::FailedToDeserializePathParams(inner) => {
                        let kind = inner.into_kind();
                        match &kind {
                            path::ErrorKind::WrongNumberOfParameters { .. } => {
                                AppError::bad_request(kind.to_string())
                            }

                            path::ErrorKind::ParseErrorAtKey { .. } => {
                                AppError::bad_request(kind.to_string())
                            }

                            path::ErrorKind::ParseErrorAtIndex { .. } => {
                                AppError::bad_request(kind.to_string())
                            }

                            path::ErrorKind::ParseError { .. } => {
                                AppError::bad_request(kind.to_string())
                            }

                            path::ErrorKind::InvalidUtf8InPathParam { .. } => {
                                AppError::bad_request(kind.to_string())
                            }

                            path::ErrorKind::UnsupportedType { .. } => {
                                AppError::internal_error(kind.to_string())
                            }

                            path::ErrorKind::Message(msg) => AppError::bad_request(msg.clone()),

                            _ => AppError::internal_error(format!(
                                "Unhandled deserialization error: {kind}"
                            )),
                        }
                    }
                    PathRejection::MissingPathParams(error) => {
                        AppError::internal_error(error.to_string())
                    }

                    _ => AppError::internal_error(format!("Unhandled path rejection: {rejection}")),
                };

                Err(error)
            }
        }
    }
}

/// `Query` extractor wrapper that customizes the error from `axum_extra::extract::Query`
pub struct Query<T>(T);

impl<S, T> FromRequestParts<S> for Query<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum_extra::extract::Query::<T>::from_request_parts(parts, state).await {
            Ok(value) => Ok(Self(value.0)),
            Err(rejection) => {
                let error = match rejection {
                    QueryRejection::FailedToDeserializeQueryString(e) => {
                        tracing::error!("Query deserialization error: {e}");
                        AppError::bad_request("Failed to deserialize query string")
                    }
                    _ => {
                        AppError::internal_error(format!("Unhandled query rejection: {rejection}"))
                    }
                };
                Err(error)
            }
        }
    }
}

/// `Json` extractor wrapper that customizes the error from `axum::extract::Json`
pub struct Json<T>(pub T);

impl<S, T> axum::extract::FromRequest<S> for Json<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = axum::Json<AppError>;

    async fn from_request(
        req: axum::http::Request<axum::body::Body>,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(e) => Err(axum::Json(AppError::bad_request(e.to_string()))),
        }
    }
}

impl<T> axum::response::IntoResponse for Json<T>
where
    T: serde::Serialize,
{
    fn into_response(self) -> axum::response::Response {
        axum::Json(self.0).into_response()
    }
}
