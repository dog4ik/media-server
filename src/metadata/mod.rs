use std::{
    collections::HashMap,
    fmt::Display,
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    app_state::AppError,
    db::{DbEpisode, DbMovie, DbSeason, DbShow},
    ffmpeg,
    torrent_index::{Torrent, TorrentIndex},
};
use anyhow::{anyhow, Context};
use reqwest::{Client, Request, Response, Url};
use serde::{
    de::{self, DeserializeOwned},
    ser::SerializeStruct,
    Deserialize, Deserializer, Serialize,
};
use tokio::sync::{mpsc, oneshot, Semaphore};

pub mod tmdb_api;
#[allow(dead_code)]
pub mod tvdb_api;

pub struct MetadataProvidersStack {
    pub discover_providers_stack: Mutex<Vec<&'static (dyn DiscoverMetadataProvider + Send + Sync)>>,
    pub movie_providers_stack: Mutex<Vec<&'static (dyn MovieMetadataProvider + Send + Sync)>>,
    pub show_providers_stack: Mutex<Vec<&'static (dyn ShowMetadataProvider + Send + Sync)>>,
    pub torrent_indexes_stack: Mutex<Vec<&'static (dyn TorrentIndex + Send + Sync)>>,
}

impl Serialize for MetadataProvidersStack {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut providers = serializer.serialize_struct("MetadataProvidersStack", 4)?;
        let discover_providers: Vec<_> = self
            .discover_providers()
            .into_iter()
            .map(|v| v.provider_identifier().to_string())
            .collect();
        providers.serialize_field("discover_providers", &discover_providers)?;
        let movie_providers: Vec<_> = self
            .movie_providers()
            .into_iter()
            .map(|v| v.provider_identifier().to_string())
            .collect();
        providers.serialize_field("movie_providers", &movie_providers)?;
        let show_providers: Vec<_> = self
            .show_providers()
            .into_iter()
            .map(|v| v.provider_identifier().to_string())
            .collect();
        providers.serialize_field("show_providers", &show_providers)?;
        let torrent_providers: Vec<_> = self
            .torrent_indexes()
            .into_iter()
            .map(|v| v.provider_identifier().to_string())
            .collect();
        providers.serialize_field("torrent_providers", &torrent_providers)?;
        providers.end()
    }
}

impl std::fmt::Debug for MetadataProvidersStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let serialized = serde_json::to_string(self).map_err(|_| std::fmt::Error)?;
        write!(f, "{}", serialized)
    }
}

impl MetadataProvidersStack {
    pub async fn search_movie(&self, query: &str) -> anyhow::Result<Vec<MovieMetadata>> {
        let discover_providers = { self.discover_providers_stack.lock().unwrap().clone() };
        let mut out = Vec::new();
        let handles: Vec<_> = discover_providers
            .into_iter()
            .map(|p| {
                let query = query.to_string();
                tokio::spawn(async move { p.movie_search(&query).await })
            })
            .collect();

        for handle in handles {
            if let Ok(Ok(res)) = handle.await {
                out.extend(res);
            }
        }
        Ok(out)
    }

    pub async fn search_show(&self, query: &str) -> anyhow::Result<Vec<ShowMetadata>> {
        let discover_providers = { self.discover_providers_stack.lock().unwrap().clone() };
        let mut out = Vec::new();
        let handles: Vec<_> = discover_providers
            .into_iter()
            .map(|p| {
                let query = query.to_string();
                tokio::spawn(async move { p.show_search(&query).await })
            })
            .collect();

        for handle in handles {
            if let Ok(Ok(res)) = handle.await {
                out.extend(res);
            }
        }
        Ok(out)
    }

    pub async fn multi_search(&self, query: &str) -> anyhow::Result<Vec<MetadataSearchResult>> {
        let discover_providers = { self.discover_providers_stack.lock().unwrap().clone() };
        let mut out = Vec::with_capacity(discover_providers.len());
        let handles: Vec<_> = discover_providers
            .into_iter()
            .map(|p| {
                let query = query.to_string();
                tokio::spawn(async move { p.multi_search(&query).await })
            })
            .collect();

        for handle in handles {
            if let Ok(Ok(res)) = handle.await {
                out.extend(res);
            }
        }
        Ok(out)
    }

    pub async fn get_movie(
        &self,
        movie_id: &str,
        provider: MetadataProvider,
    ) -> Result<MovieMetadata, AppError> {
        let movie_providers = { self.movie_providers_stack.lock().unwrap().clone() };
        let provider = movie_providers
            .into_iter()
            .find(|p| p.provider_identifier() == provider.to_string())
            .ok_or(anyhow!("provider is not supported"))?;
        provider.movie(movie_id).await
    }

    pub async fn get_show(
        &self,
        show_id: &str,
        provider: MetadataProvider,
    ) -> Result<ShowMetadata, AppError> {
        let show_providers = { self.show_providers_stack.lock().unwrap().clone() };
        let provider = show_providers
            .into_iter()
            .find(|p| p.provider_identifier() == provider.to_string())
            .ok_or(anyhow!("provider is not supported"))?;
        provider.show(show_id).await
    }

    pub async fn get_season(
        &self,
        show_id: &str,
        season: usize,
        provider: MetadataProvider,
    ) -> Result<SeasonMetadata, AppError> {
        let show_providers = { self.show_providers_stack.lock().unwrap().clone() };
        let provider = show_providers
            .into_iter()
            .find(|p| p.provider_identifier() == provider.to_string())
            .ok_or(anyhow!("provider is not supported"))?;
        provider.season(show_id, season).await
    }

    pub async fn get_episode(
        &self,
        show_id: &str,
        season: usize,
        episode: usize,
        provider: MetadataProvider,
    ) -> Result<EpisodeMetadata, AppError> {
        let show_providers = { self.show_providers_stack.lock().unwrap().clone() };
        let provider = show_providers
            .into_iter()
            .find(|p| p.provider_identifier() == provider.to_string())
            .ok_or(anyhow!("provider is not supported"))?;
        provider.episode(show_id, season, episode).await
    }

    pub async fn get_external_ids(
        &self,
        id: &str,
        content_type: ContentType,
        provider: MetadataProvider,
    ) -> Result<Vec<ExternalIdMetadata>, AppError> {
        let discover_providers = { self.discover_providers_stack.lock().unwrap().clone() };
        let provider = discover_providers
            .into_iter()
            .find(|p| p.provider_identifier() == provider.to_string())
            .ok_or(anyhow!("provider is not supported"))?;
        provider.external_ids(id, content_type).await
    }

    pub async fn get_torrents(&self, query: &str) -> Vec<Torrent> {
        let torrent_indexes = { self.torrent_indexes_stack.lock().unwrap().clone() };
        let mut out = Vec::new();
        let handles: Vec<_> = torrent_indexes
            .into_iter()
            .map(|p| {
                let query = query.to_owned();
                tokio::spawn(async move {
                    tokio::time::timeout(Duration::from_secs(5), p.search_torrent(&query)).await
                })
            })
            .collect();

        for handle in handles {
            match handle.await {
                Ok(Ok(Ok(res))) => {
                    out.extend(res);
                }
                Ok(Ok(Err(e))) => {
                    tracing::warn!("Torrent index returned an error: {e}");
                }
                Ok(Err(_)) => {
                    tracing::warn!("Torrent index timed out");
                }
                Err(e) => {
                    tracing::error!("Torrent index task paniced: {e}");
                }
            };
        }
        out
    }

    // Can do something smarter here if extract provider_identifer() in its own trait
    pub fn order_discover_providers(
        &self,
        new_order: Vec<String>,
    ) -> Vec<&'static (dyn DiscoverMetadataProvider + Send + Sync)> {
        let providers: HashMap<&str, &(dyn DiscoverMetadataProvider + Send + Sync)> = self
            .discover_providers()
            .into_iter()
            .map(|p| (p.provider_identifier(), p))
            .collect();
        let mut out = Vec::with_capacity(new_order.len());
        for identifier in new_order {
            if let Some(provider) = providers.get(identifier.as_str()) {
                out.push(*provider);
            }
        }
        *self.discover_providers_stack.lock().unwrap() = out.clone();
        out
    }

    pub fn order_movie_providers(
        &self,
        new_order: Vec<String>,
    ) -> Vec<&'static (dyn MovieMetadataProvider + Send + Sync)> {
        let providers: HashMap<&str, &(dyn MovieMetadataProvider + Send + Sync)> = self
            .movie_providers()
            .into_iter()
            .map(|p| (p.provider_identifier(), p))
            .collect();
        let mut out = Vec::with_capacity(new_order.len());
        for identifier in new_order {
            if let Some(provider) = providers.get(identifier.as_str()) {
                out.push(*provider);
            }
        }
        *self.movie_providers_stack.lock().unwrap() = out.clone();
        out
    }

    pub fn order_show_providers(
        &self,
        new_order: Vec<String>,
    ) -> Vec<&'static (dyn ShowMetadataProvider + Send + Sync)> {
        let providers: HashMap<&str, &(dyn ShowMetadataProvider + Send + Sync)> = self
            .show_providers()
            .into_iter()
            .map(|p| (p.provider_identifier(), p))
            .collect();
        let mut out = Vec::with_capacity(new_order.len());
        for identifier in new_order {
            if let Some(provider) = providers.get(identifier.as_str()) {
                out.push(*provider);
            }
        }
        *self.show_providers_stack.lock().unwrap() = out.clone();
        out
    }

    pub fn order_torrent_indexes(
        &self,
        new_order: Vec<String>,
    ) -> Vec<&'static (dyn TorrentIndex + Send + Sync)> {
        let providers: HashMap<&str, &(dyn TorrentIndex + Send + Sync)> = self
            .torrent_indexes()
            .into_iter()
            .map(|p| (p.provider_identifier(), p))
            .collect();
        let mut out = Vec::with_capacity(new_order.len());
        for identifier in new_order {
            if let Some(provider) = providers.get(identifier.as_str()) {
                out.push(*provider);
            }
        }
        *self.torrent_indexes_stack.lock().unwrap() = out.clone();
        out
    }

    pub fn discover_providers(&self) -> Vec<&'static (dyn DiscoverMetadataProvider + Send + Sync)> {
        self.discover_providers_stack.lock().unwrap().clone()
    }

    pub fn movie_providers(&self) -> Vec<&'static (dyn MovieMetadataProvider + Send + Sync)> {
        self.movie_providers_stack.lock().unwrap().clone()
    }

    pub fn show_providers(&self) -> Vec<&'static (dyn ShowMetadataProvider + Send + Sync)> {
        self.show_providers_stack.lock().unwrap().clone()
    }

    pub fn torrent_indexes(&self) -> Vec<&'static (dyn TorrentIndex + Send + Sync)> {
        self.torrent_indexes_stack.lock().unwrap().clone()
    }
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
        write!(f, "{}", self.0.to_string())
    }
}

#[axum::async_trait]
pub trait MovieMetadataProvider {
    /// Query for movie
    #[allow(async_fn_in_trait)]
    async fn movie(&self, movie_metadata_id: &str) -> Result<MovieMetadata, AppError>;

    /// Provider identifier
    fn provider_identifier(&self) -> &'static str;
}

#[axum::async_trait]
pub trait ShowMetadataProvider {
    /// Query for show
    #[allow(async_fn_in_trait)]
    async fn show(&self, show_id: &str) -> Result<ShowMetadata, AppError>;

    /// Query for season
    #[allow(async_fn_in_trait)]
    async fn season(&self, show_id: &str, season: usize) -> Result<SeasonMetadata, AppError>;

    /// Query for episode
    #[allow(async_fn_in_trait)]
    async fn episode(
        &self,
        show_id: &str,
        season: usize,
        episode: usize,
    ) -> Result<EpisodeMetadata, AppError>;

    /// Provider identifier
    fn provider_identifier(&self) -> &'static str;
}

#[axum::async_trait]
pub trait DiscoverMetadataProvider {
    /// Multi search
    async fn multi_search(&self, query: &str) -> Result<Vec<MetadataSearchResult>, AppError>;

    /// Show search
    async fn show_search(&self, query: &str) -> Result<Vec<ShowMetadata>, AppError>;

    /// Movie search
    async fn movie_search(&self, query: &str) -> Result<Vec<MovieMetadata>, AppError>;

    /// External ids without self
    async fn external_ids(
        &self,
        content_id: &str,
        content_hint: ContentType,
    ) -> Result<Vec<ExternalIdMetadata>, AppError>;

    /// Provider identifier
    fn provider_identifier(&self) -> &'static str;
}

#[derive(Debug, Clone)]
pub struct LimitedRequestClient {
    request_tx: mpsc::Sender<(Request, oneshot::Sender<Result<Response, reqwest::Error>>)>,
}

impl LimitedRequestClient {
    pub fn new(client: Client, limit_number: usize, limit_duration: Duration) -> Self {
        let (tx, mut rx) =
            mpsc::channel::<(Request, oneshot::Sender<Result<Response, reqwest::Error>>)>(100);
        tokio::spawn(async move {
            let semaphore = Arc::new(Semaphore::new(limit_number));
            while let Some((req, resp_tx)) = rx.recv().await {
                let semaphore = semaphore.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    let permit = semaphore.acquire().await.unwrap();
                    let response = client.execute(req).await;

                    if let Err(_) = resp_tx.send(response) {
                        tracing::error!("Failed to send response: channel closed")
                    }
                    tokio::time::sleep(limit_duration).await;
                    drop(permit);
                });
            }
        });
        Self { request_tx: tx }
    }

    pub async fn request<T>(&self, req: Request) -> Result<T, AppError>
    where
        T: DeserializeOwned,
    {
        let (tx, rx) = oneshot::channel::<Result<Response, reqwest::Error>>();
        let url = req.url().to_string();
        self.request_tx
            .send((req, tx))
            .await
            .context("Failed to send request")?;
        let response = rx
            .await
            .map_err(|_| anyhow::anyhow!("failed to receive response: channel closed"))?
            .map_err(|e| {
                tracing::error!("Request in {} failed: {}", url, e);
                anyhow::anyhow!("Request failed: {}", e)
            })?;
        tracing::trace!("Succeded request: {}", url);
        match response.status().as_u16() {
            200 => Ok(response.json().await.context("Parse response in json")?),
            404 => Err(AppError::not_found("Provider responded with 404")),
            rest => Err(anyhow!("provider responded with status {}", rest).into()),
        }
    }
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
            rest => Err(anyhow::anyhow!(
                "{rest} is not recognized as metadata provider"
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

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
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
    #[schema(value_type = SerdeDuration)]
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

impl Into<MetadataSearchResult> for MovieMetadata {
    fn into(self) -> MetadataSearchResult {
        MetadataSearchResult {
            title: self.title,
            poster: self.poster,
            plot: self.plot,
            metadata_provider: self.metadata_provider,
            content_type: ContentType::Movie,
            metadata_id: self.metadata_id,
        }
    }
}

impl Into<MetadataSearchResult> for ShowMetadata {
    fn into(self) -> MetadataSearchResult {
        MetadataSearchResult {
            title: self.title,
            poster: self.poster,
            plot: self.plot,
            metadata_provider: self.metadata_provider,
            content_type: ContentType::Show,
            metadata_id: self.metadata_id,
        }
    }
}

impl EpisodeMetadata {
    pub async fn into_db_episode(self, season_id: i64, video_id: i64) -> DbEpisode {
        let blur_data = if let Some(poster) = &self.poster {
            poster.generate_blur_data().await.ok()
        } else {
            None
        };
        DbEpisode {
            id: None,
            video_id,
            season_id: season_id as i64,
            title: self.title,
            number: self.number as i64,
            plot: self.plot,
            release_date: self.release_date,
            poster: self.poster.map(|x| x.as_str().to_owned()),
            blur_data,
        }
    }
}

impl SeasonMetadata {
    pub async fn into_db_season(self, show_id: i64) -> DbSeason {
        let blur_data;
        let poster;
        if let Some(metadata_image) = self.poster {
            blur_data = metadata_image.generate_blur_data().await.ok();
            poster = Some(metadata_image.as_str().to_owned());
        } else {
            blur_data = None;
            poster = None;
        }
        DbSeason {
            id: None,
            show_id,
            number: self.number as i64,
            release_date: self.release_date,
            plot: self.plot,
            poster,
            blur_data,
        }
    }
}

impl ShowMetadata {
    pub async fn into_db_show(self) -> DbShow {
        let blur_data;
        let poster;
        if let Some(metadata_image) = self.poster {
            poster = Some(metadata_image.as_str().to_owned());
            blur_data = metadata_image.generate_blur_data().await.ok();
        } else {
            poster = None;
            blur_data = None;
        };
        let backdrop = self.backdrop.map(|p| p.as_str().to_owned());

        DbShow {
            id: None,
            title: self.title,
            release_date: self.release_date,
            poster,
            blur_data,
            backdrop,
            plot: self.plot,
        }
    }
}

impl MovieMetadata {
    pub async fn into_db_movie(self, video_id: i64) -> DbMovie {
        let blur_data;
        let poster;
        if let Some(metadata_image) = self.poster {
            poster = Some(metadata_image.as_str().to_owned());
            blur_data = metadata_image.generate_blur_data().await.ok();
        } else {
            poster = None;
            blur_data = None;
        };
        let backdrop = self.backdrop.map(|p| p.as_str().to_owned());

        DbMovie {
            id: None,
            video_id,
            title: self.title,
            release_date: self.release_date,
            poster,
            blur_data,
            backdrop,
            plot: self.plot,
        }
    }
}

#[tokio::test]
async fn rate_limit() {
    use axum::routing::post;
    use axum::{Json, Router};
    use serde::{Deserialize, Serialize};
    use tokio::task::JoinSet;

    #[derive(Clone, Serialize, Deserialize)]
    struct Count {
        value: usize,
    }

    impl Count {
        pub fn new(count: usize) -> Self {
            Self { value: count }
        }
    }

    async fn echo(count: Json<Count>) -> Json<Count> {
        use rand::Rng;
        let num = rand::thread_rng().gen_range(0..1000);
        tokio::time::sleep(Duration::from_millis(num)).await;
        count
    }

    let server_handle = tokio::spawn(async move {
        let app = Router::new().route("/", post(echo));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:32402")
            .await
            .unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    let reqwest = Client::new();
    let client = LimitedRequestClient::new(reqwest.clone(), 50, Duration::from_secs(1));
    let mut handles = JoinSet::new();
    let amount = 125;
    for i in 0..amount {
        let client = client.clone();
        let count = Count::new(i);
        let req = reqwest
            .post("http://127.0.0.1:32402/")
            .json(&count)
            .build()
            .unwrap();
        handles.spawn(async move {
            let count: Count = client.request(req).await.unwrap();
            dbg!(i);
            assert_eq!(i, count.value);
            return count;
        });
    }
    let mut sum = Vec::new();
    while let Some(Ok(res)) = handles.join_next().await {
        sum.push(res.value);
    }
    server_handle.abort();
    let expected: Vec<usize> = (0..amount).collect();
    assert_eq!(sum.len(), expected.len())
}
