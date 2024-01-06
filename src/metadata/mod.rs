use std::{sync::Arc, time::Duration};

use crate::{
    db::{DbEpisode, DbMovie, DbSeason, DbShow},
    ffmpeg,
    library::{movie::MovieIdentifier, show::ShowIdentifier, LibraryFile},
};
use anyhow::Result;
use reqwest::{Client, Request, Response, Url};
use serde::{de::DeserializeOwned, Serialize};
use tokio::sync::{mpsc, oneshot, Semaphore};
use tracing::instrument;

pub mod tmdb_api;

#[derive(Debug, Clone)]
pub struct MetadataImage(pub Url);

impl Serialize for MetadataImage {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.0.as_str())
    }
}

impl MetadataImage {
    pub fn new(url: Url) -> Self {
        MetadataImage(url)
    }
    const BLUR_DATA_IMG_WIDTH: i32 = 30;

    //NOTE: This is slow (image crate)
    #[instrument(name = "Blur data", level = "trace")]
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

pub trait MovieMetadataProvider {
    /// Query for movie
    #[allow(async_fn_in_trait)]
    fn movie(
        &self,
        movie: &LibraryFile<MovieIdentifier>,
    ) -> impl std::future::Future<Output = Result<MovieMetadata>> + Send;

    /// Provider identifier
    fn provider_identifier(&self) -> &'static str;
}

pub trait ShowMetadataProvider {
    /// Query for show
    #[allow(async_fn_in_trait)]
    fn show(
        &self,
        show: &LibraryFile<ShowIdentifier>,
    ) -> impl std::future::Future<Output = Result<ShowMetadata>> + Send;

    /// Query for season
    #[allow(async_fn_in_trait)]
    fn season(
        &self,
        metadata_show_id: &str,
        season: usize,
    ) -> impl std::future::Future<Output = Result<SeasonMetadata>> + Send;

    /// Query for episode
    #[allow(async_fn_in_trait)]
    fn episode(
        &self,
        metadata_show_id: &str,
        season: usize,
        episode: usize,
    ) -> impl std::future::Future<Output = Result<EpisodeMetadata>> + Send;

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

    pub async fn request<T>(&self, req: Request) -> anyhow::Result<T>
    where
        T: DeserializeOwned,
    {
        let (tx, rx) = oneshot::channel::<Result<Response, reqwest::Error>>();
        let url = req.url().clone();
        self.request_tx.clone().send((req, tx)).await?;
        let response = rx
            .await
            .map_err(|_| anyhow::anyhow!("failed to receive response: channel closed"))?
            .map_err(|e| {
                tracing::error!("Request in {} failed: {}", url, e);
                return anyhow::anyhow!("Request failed: {}", e);
            })?;
        tracing::trace!("Succeded request: {}", url);
        Ok(response.json().await?)
    }
}

// types

#[derive(Debug, Clone, Serialize)]
pub struct MovieMetadata {
    pub metadata_id: Option<String>,
    pub metadata_provider: &'static str,
    pub poster: Option<MetadataImage>,
    pub backdrop: Option<MetadataImage>,
    pub rating: f64,
    pub plot: String,
    pub release_date: String,
    pub language: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShowMetadata {
    pub metadata_id: Option<String>,
    pub metadata_provider: &'static str,
    pub poster: Option<MetadataImage>,
    pub backdrop: Option<MetadataImage>,
    pub rating: f64,
    pub plot: String,
    pub release_date: String,
    pub language: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeasonMetadata {
    pub metadata_id: Option<String>,
    pub metadata_provider: &'static str,
    pub release_date: String,
    pub episodes_amount: usize,
    pub title: String,
    pub plot: String,
    pub poster: Option<MetadataImage>,
    pub number: usize,
    pub rating: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EpisodeMetadata {
    pub metadata_id: Option<String>,
    pub metadata_provider: &'static str,
    pub release_date: String,
    pub number: usize,
    pub title: String,
    pub plot: String,
    pub season_number: usize,
    pub poster: MetadataImage,
    pub rating: f64,
}

impl EpisodeMetadata {
    pub async fn into_db_episode(self, season_id: i64, video_id: i64) -> DbEpisode {
        let blur_data = self.poster.generate_blur_data().await.ok();
        DbEpisode {
            id: None,
            video_id,
            metadata_id: self.metadata_id,
            metadata_provider: self.metadata_provider.to_string(),
            season_id: season_id as i64,
            title: self.title,
            number: self.number as i64,
            plot: self.plot,
            release_date: self.release_date,
            rating: self.rating,
            poster: self.poster.as_str().to_owned(),
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
            metadata_id: self.metadata_id,
            metadata_provider: self.metadata_provider.to_string(),
            show_id,
            number: self.number as i64,
            release_date: self.release_date,
            plot: self.plot,
            poster,
            rating: self.rating,
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
            metadata_id: self.metadata_id,
            metadata_provider: self.metadata_provider.to_string(),
            title: self.title,
            release_date: self.release_date,
            poster,
            blur_data,
            backdrop,
            rating: self.rating,
            plot: self.plot,
            original_language: self.language,
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
            metadata_id: self.metadata_id,
            metadata_provider: self.metadata_provider.to_string(),
            title: self.title,
            release_date: self.release_date,
            poster,
            blur_data,
            backdrop,
            rating: self.rating,
            plot: self.plot,
            original_language: self.language,
        }
    }
}

#[tokio::test]
async fn blur_data_generation() {
    use std::str::FromStr;
    let image = MetadataImage::new(
        Url::from_str("https://image.tmdb.org/t/p/original/%2Fgrt1km00cjrwAckfgO3QGiHYq89.jpg")
            .unwrap(),
    );
    let blur_data = image.generate_blur_data().await.unwrap();
    assert!(blur_data.len() > 100);
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
