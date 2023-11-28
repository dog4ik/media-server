use std::io::Cursor;

use crate::{
    db::{DbEpisode, DbMovie, DbSeason, DbShow},
    tmdb_api::TmdbApi,
};
use anyhow::Result;
use base64::{engine::general_purpose, Engine};
use image::{imageops::FilterType, GenericImageView};
use reqwest::Url;
use serde::Serialize;
use tracing::instrument;

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
    const BLUR_DATA_IMG_WIDTH: u32 = 30;

    //NOTE: This is slow (image crate)
    #[instrument(name = "Blur data", level = "trace")]
    pub async fn generate_blur_data(&self) -> Result<String, anyhow::Error> {
        let MetadataImage(url) = self;
        let bytes = reqwest::get(url.clone()).await?.bytes().await?;
        let image = image::load_from_memory(&bytes)?;
        let (img_width, img_height) = image.dimensions();
        let img_aspect_ratio: f64 = img_width as f64 / img_height as f64;
        let resized_image = image.resize(
            Self::BLUR_DATA_IMG_WIDTH,
            (Self::BLUR_DATA_IMG_WIDTH as f64 / img_aspect_ratio).floor() as u32,
            FilterType::Triangle,
        );
        let mut image_data: Vec<u8> = Vec::new();
        resized_image.write_to(
            &mut Cursor::new(&mut image_data),
            image::ImageOutputFormat::Jpeg(80),
        )?;
        Ok(general_purpose::STANDARD_NO_PAD.encode(image_data))
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
        movie_title: &str,
    ) -> impl std::future::Future<Output = Result<MovieMetadata>> + Send;

    /// Provider identifier
    fn provider_identifier(&self) -> &'static str;
}

pub trait ShowMetadataProvider {
    /// Query for show
    #[allow(async_fn_in_trait)]
    fn show(
        &self,
        show_title: &str,
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

impl MovieMetadataProvider for TmdbApi {
    async fn movie(&self, movie_title: &str) -> Result<MovieMetadata> {
        let movies = self.search_movie(movie_title).await?;
        movies
            .results
            .into_iter()
            .next()
            .ok_or(anyhow::anyhow!("results are empty"))
            .map(|s| s.into())
    }

    fn provider_identifier(&self) -> &'static str {
        "tmdb"
    }
}

impl ShowMetadataProvider for TmdbApi {
    async fn show(&self, show_title: &str) -> Result<ShowMetadata> {
        let shows = self.search_tv_show(show_title).await?;
        shows
            .results
            .into_iter()
            .next()
            .ok_or(anyhow::anyhow!("results are empty"))
            .map(|s| s.into())
    }

    async fn season(&self, metadata_show_id: &str, season: usize) -> Result<SeasonMetadata> {
        let show_id = metadata_show_id.parse().expect("tmdb ids to be numbers");
        self.tv_show_season(show_id, season).await.map(|s| s.into())
    }

    async fn episode(
        &self,
        metadata_show_id: &str,
        season: usize,
        episode: usize,
    ) -> Result<EpisodeMetadata> {
        let show_id = metadata_show_id.parse().expect("tmdb ids to be numbers");
        self.tv_show_episode(show_id, season, episode)
            .await
            .map(|e| e.into())
    }

    fn provider_identifier(&self) -> &'static str {
        "tmdb"
    }
}

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
