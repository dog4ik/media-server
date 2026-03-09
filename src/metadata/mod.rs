use std::{fmt::Display, num::NonZero, str::FromStr, time::Duration};

use crate::{
    app_state::AppError,
    db::{DbContent, DbContentType, DbEpisode, DbMovie, DbSeason, DbShow},
    ffmpeg,
};
use reqwest::Url;
use serde::{
    Deserialize, Deserializer, Serialize,
    de::{self},
};

pub mod metadata_stack;
/// Fallback service for different metadata providers.
///
/// Allows to access metadata providers and torrent indexes with using user authorization.
/// Some providers are available only with this agent.
///
/// ### Performance
/// Since all user requests go to this service it will share provider limitation (e.g. rate limit)
/// between all users.
pub mod provod_agent;
/// Rate limited request client
pub mod request_client;
/// Tmdb API agent
pub mod tmdb_api;
/// Tvdb API agent
#[allow(unused)]
pub mod tvdb_api;

pub const METADATA_CACHE_SIZE: NonZero<usize> = NonZero::new(20).unwrap();

#[derive(Debug, Clone, Copy, Default, utoipa::ToSchema, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    /// Spanish
    Es,
    /// German
    De,
    /// French
    Fr,
    /// Russian
    Ru,
    /// Japanese
    Ja,
    /// Serbian
    Sr,
}

impl Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl Language {
    fn as_str(&self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Es => "es",
            Language::De => "de",
            Language::Fr => "fr",
            Language::Ru => "ru",
            Language::Ja => "ja",
            Language::Sr => "sr",
        }
    }
}

impl FromStr for Language {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "en" => Ok(Language::En),
            "es" => Ok(Language::Es),
            "de" => Ok(Language::De),
            "fr" => Ok(Language::Fr),
            "ru" => Ok(Language::Ru),
            "ja" => Ok(Language::Ja),
            "sr" => Ok(Language::Sr),
            _ => Err(anyhow::anyhow!("Unsupported language: {s}")),
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct FetchParams {
    pub lang: Language,
}

#[derive(Debug, Clone, utoipa::ToSchema)]
pub struct MetadataImage(pub Url);

impl AsRef<Url> for MetadataImage {
    fn as_ref(&self) -> &Url {
        &self.0
    }
}

impl From<MetadataImage> for Url {
    fn from(val: MetadataImage) -> Self {
        val.0
    }
}

impl Serialize for MetadataImage {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl<'de> Deserialize<'de> for MetadataImage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MetadataImageVisitor;

        impl de::Visitor<'_> for MetadataImageVisitor {
            type Value = MetadataImage;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a string representing a valid URL")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                match Url::from_str(value) {
                    Ok(url) => Ok(MetadataImage(url)),
                    Err(_) => Err(de::Error::invalid_value(de::Unexpected::Str(value), &self)),
                }
            }
        }

        deserializer.deserialize_str(MetadataImageVisitor)
    }
}

impl MetadataImage {
    pub fn new(url: Url) -> Self {
        MetadataImage(url)
    }
    const BLUR_DATA_IMG_WIDTH: i32 = 30;

    pub async fn generate_blur_data(&self) -> Result<String, anyhow::Error> {
        tracing::trace!("Generating blur data for: {}", self.0);
        let MetadataImage(url) = self;
        let bytes = reqwest::get(url.clone()).await?.bytes().await?;
        ffmpeg::resize_image_ffmpeg(bytes, Self::BLUR_DATA_IMG_WIDTH, None).await
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl Display for MetadataImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// This trait must be implemented by all movie metadata providers
#[async_trait::async_trait]
pub trait MovieMetadataProvider {
    /// Query for movie
    #[allow(async_fn_in_trait)]
    async fn movie(
        &self,
        movie_metadata_id: &str,
        params: FetchParams,
    ) -> Result<MovieMetadata, AppError>;

    /// Provider identifier
    fn provider_identifier(&self) -> MetadataProvider;
}

/// This trait must be implemented by all show metadata providers
#[async_trait::async_trait]
pub trait ShowMetadataProvider {
    /// Query for show
    #[allow(async_fn_in_trait)]
    async fn show(
        &self,
        show_id: &str,
        fetch_params: FetchParams,
    ) -> Result<ShowMetadata, AppError>;

    /// Query for season
    #[allow(async_fn_in_trait)]
    async fn season(
        &self,
        show_id: &str,
        season: usize,
        fetch_params: FetchParams,
    ) -> Result<SeasonMetadata, AppError>;

    /// Query for episode
    #[allow(async_fn_in_trait)]
    async fn episode(
        &self,
        show_id: &str,
        season: usize,
        episode: usize,
        fetch_params: FetchParams,
    ) -> Result<EpisodeMetadata, AppError>;

    /// Provider identifier
    fn provider_identifier(&self) -> MetadataProvider;
}

/// This trait must be implemented by all metadata providers with discovery capabilities
#[async_trait::async_trait]
pub trait DiscoverMetadataProvider {
    /// Multi search
    async fn multi_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<MetadataSearchResult>, AppError>;

    /// Show search
    async fn show_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<ShowMetadata>, AppError>;

    /// Movie search
    async fn movie_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<MovieMetadata>, AppError>;

    /// External ids without self
    async fn external_ids(
        &self,
        content_id: &str,
        content_hint: ContentType,
    ) -> Result<Vec<ExternalIdMetadata>, AppError>;

    /// Provider identifier
    fn provider_identifier(&self) -> MetadataProvider;
}

// types

#[derive(
    Debug,
    Serialize,
    Deserialize,
    Clone,
    Copy,
    Hash,
    Default,
    PartialEq,
    Eq,
    utoipa::ToSchema,
    sqlx::Type,
)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub enum MetadataProvider {
    #[default]
    Local,
    Tmdb,
    Tvdb,
    Imdb,
}

impl MetadataProvider {
    pub fn is_local(&self) -> bool {
        *self == Self::Local
    }
}

impl From<String> for MetadataProvider {
    fn from(value: String) -> Self {
        Self::from_str(&value).expect("direct from conversion not fail")
    }
}

impl FromStr for MetadataProvider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> std::prelude::v1::Result<Self, Self::Err> {
        match s {
            "local" => Ok(Self::Local),
            "tmdb" => Ok(Self::Tmdb),
            "tvdb" => Ok(Self::Tvdb),
            "imdb" => Ok(Self::Imdb),
            _ => Err(anyhow::anyhow!(
                "{s} is not recognized as metadata provider"
            )),
        }
    }
}

impl Display for MetadataProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetadataProvider::Local => write!(f, "local"),
            MetadataProvider::Tmdb => write!(f, "tmdb"),
            MetadataProvider::Tvdb => write!(f, "tvdb"),
            MetadataProvider::Imdb => write!(f, "imdb"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Movie,
    Show,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MetadataSearchResult {
    pub title: String,
    pub poster: Option<MetadataImage>,
    pub plot: Option<String>,
    pub metadata_provider: MetadataProvider,
    pub content_type: ContentType,
    pub metadata_id: String,
    pub locale_metadata: Option<LocaleMetadata>,
}

/// The unified movie data structure from any movie provider
#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct MovieMetadata {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub poster: Option<MetadataImage>,
    pub backdrop: Option<MetadataImage>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    #[schema(value_type = Option<crate::api::SerdeDuration>)]
    pub runtime: Option<Duration>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
}

/// The unified show data structure from any show provider
#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToResponse, utoipa::ToSchema)]
pub struct ShowMetadata {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub poster: Option<MetadataImage>,
    pub backdrop: Option<MetadataImage>,
    pub plot: Option<String>,
    /// Array of available season numbers
    pub seasons: Option<Vec<usize>>,
    pub episodes_amount: Option<usize>,
    pub release_date: Option<String>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
}

/// The unified season data structure from any show provider
#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct SeasonMetadata {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub release_date: Option<String>,
    pub title: Option<String>,
    pub episodes: Vec<EpisodeMetadata>,
    pub plot: Option<String>,
    pub poster: Option<MetadataImage>,
    pub number: usize,
}

/// The unified episode data structure from any show provider
#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct EpisodeMetadata {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub release_date: Option<String>,
    pub number: usize,
    pub title: String,
    pub plot: Option<String>,
    pub season_number: usize,
    #[schema(value_type = Option<crate::api::SerdeDuration>)]
    pub runtime: Option<Duration>,
    pub poster: Option<MetadataImage>,
}

/// Localization specific data
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LocaleMetadata {
    pub original_title: String,
    pub original_language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterMetadata {
    pub actor: String,
    pub character: String,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExternalIdMetadata {
    pub provider: MetadataProvider,
    pub id: String,
}

impl From<MovieMetadata> for MetadataSearchResult {
    fn from(val: MovieMetadata) -> Self {
        MetadataSearchResult {
            title: val.title,
            poster: val.poster,
            plot: val.plot,
            metadata_provider: val.metadata_provider,
            content_type: ContentType::Movie,
            metadata_id: val.metadata_id,
            locale_metadata: val.locale_metadata,
        }
    }
}

impl From<ShowMetadata> for MetadataSearchResult {
    fn from(val: ShowMetadata) -> Self {
        MetadataSearchResult {
            title: val.title,
            poster: val.poster,
            plot: val.plot,
            metadata_provider: val.metadata_provider,
            content_type: ContentType::Show,
            metadata_id: val.metadata_id,
            locale_metadata: val.locale_metadata,
        }
    }
}

impl ShowMetadata {
    pub fn into_db_content(&self) -> DbContent {
        let poster;
        if let Some(metadata_image) = &self.poster {
            poster = Some(metadata_image.as_str().to_owned());
        } else {
            poster = None;
        };
        let (original_language, original_title) = match self.locale_metadata.clone() {
            Some(m) => (Some(m.original_language), Some(m.original_title)),
            None => (None, None),
        };

        DbContent {
            id: None,
            content_type: DbContentType::Show,
            title: self.title.clone(),
            release_date: self.release_date.clone(),
            poster,
            plot: self.plot.clone(),
            original_language,
            original_title,
        }
    }

    pub fn into_db_show(self, content_id: i64) -> DbShow {
        let backdrop = self.backdrop.map(|p| p.as_str().to_owned());

        DbShow {
            id: None,
            content_id,
            backdrop,
        }
    }
}

impl EpisodeMetadata {
    pub fn into_db_content(&self) -> DbContent {
        DbContent {
            id: None,
            content_type: DbContentType::Episode,
            original_title: None,
            original_language: None,
            title: self.title.clone(),
            plot: self.plot.clone(),
            release_date: self.release_date.clone(),
            poster: self.poster.as_ref().map(|x| x.as_str().to_owned()),
        }
    }

    pub fn into_db_episode(self, content_id: i64, season_id: i64, duration: Duration) -> DbEpisode {
        DbEpisode {
            id: None,
            content_id,
            season_id,
            number: self.number as i64,
            duration: duration.as_secs() as i64,
        }
    }
}

impl SeasonMetadata {
    pub fn into_db_content(&self) -> DbContent {
        let poster;
        if let Some(metadata_image) = &self.poster {
            poster = Some(metadata_image.as_str().to_owned());
        } else {
            poster = None;
        }

        DbContent {
            id: None,
            content_type: DbContentType::Season,
            original_title: None,
            original_language: None,
            release_date: self.release_date.clone(),
            plot: self.plot.clone(),
            poster,
            title: self
                .title
                .clone()
                .unwrap_or_else(|| format!("Season {}", self.number)),
        }
    }

    pub fn into_db_season(self, content_id: i64, show_id: i64) -> DbSeason {
        DbSeason {
            id: None,
            content_id,
            show_id,
            number: self.number as i64,
        }
    }
}

impl MovieMetadata {
    pub fn into_db_content(&self) -> DbContent {
        let poster;
        if let Some(metadata_image) = &self.poster {
            poster = Some(metadata_image.as_str().to_owned());
        } else {
            poster = None;
        };

        let (original_language, original_title) = match self.locale_metadata.clone() {
            Some(m) => (Some(m.original_language), Some(m.original_title)),
            None => (None, None),
        };
        DbContent {
            id: None,
            content_type: DbContentType::Movie,
            title: self.title.clone(),
            release_date: self.release_date.clone(),
            poster,
            plot: self.plot.clone(),
            original_language,
            original_title,
        }
    }
    pub fn into_db_movie(self, content_id: i64, duration: Duration) -> DbMovie {
        let backdrop = self.backdrop.as_ref().map(|p| p.as_str().to_owned());
        DbMovie {
            id: None,
            content_id,
            backdrop,
            duration: duration.as_secs() as i64,
        }
    }
}
