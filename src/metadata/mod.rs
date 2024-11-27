use std::{fmt::Display, num::NonZero, str::FromStr, time::Duration};

use crate::{
    app_state::AppError,
    db::{DbEpisode, DbMovie, DbSeason, DbShow},
    ffmpeg,
};
use reqwest::Url;
use serde::{
    de::{self},
    Deserialize, Deserializer, Serialize,
};

pub mod library_scan;
pub mod local_provider;
pub mod metadata_stack;
pub mod request_client;
pub mod tmdb_api;
#[allow(unused)]
pub mod tvdb_api;

pub const METADATA_CACHE_SIZE: NonZero<usize> = NonZero::new(20).unwrap();

#[derive(Debug, Clone, Copy, Default, utoipa::ToSchema, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Es,
    De,
    Fr,
    Ru,
    Ja,
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

        impl<'de> de::Visitor<'de> for MetadataImageVisitor {
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

#[axum::async_trait]
pub trait MovieMetadataProvider {
    /// Query for movie
    #[allow(async_fn_in_trait)]
    async fn movie(
        &self,
        movie_metadata_id: &str,
        params: FetchParams,
    ) -> Result<MovieMetadata, AppError>;

    /// Provider identifier
    fn provider_identifier(&self) -> &'static str;
}

#[axum::async_trait]
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
    fn provider_identifier(&self) -> &'static str;
}

#[axum::async_trait]
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
    fn provider_identifier(&self) -> &'static str;
}

// types

#[derive(Debug, Serialize, Deserialize, Clone, Copy, Default, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum MetadataProvider {
    #[default]
    Local,
    Tmdb,
    Tvdb,
    Imdb,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct MovieMetadata {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub poster: Option<MetadataImage>,
    pub backdrop: Option<MetadataImage>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    #[schema(value_type = Option<crate::server::SerdeDuration>)]
    pub runtime: Option<Duration>,
    pub title: String,
}

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
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct SeasonMetadata {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub release_date: Option<String>,
    pub episodes: Vec<EpisodeMetadata>,
    pub plot: Option<String>,
    pub poster: Option<MetadataImage>,
    pub number: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct EpisodeMetadata {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub release_date: Option<String>,
    pub number: usize,
    pub title: String,
    pub plot: Option<String>,
    pub season_number: usize,
    #[schema(value_type = Option<crate::server::SerdeDuration>)]
    pub runtime: Option<Duration>,
    pub poster: Option<MetadataImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterMetadata {
    pub actor: String,
    pub character: String,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
        }
    }
}

impl EpisodeMetadata {
    pub fn into_db_episode(self, season_id: i64, duration: Duration) -> DbEpisode {
        DbEpisode {
            id: None,
            season_id,
            title: self.title,
            number: self.number as i64,
            plot: self.plot,
            release_date: self.release_date,
            duration: duration.as_secs() as i64,
            poster: self.poster.map(|x| x.as_str().to_owned()),
        }
    }
}

impl SeasonMetadata {
    pub fn into_db_season(self, show_id: i64) -> DbSeason {
        let poster;
        if let Some(metadata_image) = self.poster {
            poster = Some(metadata_image.as_str().to_owned());
        } else {
            poster = None;
        }
        DbSeason {
            id: None,
            show_id,
            number: self.number as i64,
            release_date: self.release_date,
            plot: self.plot,
            poster,
        }
    }
}

impl ShowMetadata {
    pub fn into_db_show(self) -> DbShow {
        let poster;
        if let Some(metadata_image) = self.poster {
            poster = Some(metadata_image.as_str().to_owned());
        } else {
            poster = None;
        };
        let backdrop = self.backdrop.map(|p| p.as_str().to_owned());

        DbShow {
            id: None,
            title: self.title,
            release_date: self.release_date,
            poster,
            backdrop,
            plot: self.plot,
        }
    }
}

impl MovieMetadata {
    pub fn into_db_movie(self, duration: Duration) -> DbMovie {
        let poster;
        if let Some(metadata_image) = self.poster {
            poster = Some(metadata_image.as_str().to_owned());
        } else {
            poster = None;
        };
        let backdrop = self.backdrop.map(|p| p.as_str().to_owned());

        DbMovie {
            id: None,
            title: self.title,
            release_date: self.release_date,
            poster,
            backdrop,
            duration: duration.as_secs() as i64,
            plot: self.plot,
        }
    }
}
