use std::sync::Mutex;
use std::{collections::HashMap, time::Duration};

use anyhow::anyhow;
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT_ENCODING},
    Client, Method, Request, Url,
};
use serde::Deserialize;
use time::{Date, OffsetDateTime};

use crate::app_state::AppError;

use super::{
    ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata,
    LimitedRequestClient, MetadataImage, MetadataProvider, MetadataSearchResult, MovieMetadata,
    MovieMetadataProvider, SeasonMetadata, ShowMetadata, ShowMetadataProvider,
};

#[derive(Debug)]
pub struct TmdbApi {
    pub api_key: String,
    pub base_url: Url,
    client: LimitedRequestClient,
    episodes_cache: Mutex<HashMap<usize, HashMap<usize, Vec<TmdbSeasonEpisode>>>>,
}

#[derive(Debug, Clone)]
pub enum PosterSizes {
    W92,
    W154,
    W185,
    W342,
    W500,
    W780,
    Original,
}

impl PosterSizes {
    pub fn get_size(&self) -> &'static str {
        match self {
            PosterSizes::W92 => "w92",
            PosterSizes::W154 => "w154",
            PosterSizes::W185 => "w185",
            PosterSizes::W342 => "w342",
            PosterSizes::W500 => "w500",
            PosterSizes::W780 => "w780",
            PosterSizes::Original => "original",
        }
    }
}

#[derive(Debug, Clone)]
pub struct TmdbImage {
    url: Url,
}

impl TmdbImage {
    pub const IMG_BASE_URL: &'static str = "https://image.tmdb.org/t/p";
    pub fn new(appendix: &str, size: PosterSizes) -> Self {
        let mut url = Url::parse(Self::IMG_BASE_URL).unwrap();
        url.path_segments_mut()
            .unwrap()
            .push(size.get_size())
            .push(appendix);
        Self { url }
    }

    pub fn url(&self) -> &str {
        self.url.as_ref()
    }
}

impl From<TmdbImage> for MetadataImage {
    fn from(val: TmdbImage) -> Self {
        MetadataImage::new(val.url)
    }
}

impl TmdbApi {
    const API_URL: &'static str = "http://api.themoviedb.org/3";
    const RATE_LIMIT: usize = 50;
    pub fn new(api_key: String) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_str("compress").unwrap());

        let params = [("api_key", api_key.clone())];
        let client = Client::builder()
            .default_headers(headers)
            .build()
            .expect("build to succeed");
        let limited_client =
            LimitedRequestClient::new(client, Self::RATE_LIMIT, std::time::Duration::from_secs(1));
        let base_url = Url::parse_with_params(Self::API_URL, params).expect("url to parse");
        Self {
            api_key,
            client: limited_client,
            episodes_cache: Mutex::new(HashMap::new()),
            base_url,
        }
    }

    pub async fn search_movie(
        &self,
        query: &str,
    ) -> Result<TmdbSearch<TmdbSearchMovieResult>, AppError> {
        let query = [("query", query)];
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("search").push("movie");
        url.query_pairs_mut().extend_pairs(query);
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    pub async fn search_tv_show(
        &self,
        query: &str,
    ) -> Result<TmdbSearch<TmdbSearchShowResult>, AppError> {
        let query = [("query", query)];
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("search").push("tv");
        url.query_pairs_mut().extend_pairs(query);
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    async fn search_multi(&self, query: &str) -> Result<TmdbSearch<TmdbFindMultiResult>, AppError> {
        let query = [("query", query)];
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("search")
            .push("multi");
        url.query_pairs_mut().extend_pairs(query);
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    pub async fn tv_show_season(
        &self,
        tmdb_show_id: usize,
        season: usize,
    ) -> Result<TmdbShowSeason, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("tv")
            .push(&tmdb_show_id.to_string())
            .push("season")
            .push(&season.to_string());
        let req = Request::new(Method::GET, url);
        let response: TmdbShowSeason = self.client.request(req).await?;

        self.update_cache(tmdb_show_id, season, response.episodes.clone());

        Ok(response)
    }

    pub async fn tv_show_episode(
        &self,
        tmdb_show_id: usize,
        season: usize,
        episode: usize,
    ) -> Result<TmdbSeasonEpisode, AppError> {
        //FIX: case when episode cant be found by metadata provider while we have its siblings in
        //cache
        if let Some(cache_episode) = self.get_from_cache(tmdb_show_id, season, episode) {
            tracing::debug!(
                "Reused cache entry for {} season: {} episode: {}",
                tmdb_show_id,
                season,
                episode
            );
            Ok(cache_episode)
        } else {
            let response = self.tv_show_season(tmdb_show_id, season).await?;
            self.update_cache(tmdb_show_id, season, response.episodes);
            self.get_from_cache(tmdb_show_id, season, episode)
                .ok_or(AppError::not_found("Could not found episode in cache"))
        }
    }

    pub async fn find_by_imdb_id(&self, imdb_id: &str) -> Result<TmdbFindByIdResult, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("find").push(imdb_id);
        url.query_pairs_mut()
            .append_pair("external_source", "imdb_id");
        let req = Request::new(Method::GET, url);
        let res = self.client.request(req).await?;
        Ok(res)
    }

    pub async fn movie_external_ids(&self, id: usize) -> Result<TmdbExternalIds, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("movie")
            .push(&id.to_string())
            .push("external_ids");
        let req = Request::new(Method::GET, url);
        let res = self.client.request(req).await?;
        Ok(res)
    }

    pub async fn show_external_ids(&self, id: usize) -> Result<TmdbExternalIds, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("tv")
            .push(&id.to_string())
            .push("external_ids");
        let req = Request::new(Method::GET, url);
        let res = self.client.request(req).await?;
        Ok(res)
    }

    pub async fn movie_details(&self, movie_id: usize) -> Result<TmdbMovieDetails, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("movie")
            .push(&movie_id.to_string());
        let req = Request::new(Method::GET, url);
        let res = self.client.request(req).await?;
        Ok(res)
    }

    pub async fn show_details(&self, show_id: usize) -> Result<TmdbShowDetails, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("tv")
            .push(&show_id.to_string());
        let req = Request::new(Method::GET, url);
        let res = self.client.request(req).await?;
        Ok(res)
    }

    fn update_cache(&self, tmdb_show_id: usize, season: usize, episodes: Vec<TmdbSeasonEpisode>) {
        let mut episodes_cache = self.episodes_cache.lock().unwrap();
        match episodes_cache.try_insert(tmdb_show_id, HashMap::new()) {
            Ok(entry) => {
                entry.insert(season, episodes);
            }
            Err(_) => {
                let show = episodes_cache
                    .get_mut(&tmdb_show_id)
                    .expect("to exist due previous try_insert");
                show.insert(season, episodes);
            }
        }
    }

    fn get_from_cache(
        &self,
        tmdb_show_id: usize,
        season: usize,
        episode: usize,
    ) -> Option<TmdbSeasonEpisode> {
        let episodes_cache = self.episodes_cache.lock().unwrap();
        let show = episodes_cache.get(&tmdb_show_id)?;
        let season = show.get(&season)?;
        season.get(episode - 1).cloned()
    }
}

impl From<TmdbSearchMovieResult> for MovieMetadata {
    fn from(val: TmdbSearchMovieResult) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        let backdrop = val
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).into());
        MovieMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            poster,
            backdrop,
            plot: Some(val.overview),
            release_date: Some(val.release_date),
            title: val.title,
        }
    }
}

impl From<TmdbSearchShowResult> for ShowMetadata {
    fn from(val: TmdbSearchShowResult) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        let backdrop = val
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).into());

        ShowMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            poster,
            backdrop,
            plot: Some(val.overview),
            release_date: Some(val.first_air_date),
            title: val.name,
            ..Default::default()
        }
    }
}

impl From<TmdbShowSeason> for SeasonMetadata {
    fn from(val: TmdbShowSeason) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        SeasonMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            release_date: Some(val.air_date),
            plot: Some(val.overview),
            episodes: val.episodes.into_iter().map(|e| e.into()).collect(),
            poster,
            number: val.season_number,
        }
    }
}

impl From<TmdbSeasonEpisode> for EpisodeMetadata {
    fn from(val: TmdbSeasonEpisode) -> Self {
        let poster = val
            .still_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        EpisodeMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            release_date: Some(val.air_date),
            number: val.episode_number,
            title: val.name,
            runtime: val.runtime.map(|t| Duration::from_mins(t as u64)),
            plot: Some(val.overview),
            season_number: val.season_number,
            poster,
        }
    }
}

#[axum::async_trait]
impl MovieMetadataProvider for TmdbApi {
    async fn movie(&self, metadata_id: &str) -> Result<MovieMetadata, AppError> {
        let movie = self.movie_details(metadata_id.parse()?).await?;
        Ok(movie.into())
    }

    fn provider_identifier(&self) -> &'static str {
        "tmdb"
    }
}

#[axum::async_trait]
impl ShowMetadataProvider for TmdbApi {
    async fn show(&self, metadata_show_id: &str) -> Result<ShowMetadata, AppError> {
        self.show_details(metadata_show_id.parse()?)
            .await
            .map(|r| r.into())
    }

    async fn season(
        &self,
        metadata_show_id: &str,
        season: usize,
    ) -> Result<SeasonMetadata, AppError> {
        let show_id = metadata_show_id.parse().expect("tmdb ids to be numbers");
        self.tv_show_season(show_id, season).await.map(|s| s.into())
    }

    async fn episode(
        &self,
        metadata_show_id: &str,
        season: usize,
        episode: usize,
    ) -> Result<EpisodeMetadata, AppError> {
        let show_id = metadata_show_id.parse().expect("tmdb ids to be numbers");
        self.tv_show_episode(show_id, season, episode)
            .await
            .map(|e| e.into())
    }

    fn provider_identifier(&self) -> &'static str {
        "tmdb"
    }
}

#[axum::async_trait]
impl DiscoverMetadataProvider for TmdbApi {
    async fn multi_search(&self, query: &str) -> Result<Vec<MetadataSearchResult>, AppError> {
        let content = self.search_multi(query).await?;
        Ok(content
            .results
            .into_iter()
            .filter_map(|x| x.try_into().ok())
            .collect())
    }

    async fn show_search(&self, query: &str) -> Result<Vec<ShowMetadata>, AppError> {
        let shows = self.search_tv_show(query).await?;
        Ok(shows.results.into_iter().map(|x| x.into()).collect())
    }

    async fn movie_search(&self, query: &str) -> Result<Vec<MovieMetadata>, AppError> {
        let content = self.search_movie(query).await?;
        Ok(content.results.into_iter().map(|x| x.into()).collect())
    }

    async fn external_ids(
        &self,
        content_id: &str,
        content_hint: ContentType,
    ) -> Result<Vec<ExternalIdMetadata>, AppError> {
        let id = content_id.parse()?;

        let ids = match content_hint {
            ContentType::Movie => self.movie_external_ids(id).await,
            ContentType::Show => self.show_external_ids(id).await,
        }?;
        let mut out = Vec::new();

        if let Some(tvdb_id) = ids.tvdb_id {
            out.push(ExternalIdMetadata {
                provider: MetadataProvider::Tvdb,
                id: tvdb_id.to_string(),
            });
        }
        if let Some(imdb_id) = ids.imdb_id {
            out.push(ExternalIdMetadata {
                provider: MetadataProvider::Imdb,
                id: imdb_id,
            });
        }

        Ok(out)
    }

    fn provider_identifier(&self) -> &'static str {
        "tmdb"
    }
}

impl From<TmdbMovieDetails> for MovieMetadata {
    fn from(val: TmdbMovieDetails) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        let backdrop = val
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).into());
        MovieMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            poster,
            backdrop,
            plot: Some(val.overview),
            release_date: Some(val.release_date),
            title: val.title,
        }
    }
}

impl From<TmdbShowDetails> for ShowMetadata {
    fn from(val: TmdbShowDetails) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        let backdrop = val
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).into());
        ShowMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            poster,
            backdrop,
            plot: Some(val.overview),
            release_date: val.first_air_date,
            title: val.name,
            seasons: Some((1..=val.number_of_seasons).collect()),
            episodes_amount: Some(val.number_of_episodes),
        }
    }
}

impl TryInto<MetadataSearchResult> for TmdbFindMultiResult {
    type Error = anyhow::Error;
    fn try_into(self) -> Result<MetadataSearchResult, Self::Error> {
        let title;
        let poster;
        let tmdb_id;
        let plot;
        let content_type;
        match self {
            Self::Movie(movie) => {
                title = movie.title;
                poster = movie
                    .poster_path
                    .map(|p| MetadataImage::new(TmdbImage::new(&p, PosterSizes::W342).url));
                tmdb_id = movie.id;
                plot = movie.overview;
                content_type = ContentType::Movie;
            }
            Self::Show(show) => {
                title = show.name;
                poster = show
                    .poster_path
                    .map(|p| MetadataImage::new(TmdbImage::new(&p, PosterSizes::W342).url));
                tmdb_id = show.id;
                plot = show.overview;
                content_type = ContentType::Show;
            }
            Self::Episode(_) => return Err(anyhow!("Episode is not implemented")),
            Self::Person(_) => return Err(anyhow!("Person is not implemented")),
            Self::Other {} => return Err(anyhow!("Other is not implemented")),
        };
        Ok(MetadataSearchResult {
            title,
            poster,
            plot: Some(plot),
            metadata_id: tmdb_id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            content_type,
        })
    }
}

fn parse_tmdb_date(input: &str) -> anyhow::Result<OffsetDateTime> {
    use time::format_description::parse_borrowed;
    let parsed = parse_borrowed::<2>("[year]-[month]-[day]").unwrap();
    let date = Date::parse(input, &parsed)?;
    Ok(OffsetDateTime::new_utc(date, time::Time::MIDNIGHT))
}

// Types

#[derive(Debug, Clone, Deserialize)]
pub struct TmdbFindByIdResult {
    pub movie_results: Vec<TmdbSearchMovieResult>,
    pub tv_results: Vec<TmdbSearchShowResult>,
}

// possible bug: media_type field is not checked. if semantics of different content types are the same
// wrong content type might be selected
// consider manual deserialize implementation
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum TmdbFindMultiResult {
    Movie(TmdbSearchMovieResult),
    Show(TmdbSearchShowResult),
    Person(TmdbPerson),
    Episode(TmdbSeasonEpisode),
    Other {},
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbExternalIds {
    pub id: Option<usize>,
    pub imdb_id: Option<String>,
    pub freebase_mid: Option<String>,
    pub freebase_id: Option<String>,
    pub tvdb_id: Option<usize>,
    pub tvrage_id: Option<usize>,
    pub wikidata_id: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbShowDetails {
    pub adult: bool,
    pub backdrop_path: Option<String>,
    pub first_air_date: Option<String>,
    pub genres: Option<Vec<TmdbGenre>>,
    pub id: usize,
    pub in_production: bool,
    pub languages: Option<Vec<String>>,
    pub last_air_date: Option<String>,
    pub last_episode_to_air: Option<TmdbEpisodeToAir>,
    pub name: String,
    pub next_episode_to_air: Option<TmdbEpisodeToAir>,
    pub number_of_episodes: usize,
    pub number_of_seasons: usize,
    pub origin_country: Option<Vec<String>>,
    pub original_language: Option<String>,
    pub original_name: String,
    pub overview: String,
    pub popularity: f64,
    pub poster_path: Option<String>,
    pub tagline: Option<String>,
    pub vote_average: f64,
    pub vote_count: usize,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbMovieDetails {
    pub adult: bool,
    pub backdrop_path: Option<String>,
    pub budget: Option<usize>,
    pub genres: Option<Vec<TmdbGenre>>,
    pub homepage: Option<String>,
    pub id: usize,
    pub imdb_id: Option<String>,
    pub original_language: Option<String>,
    pub original_title: Option<String>,
    pub overview: String,
    pub popularity: f64,
    pub poster_path: Option<String>,
    pub release_date: String,
    pub revenue: Option<isize>,
    pub runtime: Option<usize>,
    pub status: Option<String>,
    pub tagline: Option<String>,
    pub title: String,
    pub vote_average: f64,
    pub vote_count: usize,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbGenre {
    pub id: usize,
    pub name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbEpisodeToAir {
    pub id: usize,
    pub name: String,
    pub overview: String,
    pub vote_average: f64,
    pub vote_count: usize,
    pub air_date: String,
    pub episode_number: usize,
    pub episode_type: Option<String>,
    pub runtime: Option<usize>,
    pub season_number: usize,
    pub show_id: usize,
    pub still_path: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbCrew {
    pub id: Option<usize>,
    pub credit_id: Option<String>,
    pub name: Option<String>,
    pub adult: Option<bool>,
    pub gender: Option<usize>,
    pub known_for_department: Option<String>,
    pub department: Option<String>,
    pub original_name: Option<String>,
    pub popularity: Option<f64>,
    pub job: Option<String>,
    pub profile_path: Option<String>,
}
#[derive(Deserialize, Debug, Clone)]
pub struct TmdbGuestStars {
    pub adult: Option<bool>,
    pub gender: Option<usize>,
    pub known_for_department: Option<String>,
    pub original_name: Option<String>,
    pub popularity: Option<f64>,
    pub id: Option<usize>,
    pub name: Option<String>,
    pub credit_id: Option<String>,
    pub character: Option<String>,
    pub order: Option<usize>,
    pub profile_path: Option<String>,
}
#[derive(Deserialize, Debug, Clone)]
pub struct TmdbSeasonEpisode {
    pub air_date: String,
    pub episode_number: usize,
    pub crew: Vec<Option<TmdbCrew>>,
    pub guest_stars: Vec<Option<TmdbGuestStars>>,
    pub name: String,
    pub overview: String,
    pub id: usize,
    pub production_code: Option<String>,
    /// Duration in minutes
    pub runtime: Option<usize>,
    pub season_number: usize,
    pub still_path: Option<String>,
    pub vote_average: f64,
    pub vote_count: usize,
}
#[derive(Deserialize, Debug, Clone)]
pub struct TmdbShowSeason {
    pub _id: String,
    pub air_date: String,
    pub episodes: Vec<TmdbSeasonEpisode>,
    pub name: String,
    pub overview: String,
    pub id: usize,
    pub poster_path: Option<String>,
    pub season_number: usize,
    pub vote_average: f64,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbSearchShowResult {
    pub poster_path: Option<String>,
    pub popularity: f64,
    pub id: usize,
    pub backdrop_path: Option<String>,
    pub vote_average: f64,
    pub overview: String,
    pub first_air_date: String,
    pub origin_country: Vec<String>,
    pub genre_ids: Vec<usize>,
    pub original_language: String,
    pub vote_count: usize,
    pub name: String,
    pub original_name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbSearch<T> {
    pub page: usize,
    pub results: Vec<T>,
    pub total_results: usize,
    pub total_pages: usize,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbSearchMovieResult {
    pub backdrop_path: Option<String>,
    pub poster_path: Option<String>,
    pub popularity: f64,
    pub id: usize,
    pub vote_average: f64,
    pub overview: String,
    pub release_date: String,
    pub origin_country: Option<Vec<String>>,
    pub genre_ids: Vec<usize>,
    pub original_language: String,
    pub vote_count: usize,
    pub title: String,
    pub original_title: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbPerson {
    // todo
}
