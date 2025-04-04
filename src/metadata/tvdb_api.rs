use std::{
    collections::{BTreeSet, HashMap, HashSet},
    fmt::Display,
    sync::Mutex,
    time::Duration,
};

use lru::LruCache;
use reqwest::{
    Client, Method, Request, Url,
    header::{AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;

use crate::app_state::AppError;

use super::{
    ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, FetchParams,
    Language, METADATA_CACHE_SIZE, MetadataImage, MetadataProvider, MetadataSearchResult,
    MovieMetadata, MovieMetadataProvider, SeasonMetadata, ShowMetadata, ShowMetadataProvider,
    provod_agent, request_client::LimitedRequestClient,
};

#[derive(Debug)]
pub struct TvdbApi {
    client: LimitedRequestClient,
    show_cache: Mutex<LruCache<usize, TvdbSeriesExtendedRecord>>,
    movie_cache: Mutex<LruCache<usize, TvdbMovieExtendedRecord>>,
    base_url: Url,
}

impl TvdbApi {
    pub const RATE_LIMIT: usize = 10;
    pub const API_URL: &'static str = "https://api4.thetvdb.com/v4";
    pub fn new(api_key: Option<&str>) -> Self {
        let (client, base_url) = match api_key {
            Some(api_key) => {
                tracing::info!("Using personal tvdb token");
                let mut headers = HeaderMap::with_capacity(1);
                headers.insert(AUTHORIZATION, HeaderValue::from_str(api_key).unwrap());
                (
                    Client::builder()
                        .default_headers(headers)
                        .build()
                        .expect("build to succeed"),
                    Url::parse(Self::API_URL).expect("url to parse"),
                )
            }
            None => provod_agent::new_client("tvdb"),
        };

        let limited_client =
            LimitedRequestClient::new(client, Self::RATE_LIMIT, std::time::Duration::from_secs(1));
        Self {
            client: limited_client,
            show_cache: Mutex::new(LruCache::new(METADATA_CACHE_SIZE)),
            movie_cache: Mutex::new(LruCache::new(METADATA_CACHE_SIZE)),
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
    async fn fetch_movie(
        &self,
        id: usize,
        params: FetchParams,
    ) -> Result<TvdbMovieExtendedRecord, AppError> {
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
            .append_pair("short", "false")
            .append_pair("language", params.lang.as_str());
        let req = Request::new(Method::GET, url);
        let res: TvdbResponse<TvdbMovieExtendedRecord> = self.client.request(req).await?;
        let mut movie_cache = self.movie_cache.lock().unwrap();
        movie_cache.put(id, res.data.clone());
        Ok(res.data)
    }

    // https://api4.thetvdb.com/v4/series/366524/extended?meta=episodes&short=false
    async fn fetch_show(
        &self,
        id: usize,
        params: FetchParams,
    ) -> Result<TvdbSeriesExtendedRecord, AppError> {
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
            .append_pair("short", "false")
            .append_pair("language", params.lang.as_str());
        let req = Request::new(Method::GET, url);
        let res: TvdbResponse<TvdbSeriesExtendedRecord> = self.client.request(req).await?;
        let mut show_cache = self.show_cache.lock().unwrap();
        show_cache.put(id, res.data.clone());
        Ok(res.data)
    }

    fn get_movie_from_cache(&self, id: usize) -> Option<TvdbMovieExtendedRecord> {
        self.movie_cache.lock().unwrap().get(&id).cloned()
    }

    fn get_show_from_cache(&self, id: usize) -> Option<TvdbSeriesExtendedRecord> {
        self.show_cache.lock().unwrap().get(&id).cloned()
    }
}

#[async_trait::async_trait]
impl MovieMetadataProvider for TvdbApi {
    async fn movie(
        &self,
        movie_metadata_id: &str,
        params: FetchParams,
    ) -> Result<MovieMetadata, AppError> {
        let id = movie_metadata_id.parse()?;
        if let Some(movie) = self.get_movie_from_cache(id) {
            return Ok(movie.into());
        }
        let movie = self.fetch_movie(id, params).await?;
        Ok(movie.into())
    }

    fn provider_identifier(&self) -> &'static str {
        "tvdb"
    }
}

#[async_trait::async_trait]
impl ShowMetadataProvider for TvdbApi {
    async fn show(
        &self,
        show_id: &str,
        fetch_params: FetchParams,
    ) -> Result<ShowMetadata, AppError> {
        match self.get_show_from_cache(show_id.parse()?) {
            Some(s) => Ok(s.into()),
            None => self
                .fetch_show(show_id.parse()?, fetch_params)
                .await
                .map(Into::into),
        }
    }

    async fn season(
        &self,
        show_id: &str,
        season: usize,
        fetch_params: FetchParams,
    ) -> Result<SeasonMetadata, AppError> {
        let show = match self.get_show_from_cache(show_id.parse()?) {
            Some(s) => s,
            None => self.fetch_show(show_id.parse()?, fetch_params).await?,
        };
        let mut episodes = show.episodes;
        let episodes = episodes
            .into_iter()
            .filter(|e| e.season_number == season)
            .map(|e| e.into())
            .collect();
        let season = show
            .seasons
            .into_iter()
            .find(|s| s.number == season)
            .ok_or(AppError::not_found("Season not found"))?;

        let plot = season
            .overview_translations
            .and_then(|t| t.into_iter().next());
        let poster = season
            .image
            .and_then(|i| Some(MetadataImage::new(i.parse().ok()?)));

        Ok(SeasonMetadata {
            metadata_id: show.id.to_string(),
            metadata_provider: MetadataProvider::Tvdb,
            release_date: season.year,
            episodes,
            plot,
            poster,
            number: season.number,
        })
    }

    async fn episode(
        &self,
        show_id: &str,
        season: usize,
        episode: usize,
        fetch_params: FetchParams,
    ) -> Result<EpisodeMetadata, AppError> {
        let season = self.season(show_id, season, fetch_params).await?;
        season
            .episodes
            .into_iter()
            .find(|e| e.number == episode)
            .ok_or(AppError::not_found("episode is not found"))
    }

    fn provider_identifier(&self) -> &'static str {
        "tvdb"
    }
}

#[async_trait::async_trait]
impl DiscoverMetadataProvider for TvdbApi {
    async fn multi_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<MetadataSearchResult>, AppError> {
        Ok(self
            .search_multi(query)
            .await?
            .into_iter()
            .filter_map(|r| r.try_into().ok())
            .collect())
    }

    async fn show_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<ShowMetadata>, AppError> {
        Ok(self
            .search_series(query)
            .await?
            .into_iter()
            .map(|r| r.into())
            .collect())
    }

    async fn movie_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<MovieMetadata>, AppError> {
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
            ContentType::Movie => self
                .fetch_movie(id, FetchParams::default())
                .await
                .map(|x| x.remote_ids),
            ContentType::Show => self
                .fetch_show(id, FetchParams::default())
                .await
                .map(|x| x.remote_ids),
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
        // 3 means 16 / 9 image
        let backdrop = self
            .artworks
            .iter()
            .find(|a| a.artwork_type == 3)
            .and_then(|a| Some(MetadataImage::new(Url::parse(&a.image).ok()?)));

        // season_number 0 is extras
        let seasons: BTreeSet<_> = self
            .seasons
            .iter()
            .map(|s| s.number)
            .filter(|s| *s != 0)
            .collect();

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
        let poster = self
            .image
            .map(|p| MetadataImage::new(Url::parse(&p).unwrap()));
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
            metadata_id: self.id.to_string(),
            metadata_provider: MetadataProvider::Tvdb,
            poster,
            backdrop,
            plot,
            release_date: self.first_release.map(|r| r.date),
            runtime: self.runtime.map(|t| Duration::from_secs(t as u64 * 60)),
            title: self.name,
        }
    }
}

impl From<TvdbSearchResult> for MovieMetadata {
    fn from(val: TvdbSearchResult) -> Self {
        let poster = val
            .image_url
            .and_then(|url| url.parse().ok())
            .map(MetadataImage::new);

        MovieMetadata {
            metadata_id: val.tvdb_id,
            metadata_provider: MetadataProvider::Tvdb,
            poster,
            backdrop: None,
            plot: val.overview,
            release_date: val.first_air_time,
            runtime: None,
            title: val.name,
        }
    }
}

impl From<TvdbSearchResult> for ShowMetadata {
    fn from(val: TvdbSearchResult) -> Self {
        let poster = val
            .image_url
            .and_then(|url| url.parse().ok())
            .map(MetadataImage::new);

        ShowMetadata {
            metadata_id: val.tvdb_id,
            metadata_provider: MetadataProvider::Tvdb,
            poster,
            backdrop: None,
            plot: val.overview,
            release_date: val.first_air_time,
            title: val.name,
            ..Default::default()
        }
    }
}

impl From<TvdbEpisode> for EpisodeMetadata {
    fn from(value: TvdbEpisode) -> Self {
        let poster = value.image.map(Into::into);
        Self {
            metadata_id: value.id.to_string(),
            metadata_provider: MetadataProvider::Tvdb,
            release_date: value.year,
            number: value.number,
            title: value.name,
            plot: value.overview,
            season_number: value.season_number,
            runtime: value.runtime.map(|r| Duration::from_secs(r as u64 * 60)),
            poster,
        }
    }
}
impl TryFrom<TvdbSearchResult> for MetadataSearchResult {
    type Error = AppError;
    fn try_from(val: TvdbSearchResult) -> Result<Self, Self::Error> {
        let content_type = match val.search_type.as_ref() {
            "series" => ContentType::Show,
            "movie" => ContentType::Movie,
            _ => Err(anyhow::anyhow!("Unknown content type: {}", val.search_type))?,
        };
        let poster = val
            .image_url
            .and_then(|url| url.parse().ok())
            .map(MetadataImage::new);

        Ok(MetadataSearchResult {
            title: val.name,
            poster,
            plot: val.overview,
            metadata_provider: MetadataProvider::Tvdb,
            content_type,
            metadata_id: val.tvdb_id,
        })
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
pub struct TvdbPoster(String);

impl TvdbPoster {
    const BASE_PATH: &str = "https://artworks.thetvdb.com";
}

impl Display for TvdbPoster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", Self::BASE_PATH, self.0)
    }
}

impl From<TvdbPoster> for MetadataImage {
    fn from(value: TvdbPoster) -> Self {
        Self(
            value
                .to_string()
                .parse()
                .expect("Tvdb images are valid urls"),
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
struct TvdbSearchResult {
    id: String,
    image_url: Option<String>,
    name: String,
    first_air_time: Option<String>,
    overview: Option<String>,
    primary_language: Option<String>,
    #[serde(rename = "type")]
    search_type: String,
    tvdb_id: String,
    year: Option<String>,
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
    aired: Option<String>,
    runtime: Option<usize>,
    name_translations: Option<Vec<String>>,
    overview: Option<String>,
    overview_translations: Option<Vec<String>>,
    image: Option<TvdbPoster>,
    image_type: Option<usize>,
    seasons: Option<Vec<TvdbSeasonBaseRecord>>,
    number: usize,
    season_number: usize,
    last_updated: Option<String>,
    year: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbSeasonBaseRecord {
    id: usize,
    image: Option<String>,
    image_type: Option<usize>,
    last_updated: Option<String>,
    name: Option<String>,
    name_translations: Option<Vec<String>>,
    number: usize,
    overview_translations: Option<Vec<String>>,
    series_id: usize,
    year: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbGenre {
    id: usize,
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbTrailer {
    id: usize,
    name: String,
    url: String,
    language: Option<String>,
    runtime: Option<usize>,
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
    thumbnail: Option<String>,
    language: Option<String>,
    #[serde(rename = "type")]
    artwork_type: usize,
    width: usize,
    height: usize,
    includes_text: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbSeriesExtendedRecord {
    id: usize,
    name: String,
    image: Option<String>,
    name_translations: Option<Vec<String>>,
    overview_translations: Option<Vec<String>>,
    first_aired: Option<String>,
    last_aired: Option<String>,
    next_aired: Option<String>,
    original_country: Option<String>,
    original_language: Option<String>,
    seasons: Vec<TvdbSeasonBaseRecord>,
    is_order_randomized: bool,
    last_updated: Option<String>,
    average_runtime: Option<usize>,
    episodes: Vec<TvdbEpisode>,
    overview: Option<String>,
    year: Option<String>,
    artworks: Vec<TvdbArtwork>,
    genres: Vec<TvdbGenre>,
    remote_ids: Vec<TvdbRemoteIds>,
    characters: Option<Vec<TvdbCharacter>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TvdbMovieExtendedRecord {
    id: usize,
    name: String,
    image: Option<String>,
    translations: TvdbTranslations,
    runtime: Option<usize>,
    year: Option<String>,
    genres: Vec<TvdbGenre>,
    artworks: Vec<TvdbArtwork>,
    remote_ids: Vec<TvdbRemoteIds>,
    characters: Vec<TvdbCharacter>,
    original_country: Option<String>,
    original_language: Option<String>,
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
    overview: Option<String>,
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
    name: Option<String>,
    people_id: Option<usize>,
    series_id: Option<usize>,
    series: Option<usize>,
    movie: Option<usize>,
    movie_id: Option<usize>,
    episode_id: Option<usize>,
    #[serde(rename = "type")]
    character_type: usize,
    image: Option<String>,
    url: Option<String>,
    name_translations: Option<Vec<String>>,
    overview_translations: Option<Vec<String>>,
    people_type: Option<String>,
    person_name: Option<String>,
    #[serde(rename = "personImgURL")]
    person_img_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct TvdbResponse<T> {
    status: String,
    data: T,
}
