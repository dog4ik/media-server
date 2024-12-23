use std::{convert::Infallible, path::PathBuf, str::FromStr};

use anyhow::Context;
use axum::{
    extract::{FromRequest, Multipart, Path, Query, State},
    response::{
        sse::{Event, KeepAlive},
        Sse,
    },
    Json,
};
use reqwest::StatusCode;
use serde::{Deserialize, Deserializer};
use tokio_stream::{Stream, StreamExt};
use torrent::{DownloadParams, MagnetLink, TorrentFile};

use crate::{
    app_state::{AppError, AppState},
    config,
    metadata::{metadata_stack::MetadataProvidersStack, ContentType},
    server::OptionalContentTypeQuery,
    torrent::{
        DownloadContentHint, Priority, ResolveMagnetLinkPayload, TorrentClient,
        TorrentDownloadPayload, TorrentInfo, TorrentState,
    },
};

#[derive(Debug, Clone)]
pub struct InfoHash(pub [u8; 20]);

impl<'de> Deserialize<'de> for InfoHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HexVisitor;

        impl<'de> serde::de::Visitor<'de> for HexVisitor {
            type Value = InfoHash;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a hex string representing 20 bytes")
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                InfoHash::from_str(v).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(HexVisitor)
    }
}

impl InfoHash {
    /// Hex string of info hash
    pub fn hex(&self) -> String {
        self.to_string()
    }
}

impl FromStr for InfoHash {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        anyhow::ensure!(s.len() == 40);
        pub fn decode_hex(s: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
                .collect()
        }
        let bytes = decode_hex(s)?;
        let mut array = [0u8; 20];
        array.copy_from_slice(&bytes);
        Ok(Self(array))
    }
}

impl AsRef<[u8; 20]> for InfoHash {
    fn as_ref(&self) -> &[u8; 20] {
        &self.0
    }
}

impl std::fmt::Display for InfoHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:x}", bytes::Bytes::copy_from_slice(&self.0))
    }
}

#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct PriorityPayload {
    file: usize,
    priority: Priority,
}

#[derive(Debug, utoipa::ToSchema)]
pub struct MultipartTorrent {
    #[schema(value_type = Option<String>)]
    save_location: Option<PathBuf>,
    #[schema(content_media_type = "application/octet-stream", value_type = Vec<u8>)]
    torrent_file: TorrentFile,
}

impl MultipartTorrent {
    pub async fn from_multipart(mut multipart: Multipart) -> anyhow::Result<Self> {
        let mut save_location = None;
        let mut torrent_file = None;
        while let Ok(Some(field)) = multipart.next_field().await {
            if let Some("save_location") = field.name() {
                save_location = field.text().await.ok().map(Into::into);
                continue;
            }
            let data = field.bytes().await?;
            torrent_file = Some(TorrentFile::from_bytes(data)?);
        }
        Ok(Self {
            torrent_file: torrent_file.context("get torrent file")?,
            save_location,
        })
    }
}

#[axum::async_trait]
impl<S> FromRequest<S> for MultipartTorrent
where
    S: Send + Sync,
{
    type Rejection = AppError;

    /// Perform the extraction.
    async fn from_request(req: axum::extract::Request, state: &S) -> Result<Self, Self::Rejection> {
        let multipart = Multipart::from_request(req, state)
            .await
            .map_err(|_| AppError::bad_request("failed to extract multipart from request"))?;
        let res = MultipartTorrent::from_multipart(multipart).await?;
        Ok(res)
    }
}

/// Get list of all torrents
#[utoipa::path(
    get,
    path = "/api/torrent/all",
    responses(
        (status = 200, body = Vec<TorrentState>),
    ),
    tag = "Torrent",
)]
pub async fn all_torrents(State(client): State<&'static TorrentClient>) -> Json<Vec<TorrentState>> {
    Json(client.all_downloads().await)
}

/// Set file priority
#[utoipa::path(
    post,
    path = "/api/torrent/{info_hash}/file_priority",
    params(
        ("info_hash", description = "Hex encoded info_hash of the torrent"),
    ),
    request_body = PriorityPayload,
    responses(
        (status = 200),
    ),
    tag = "Torrent",
)]
pub async fn set_file_priority(
    Path(info_hash): Path<InfoHash>,
    State(client): State<&'static TorrentClient>,
    Json(payload): Json<PriorityPayload>,
) -> Result<(), AppError> {
    let torrent = client
        .get_download(info_hash.as_ref())
        .ok_or(AppError::not_found("Torrent is not found"))?;
    let priority: torrent::Priority = payload.priority.into();
    if payload.file > torrent.torrent_info.contents.files.len() - 1 {
        return Err(AppError::bad_request("File is out of bounds"));
    }
    torrent
        .download_handle
        .set_file_priority(payload.file, priority)
        .await?;
    client
        .update_file_priority(info_hash.as_ref(), payload.file, priority)
        .await?;

    Ok(())
}

/// Open torrent using magnet link
#[utoipa::path(
    post,
    path = "/api/torrent/open",
    request_body = TorrentDownloadPayload,
    responses(
        (status = 201, description = "Torrent is added"),
        (status = 400, description = "Magnet link is incorrect"),
        (status = 500, description = "Failed to add torrent"),
    ),
    tag = "Torrent",
)]
pub async fn open_torrent(
    State(AppState {
        providers_stack,
        torrent_client,
        ..
    }): State<AppState>,
    Json(payload): Json<TorrentDownloadPayload>,
) -> Result<StatusCode, AppError> {
    let magnet_link = MagnetLink::from_str(&payload.magnet_link)
        .map_err(|_| AppError::bad_request("Failed to parse magnet link"))?;
    let tracker_list = magnet_link.all_trackers().ok_or(AppError::bad_request(
        "Magnet links without tracker list are not supported",
    ))?;
    let info = torrent_client.resolve_magnet_link(&magnet_link).await?;
    let mut torrent_info = TorrentInfo::new(&info, payload.content_hint, providers_stack).await;

    let enabled_files = payload
        .enabled_files
        .unwrap_or_else(|| (0..info.files_amount()).collect());
    dbg!(&enabled_files);
    for enabled_idx in &enabled_files {
        torrent_info.contents.files[*enabled_idx].enabled = true;
    }
    let save_location = payload
        .save_location
        .map(PathBuf::from)
        .or_else(|| {
            let content_type = torrent_info
                .contents
                .content
                .as_ref()
                .map(|c| c.content_type())?;
            let folders = match content_type {
                ContentType::Movie => config::CONFIG.get_value::<config::MovieFolders>().0,
                ContentType::Show => config::CONFIG.get_value::<config::ShowFolders>().0,
            };
            folders
                .into_iter()
                .find(|f| f.try_exists().unwrap_or(false))
        })
        .ok_or(AppError::bad_request("Could not determine save location"))?;
    tracing::debug!("Selected torrent output: {}", save_location.display());
    let params = DownloadParams::empty(info, tracker_list, enabled_files, save_location);
    torrent_client.add_torrent(params, torrent_info).await?;

    Ok(StatusCode::CREATED)
}

/// Parse .torrent file
#[utoipa::path(
    post,
    path = "/api/torrent/parse_torrent_file",
    params(
        OptionalContentTypeQuery,
    ),
    request_body(content = inline(MultipartTorrent), content_type = "multipart/form-data"),
    responses(
        (status = 200, body = TorrentInfo),
        (status = 400, description = "Failed to parse torrent file"),
    ),
    tag = "Torrent",
)]
pub async fn parse_torrent_file(
    State(providers_stack): State<&'static MetadataProvidersStack>,
    Query(hint): Query<Option<DownloadContentHint>>,
    MultipartTorrent { torrent_file, .. }: MultipartTorrent,
) -> Result<Json<TorrentInfo>, AppError> {
    let torrent_info = TorrentInfo::new(&torrent_file.info, hint, providers_stack).await;
    return Ok(Json(torrent_info));
}

/// Open .torrent file
#[utoipa::path(
    post,
    path = "/api/torrent/open_torrent_file",
    request_body(content = inline(MultipartTorrent), content_type = "multipart/form-data"),
    responses(
        (status = 200),
        (status = 400, description = "Failed to parse/open torrent file"),
    ),
    tag = "Torrent",
)]
pub async fn open_torrent_file(
    State(app_state): State<AppState>,
    MultipartTorrent {
        save_location,
        torrent_file,
        ..
    }: MultipartTorrent,
) -> Result<(), AppError> {
    let torrent_info = TorrentInfo::new(&torrent_file.info, None, app_state.providers_stack).await;
    let save_location = save_location
        .or_else(|| {
            let content_type = torrent_info
                .contents
                .content
                .as_ref()
                .map(|c| c.content_type())?;
            let folders = match content_type {
                ContentType::Movie => config::CONFIG.get_value::<config::MovieFolders>().0,
                ContentType::Show => config::CONFIG.get_value::<config::ShowFolders>().0,
            };
            folders
                .into_iter()
                .find(|f| f.try_exists().unwrap_or(false))
        })
        .ok_or(AppError::bad_request("Could not determine save location"))?;

    let enabled_files = (0..torrent_file.info.files_amount()).collect();
    let trackers = torrent_file.all_trackers();
    let download_params =
        DownloadParams::empty(torrent_file.info, trackers, enabled_files, save_location);

    app_state
        .torrent_client
        .add_torrent(download_params, torrent_info)
        .await?;
    Ok(())
}

/// Resolve magnet link
#[utoipa::path(
    get,
    path = "/api/torrent/resolve_magnet_link",
    params(
        ResolveMagnetLinkPayload,
        ("content_type" = Option<ContentType>, Query, description = "Content type"),
        ("metadata_provider" = Option<crate::metadata::MetadataProvider>, Query, description = "Metadata provider"),
        ("metadata_id" = Option<String>, Query, description = "Metadata id"),
    ),
    responses(
        (status = 200, body = TorrentInfo),
        (status = 400, description = "Failed to parse magnet link"),
    ),
    tag = "Torrent",
)]
pub async fn resolve_magnet_link(
    State(client): State<&'static TorrentClient>,
    State(providers_stack): State<&'static MetadataProvidersStack>,
    Query(payload): Query<ResolveMagnetLinkPayload>,
    hint: Option<Query<DownloadContentHint>>,
) -> Result<Json<TorrentInfo>, AppError> {
    let magnet_link = MagnetLink::from_str(&payload.magnet_link)
        .map_err(|_| AppError::bad_request("Failed to parse magnet link"))?;
    let info = client.resolve_magnet_link(&magnet_link).await?;
    let torrent_info = TorrentInfo::new(&info, hint.map(|x| x.0), providers_stack).await;
    Ok(Json(torrent_info))
}

/// Get fresh full torrent state
#[utoipa::path(
    get,
    path = "/api/torrent/{info_hash}/state",
    params(
        ("info_hash", description = "Hex encoded info_hash of the torrent"),
    ),
    responses(
        (status = 200, body = TorrentState),
        (status = 404, description = "Torrent not found"),
    ),
    tag = "Torrent",
)]
pub async fn torrent_state(
    State(client): State<&'static TorrentClient>,
    Path(info_hash): Path<InfoHash>,
) -> Result<Json<TorrentState>, AppError> {
    let progress = client
        .full_progress(info_hash.as_ref())
        .await
        .ok_or(AppError::not_found("Torrent is not found"))?;
    Ok(Json(progress))
}

/// SSE stream of torrent updates
#[utoipa::path(
    get,
    path = "/api/torrent/updates",
    responses(
        (status = 200, body = [u8]),
    ),
    tag = "Torrent",
)]
pub async fn updates(
    State(client): State<&'static TorrentClient>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let channel = &client.progress_broadcast.clone();
    let rx = channel.subscribe();

    let stream = tokio_stream::wrappers::BroadcastStream::new(rx).map(|item| {
        if let Ok(item) = item {
            Ok(Event::default().json_data(item).unwrap())
        } else {
            Ok(Event::default())
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// Remove torrent by its info hash
#[utoipa::path(
    delete,
    path = "/api/torrent/{info_hash}",
    params(
        ("info_hash", description = "Hex encoded info_hash of the torrent"),
    ),
    responses(
        (status = 200),
        (status = 404, description = "Torrent is not found"),
    ),
    tag = "Torrent",
)]
pub async fn delete_torrent(
    Path(info_hash): Path<InfoHash>,
    State(client): State<&'static TorrentClient>,
) -> Result<(), AppError> {
    client
        .remove_download(info_hash.0)
        .await
        .ok_or(AppError::not_found("Torrent is not found"))?;
    Ok(())
}
