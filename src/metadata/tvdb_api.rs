use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client, Method, Request, Url,
};
use serde::Deserialize;

use crate::app_state::AppError;

use super::{
    ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata,
    LimitedRequestClient, MetadataImage, MetadataProvider, MetadataSearchResult, MovieMetadata,
    MovieMetadataProvider, SeasonMetadata, ShowMetadata, ShowMetadataProvider,
};

#[derive(Debug)]
pub struct TvdbApi {
    client: LimitedRequestClient,
    show_cache: Mutex<HashMap<usize, TvdbSeriesExtendedRecord>>,
    movie_cache: Mutex<HashMap<usize, TvdbMovieExtendedRecord>>,
    base_url: Url,
}

impl TvdbApi {
    pub const RATE_LIMIT: usize = 10;
    pub const API_URL: &'static str = "https://api4.thetvdb.com/v4";
    pub fn new(api_key: &str) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(api_key).unwrap());

        let client = Client::builder()
            .default_headers(headers)
            .build()
            .expect("build to succeed");
        let limited_client =
            LimitedRequestClient::new(client, Self::RATE_LIMIT, std::time::Duration::from_secs(1));
        let base_url = Url::parse(Self::API_URL).expect("url to parse");
        Self {
            client: limited_client,
            show_cache: Mutex::new(HashMap::new()),
            movie_cache: Mutex::new(HashMap::new()),
            base_url,
        }
    }

    // https://api4.thetvdb.com/v4/search?query=halo&type=series
    async fn search_series(&self, query: &str) -> Result<Vec<TvdbSearchResult>, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map(|mut p| {
                p.push("search");
            })
            .unwrap();
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("type", "series");
        let request = Request::new(Method::GET, url);
        let res: TvdbResponse<Vec<TvdbSearchResult>> = self.client.request(request).await?;
        Ok(res.data)
    }

    // https://api4.thetvdb.com/v4/search?query=inception&type=movie
    async fn search_movie(&self, query: &str) -> Result<Vec<TvdbSearchResult>, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map(|mut p| {
                p.push("search");
            })
            .unwrap();
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("type", "movie");
        let request = Request::new(Method::GET, url);
        let res: TvdbResponse<Vec<TvdbSearchResult>> = self.client.request(request).await?;
        Ok(res.data)
    }

    // https://api4.thetvdb.com/v4/search?query=inception
    async fn search_multi(&self, query: &str) -> Result<Vec<TvdbSearchResult>, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map(|mut p| {
                p.push("search");
            })
            .unwrap();
        url.query_pairs_mut().append_pair("query", query);
        let request = Request::new(Method::GET, url);
        let res: TvdbResponse<Vec<TvdbSearchResult>> = self.client.request(request).await?;
        Ok(res.data)
    }

    // https://api4.thetvdb.com/v4/movies/113/extended?meta=translations&short=false
    async fn fetch_movie(&self, id: usize) -> Result<TvdbMovieExtendedRecord, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map(|mut path| {
                path.push("movies");
                path.push(&id.to_string());
                path.push("extended");
            })
            .unwrap();
        url.query_pairs_mut()
            .append_pair("meta", "translations")
            .append_pair("short", "false");
        let req = Request::new(Method::GET, url);
        let res: TvdbResponse<TvdbMovieExtendedRecord> = self.client.request(req).await?;
        let mut movie_cache = self.movie_cache.lock().unwrap();
        movie_cache.insert(id, res.data.clone());
        Ok(res.data)
    }

    // https://api4.thetvdb.com/v4/series/366524/extended?meta=episodes&short=false
    async fn fetch_show(&self, id: usize) -> Result<TvdbSeriesExtendedRecord, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .map(|mut path| {
                path.push("series");
                path.push(&id.to_string());
                path.push("extended");
            })
            .unwrap();
        url.query_pairs_mut()
            .append_pair("meta", "episodes")
            .append_pair("short", "false");
        let req = Request::new(Method::GET, url);
        let res: TvdbResponse<TvdbSeriesExtendedRecord> = self.client.request(req).await?;
        let mut show_cache = self.show_cache.lock().unwrap();
        show_cache.insert(id, res.data.clone());
        Ok(res.data)
    }

    fn get_movie_from_cache(&self, id: usize) -> Option<TvdbMovieExtendedRecord> {
        self.movie_cache.lock().unwrap().get(&id).cloned()
    }

    fn get_show_from_cache(&self, id: usize) -> Option<TvdbSeriesExtendedRecord> {
        self.show_cache.lock().unwrap().get(&id).cloned()
    }
}

#[axum::async_trait]
impl MovieMetadataProvider for TvdbApi {
    async fn movie(&self, movie_metadata_id: &str) -> Result<MovieMetadata, AppError> {
        let id = movie_metadata_id.parse()?;
        if let Some(movie) = self.get_movie_from_cache(id) {
            return Ok(movie.into());
        }
        let movie = self.fetch_movie(id).await?;
        Ok(movie.into())
    }

    fn provider_identifier(&self) -> &'static str {
        "tvdb"
    }
}

#[axum::async_trait]
impl ShowMetadataProvider for TvdbApi {
    async fn show(&self, show_id: &str) -> Result<ShowMetadata, AppError> {
        todo!()
    }

    async fn season(&self, show_id: &str, season: usize) -> Result<SeasonMetadata, AppError> {
        todo!()
    }

    async fn episode(
        &self,
        show_id: &str,
        season: usize,
        episode: usize,
    ) -> Result<EpisodeMetadata, AppError> {
        todo!()
    }

    fn provider_identifier(&self) -> &'static str {
        "tvdb"
    }
}

#[axum::async_trait]
impl DiscoverMetadataProvider for TvdbApi {
    async fn multi_search(&self, query: &str) -> Result<Vec<MetadataSearchResult>, AppError> {
        Ok(self
            .search_multi(query)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect())
    }

    async fn show_search(&self, query: &str) -> Result<Vec<ShowMetadata>, AppError> {
        Ok(self
            .search_series(query)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect())
    }

    async fn movie_search(&self, query: &str) -> Result<Vec<MovieMetadata>, AppError> {
        Ok(self
            .search_movie(query)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect())
    }

    async fn external_ids(
        &self,
        content_id: &str,
        content_hint: ContentType,
    ) -> Result<Vec<ExternalIdMetadata>, AppError> {
        let id = content_id.parse()?;
        let retrieve_ids = |ids: Vec<TvdbRemoteIds>| {
            ids.into_iter()
                .filter_map(|id| id.try_into().ok())
                .collect()
        };

        let cached_ids = match content_hint {
            ContentType::Movie => self.get_movie_from_cache(id).map(|c| c.remote_ids),
            ContentType::Show => self.get_show_from_cache(id).map(|c| c.remote_ids),
        };
        if let Some(ids) = cached_ids {
            return Ok(retrieve_ids(ids));
        }

        let fresh_ids = match content_hint {
            ContentType::Movie => self.fetch_movie(id).await.map(|x| x.remote_ids),
            ContentType::Show => self.fetch_show(id).await.map(|x| x.remote_ids),
        }?;
        return Ok(retrieve_ids(fresh_ids));
    }

    fn provider_identifier(&self) -> &'static str {
        "tvdb"
    }
}

impl Into<ShowMetadata> for TvdbSeriesExtendedRecord {
    fn into(self) -> ShowMetadata {
        let poster = self
            .image
            .map(|p| MetadataImage::new(Url::parse(&p).unwrap()));
        // 3 is somehow 16 / 9 image
        let backdrop = self
            .artworks
            .iter()
            .find(|a| a.artwork_type == 3)
            .and_then(|a| Some(MetadataImage::new(Url::parse(&a.image).ok()?)));

        let seasons: HashSet<_> = self.episodes.iter().map(|x| x.season_number).collect();

        ShowMetadata {
            metadata_id: self.id.to_string(),
            metadata_provider: MetadataProvider::Tvdb,
            poster,
            backdrop,
            plot: self.overview,
            release_date: self.first_aired,
            title: self.name,
            episodes_amount: Some(self.episodes.len()),
            seasons: Some(seasons.into_iter().collect()),
        }
    }
}

impl Into<MovieMetadata> for TvdbMovieExtendedRecord {
    fn into(self) -> MovieMetadata {
        let poster = Some(self.image).map(|p| MetadataImage::new(Url::parse(&p).unwrap()));
        // 3 is somehow 16 / 9 image
        let backdrop = self
            .artworks
            .iter()
            .find(|a| a.artwork_type == 3)
            .and_then(|a| Some(MetadataImage::new(Url::parse(&a.image).ok()?)));
        let plot = self
            .translations
            .overview_translations
            .into_iter()
            .find(|t| t.is_primary.unwrap_or(false))
            .unwrap()
            .overview;
        MovieMetadata {
            metadata_id: self.id,
            metadata_provider: MetadataProvider::Tvdb,
            poster,
            backdrop,
            plot: Some(plot),
            release_date: self.first_release.map(|r| r.date),
            title: self.name,
        }
    }
}

impl From<TvdbSearchResult> for MovieMetadata {
    fn from(val: TvdbSearchResult) -> Self {
        let poster = MetadataImage::new(val.image_url.parse().unwrap());

        MovieMetadata {
            metadata_id: val.tvdb_id,
            metadata_provider: MetadataProvider::Tvdb,
            poster: Some(poster),
            backdrop: None,
            plot: val.overview,
            release_date: val.first_air_time,
            title: val.name,
        }
    }
}

impl From<TvdbSearchResult> for ShowMetadata {
    fn from(val: TvdbSearchResult) -> Self {
        let poster = MetadataImage::new(val.image_url.parse().unwrap());

        ShowMetadata {
            metadata_id: val.tvdb_id,
            metadata_provider: MetadataProvider::Tvdb,
            poster: Some(poster),
            backdrop: None,
            plot: val.overview,
            release_date: val.first_air_time,
            title: val.name,
            ..Default::default()
        }
    }
}

impl From<TvdbSearchResult> for MetadataSearchResult {
    fn from(val: TvdbSearchResult) -> Self {
        let content_type = match val.search_type.as_ref() {
            "series" => ContentType::Show,
            "movie" => ContentType::Movie,
            rest => panic!("unknown content type: {}", rest),
        };
        let poster = MetadataImage::new(val.image_url.parse().unwrap());

        MetadataSearchResult {
            title: val.name,
            poster: Some(poster),
            plot: val.overview,
            metadata_provider: MetadataProvider::Tvdb,
            content_type,
            metadata_id: val.tvdb_id,
        }
    }
}

impl TryInto<ExternalIdMetadata> for TvdbRemoteIds {
    type Error = anyhow::Error;

    fn try_into(self) -> Result<ExternalIdMetadata, Self::Error> {
        let provider = match self.source_name.as_ref() {
            "IMDB" => MetadataProvider::Imdb,
            "TheMovieDB.com" => MetadataProvider::Tvdb,
            rest => return Err(anyhow::anyhow!("{rest} is not supported")),
        };
        Ok(ExternalIdMetadata {
            provider,
            id: self.id,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TvdbSearchResult {
    country: String,
    id: String,
    image_url: String,
    name: String,
    first_air_time: Option<String>,
    overview: Option<String>,
    primary_language: String,
    primary_type: String,
    status: Option<String>,
    #[serde(rename = "type")]
    search_type: String,
    tvdb_id: String,
    year: Option<String>,
    slug: String,
    overviews: Option<HashMap<String, String>>,
    translations: HashMap<String, String>,
    remote_ids: Option<Vec<TvdbRemoteIds>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbEpisode {
    id: usize,
    series_id: usize,
    name: String,
    aired: String,
    runtime: Option<usize>,
    name_translations: Option<Vec<String>>,
    overview: Option<String>,
    overview_translations: Option<Vec<String>>,
    image: Option<String>,
    image_type: usize,
    is_movie: usize,
    seasons: Option<Vec<TvdbSeasonBaseRecord>>,
    number: usize,
    season_number: usize,
    last_updated: String,
    finale_type: Option<String>,
    year: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbSeasonBaseRecord {
    id: usize,
    image: String,
    image_type: usize,
    last_updated: String,
    name: String,
    name_translations: Option<Vec<String>>,
    number: usize,
    overview_translations: Option<Vec<String>>,
    series_id: usize,
    year: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbGenre {
    id: usize,
    name: String,
    slug: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbTrailer {
    id: usize,
    name: String,
    url: String,
    language: String,
    runtime: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbRemoteIds {
    id: String,
    #[serde(rename = "type")]
    id_type: usize,
    source_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbArtwork {
    id: usize,
    image: String,
    thumbnail: String,
    language: String,
    #[serde(rename = "type")]
    artwork_type: usize,
    score: usize,
    width: usize,
    height: usize,
    includes_text: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbSeriesExtendedRecord {
    id: usize,
    name: String,
    slug: Option<String>,
    image: Option<String>,
    name_translations: Option<Vec<String>>,
    overview_translations: Option<Vec<String>>,
    first_aired: Option<String>,
    last_aired: String,
    next_aired: String,
    score: usize,
    original_country: String,
    original_language: String,
    default_season_type: usize,
    is_order_randomized: bool,
    last_updated: String,
    average_runtime: Option<usize>,
    episodes: Vec<TvdbEpisode>,
    overview: Option<String>,
    year: String,
    artworks: Vec<TvdbArtwork>,
    genres: Vec<TvdbGenre>,
    trailers: Vec<TvdbTrailer>,
    remote_ids: Vec<TvdbRemoteIds>,
    characeters: Vec<TvdbCharacter>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbMovieExtendedRecord {
    id: String,
    name: String,
    image: String,
    translations: TvdbTranslations,
    score: usize,
    runtime: Option<usize>,
    last_updated: String,
    year: String,
    trailers: Vec<TvdbTrailer>,
    genres: Vec<TvdbGenre>,
    artworks: Vec<TvdbArtwork>,
    remote_ids: Vec<TvdbRemoteIds>,
    characters: Vec<TvdbCharacter>,
    budget: Option<String>,
    box_office: Option<String>,
    original_country: String,
    original_language: String,
    first_release: Option<TvdbRelease>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbTranslations {
    name_translations: Vec<TvdbTranslation>,
    overview_translations: Vec<TvdbTranslation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbTranslation {
    overview: String,
    language: String,
    tagline: Option<String>,
    is_primary: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
struct TvdbRelease {
    country: Option<String>,
    date: String,
    detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbCharacter {
    id: usize,
    name: String,
    people_id: usize,
    series_id: Option<usize>,
    series: Option<usize>,
    movie: Option<usize>,
    movie_id: Option<usize>,
    episode_id: Option<usize>,
    #[serde(rename = "type")]
    character_type: usize,
    image: Option<String>,
    sort: usize,
    is_featured: bool,
    url: String,
    name_translations: Option<Vec<String>>,
    overview_translations: Option<Vec<String>>,
    people_type: String,
    person_name: String,
    #[serde(rename = "personImgURL")]
    person_img_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TvdbResponse<T> {
    status: String,
    data: T,
}
