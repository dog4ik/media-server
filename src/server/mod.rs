use std::path::Path;

use crate::app_state;
use crate::config;
use crate::db;
use crate::ffmpeg;
use crate::library;
use crate::metadata;
use crate::progress;
use crate::torrent_index;
use axum::response::IntoResponse;
use axum_extra::{headers::Range, TypedHeader};
use serde::Deserialize;
use utoipa::OpenApi;

pub mod admin_api;
pub mod content;
pub mod public_api;

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
        public_api::pull_video_subtitle,
        public_api::get_movie,
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
        public_api::episode_poster,
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
        admin_api::remove_variant,
        public_api::all_history,
        admin_api::clear_history,
        public_api::video_history,
        admin_api::remove_history_item,
        admin_api::update_history,
        public_api::search_torrent,
        admin_api::download_torrent,
        public_api::search_content,
        admin_api::server_configuration,
        admin_api::update_server_configuration,
        admin_api::server_configuration_schema,
        admin_api::reset_server_configuration,
        admin_api::order_providers,
        admin_api::providers_order,
        admin_api::latest_log,
        admin_api::get_tasks,
        admin_api::cancel_task,
        admin_api::progress,
        admin_api::mock_progress,
        admin_api::cancel_task,
        admin_api::reconciliate_lib,
        admin_api::clear_db,
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
            app_state::AppError,
            app_state::AppErrorKind,
            public_api::DetailedVideo,
            public_api::DetailedVideoTrack,
            public_api::DetailedAudioTrack,
            public_api::DetailedSubtitleTrack,
            public_api::DetailedVariant,
            public_api::VariantSummary,
            admin_api::ProviderOrder,
            admin_api::UpdateHistoryPayload,
            admin_api::TorrentDownloadHint,
            admin_api::TorrentDownloadPayload,
            admin_api::ProviderType,
            progress::Task,
            progress::TaskKind,
            torrent_index::Torrent,
            db::DbHistory,
            db::DbExternalId,
            library::TranscodePayload,
            library::AudioCodec,
            library::VideoCodec,
            library::SubtitlesCodec,
            library::Resolution,
            config::ServerConfiguration,
            config::FileConfigSchema,
            config::AppResources,
            config::Capabilities,
            config::Codec,
            config::CodecType,
            ffmpeg::H264Preset,
            SerdeDuration
        )
    ),
    tags(
        (name = "public_api", description = "Show API")
    )
)]
pub struct OpenApiDoc;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct PageQuery {
    pub page: Option<usize>,
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

async fn serve_video(
    path: impl AsRef<Path>,
    range: Option<TypedHeader<Range>>,
) -> impl IntoResponse {
    use std::io::SeekFrom;

    use axum::{body::Body, http::HeaderMap};
    use reqwest::{header, StatusCode};
    use tokio::fs;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    use tokio_util::codec::{BytesCodec, FramedRead};

    let mut file = fs::File::open(path).await.unwrap();
    let metadata = file.metadata().await.unwrap();
    let file_size = metadata.len();
    let range = range.map(|h| h.0).unwrap_or(Range::bytes(0..).unwrap());
    let (start, end) = range
        .satisfiable_ranges(file_size)
        .next()
        .expect("at least one tuple");
    let start = match start {
        std::ops::Bound::Included(val) => val,
        std::ops::Bound::Excluded(val) => val,
        std::ops::Bound::Unbounded => 0,
    };

    let end = match end {
        std::ops::Bound::Included(val) => val,
        std::ops::Bound::Excluded(val) => val,
        std::ops::Bound::Unbounded => file_size,
    };

    let chunk_size = end - start + 1;
    file.seek(SeekFrom::Start(start)).await.unwrap();
    let stream_of_bytes = FramedRead::new(file.take(chunk_size), BytesCodec::new());
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from(end - start),
    );
    // headers.insert(
    //     header::CONTENT_TYPE,
    //     header::HeaderValue::from_static(self.guess_mime_type()),
    // );
    headers.insert(
        header::ACCEPT_RANGES,
        header::HeaderValue::from_static("bytes"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("public, max-age=0"),
    );
    headers.insert(
        header::CONTENT_RANGE,
        header::HeaderValue::from_str(&format!("bytes {}-{}/{}", start, end - 1, file_size))
            .unwrap(),
    );

    return (
        StatusCode::PARTIAL_CONTENT,
        headers,
        Body::from_stream(stream_of_bytes),
    );
}
