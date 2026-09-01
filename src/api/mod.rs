use crate::AppError;
use crate::OffsetDateTime as CrateOffsetDateTime;
use crate::app_state;
use crate::config;
use crate::db;
use crate::metadata;
use crate::metadata::MovieMetadataProvider;
use crate::metadata::ShowMetadataProvider;
use crate::torrent_index;
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
use utoipa_axum::router::OpenApiRouter;

/// API data types
///
/// This module defines the data types used by the API, as well as the methods required for their construction.
pub mod api_data;
mod file_browser;
mod history;
mod intros;
/// Liked, watched, custom lists endpoints
mod lists;
/// Resources api endpoints
mod resources;
pub mod server;
mod subtitles;
/// Torrent client specific endpoints
pub mod torrent;

#[derive(OpenApi)]
#[openapi(
    components(
        schemas(
            metadata::ParentMediaType,
            metadata::LeafMediaType,
            metadata::MediaType,
            crate::api::torrent::DownloadContentHint,
            config::UtoipaConfigSchema,
            ws::WsRequest,
            ws::WsMessage
        )
    ),
    tags(
        (name = "Configuration", description = "Server configuration options"),
        (name = "Shows", description = "Shows, seasons, episodes operations"),
        (name = "Movies", description = "Movies operations"),
        (name = "Metadata", description = "Metadata operations"),
        (name = "History", description = "History operations"),
        (name = "Tasks", description = "Tasks operations"),
        (name = "Search", description = "Endopoints for searching content"),
        (name = "Torrent", description = "Torrent client operations"),
        (name = "Watch", description = "Content watching operations"),
        (name = "Videos", description = "Video files operations"),
        (name = "Subtitles", description = "Subtitles operations"),
        (name = "Actors", description = "Actors operations"),
        (name = "Resources", description = "Server resources monitoring"),
    )
)]
pub struct OpenApiDoc;

pub fn router() -> OpenApiRouter<app_state::AppState> {
    OpenApiRouter::with_openapi(OpenApiDoc::openapi())
        .merge(server::router())
        .merge(file_browser::router())
        .merge(history::router())
        .merge(intros::router())
        .merge(lists::router())
        .merge(resources::router())
        .merge(subtitles::router())
        .merge(torrent::router())
        .merge(ws::router())
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ContentFilterQuery {
    #[serde(default)]
    pub actors: Vec<i64>,
    pub search: Option<String>,
    pub take: Option<i64>,
    pub cursor: Option<String>,
    /// Only include content with at least one local video file. Defaults to `true`.
    pub only_local: Option<bool>,
}

impl From<ContentFilterQuery> for db::ContentFetchParams {
    fn from(filter: ContentFilterQuery) -> Self {
        db::ContentFetchParams {
            take: filter.take,
            cursor: filter.cursor,
            search: filter.search,
            actors: (!filter.actors.is_empty()).then_some(filter.actors),
            only_local: filter.only_local.unwrap_or(true),
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
    pub content_type: metadata::ParentMediaType,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct OptionalContentTypeQuery {
    #[param(inline)]
    pub content_type: Option<metadata::ParentMediaType>,
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
pub struct NumberQuery {
    pub number: usize,
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct TakeQuery {
    pub take: Option<i64>,
}

pub struct DynShowProviderQuery(pub &'static (dyn ShowMetadataProvider + Send + Sync + 'static));

impl FromRequestParts<crate::AppState> for DynShowProviderQuery {
    type Rejection = crate::AppError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &crate::AppState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let Query(ProviderQuery { provider }) =
                Query::<ProviderQuery>::from_request_parts(parts, state).await?;
            match state.providers_stack.show_provider(provider) {
                Some(provider) => Ok(Self(provider)),
                None => Err(AppError::not_found("requested metadata provider not found")),
            }
        }
    }
}

pub struct DynMovieProviderQuery(pub &'static (dyn MovieMetadataProvider + Send + Sync + 'static));

impl FromRequestParts<crate::AppState> for DynMovieProviderQuery {
    type Rejection = crate::AppError;

    fn from_request_parts(
        parts: &mut Parts,
        state: &crate::AppState,
    ) -> impl Future<Output = Result<Self, Self::Rejection>> + Send {
        async move {
            let Query(ProviderQuery { provider }) =
                Query::<ProviderQuery>::from_request_parts(parts, state).await?;
            match state.providers_stack.movie_provider(provider) {
                Some(provider) => Ok(Self(provider)),
                None => Err(AppError::not_found("requested metadata provider not found")),
            }
        }
    }
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
    type Rejection = AppError;

    async fn from_request(
        req: axum::http::Request<axum::body::Body>,
        state: &S,
    ) -> std::result::Result<Self, Self::Rejection> {
        match axum::Json::<T>::from_request(req, state).await {
            Ok(axum::Json(value)) => Ok(Self(value)),
            Err(axum::extract::rejection::JsonRejection::JsonDataError(e)) => {
                Err(AppError::unprocessable(e.to_string()))
            }
            Err(e) => Err(AppError::bad_request(e.to_string())),
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
