use std::collections::HashMap;
use std::sync::Mutex;

use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT_ENCODING},
    Client, Method, Request, Url,
};
use serde::Deserialize;

use crate::library::movie::MovieFile;
use crate::library::show::ShowFile;

use super::{
    EpisodeMetadata, LimitedRequestClient, MetadataImage, MovieMetadata, MovieMetadataProvider,
    SeasonMetadata, ShowMetadata, ShowMetadataProvider,
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
}

impl Into<MetadataImage> for TmdbImage {
    fn into(self) -> MetadataImage {
        MetadataImage::new(self.url)
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
    ) -> Result<TmdbSearch<TmdbSearchMovieResult>, anyhow::Error> {
        let query = [("query", query)];
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("search").push("tv");
        url.query_pairs_mut().extend_pairs(query);
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    pub async fn search_tv_show(
        &self,
        query: &str,
    ) -> Result<TmdbSearch<TmdbSearchShowResult>, anyhow::Error> {
        let query = [("query", query)];
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("search").push("tv");
        url.query_pairs_mut().extend_pairs(query);
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    pub async fn tv_show_season(
        &self,
        tmdb_show_id: usize,
        season: usize,
    ) -> anyhow::Result<TmdbShowSeason> {
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
    ) -> anyhow::Result<TmdbSeasonEpisode> {
        //FIX: case when episode cant be found by metadata provider while we have its siblings in
        //cache
        if let Some(cache_episode) = self.get_from_cache(tmdb_show_id, season, episode) {
            tracing::debug!(
                "Reused cache entry for {} season: {} episode: {}",
                tmdb_show_id,
                season,
                episode
            );
            return Ok(cache_episode);
        } else {
            let response = self.tv_show_season(tmdb_show_id, season).await?;
            self.update_cache(tmdb_show_id, season, response.episodes);
            self.get_from_cache(tmdb_show_id, season, episode)
                .ok_or(anyhow::anyhow!(
                    "episode does not exist in metadata provider"
                ))
        }
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

impl Into<MovieMetadata> for TmdbSearchMovieResult {
    fn into(self) -> MovieMetadata {
        let poster = self
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        let backdrop = self
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).into());
        MovieMetadata {
            metadata_id: Some(self.id.to_string()),
            metadata_provider: "tmdb",
            poster,
            backdrop,
            rating: self.vote_average,
            plot: self.overview,
            release_date: self.first_air_date,
            language: self.original_language,
            title: self.original_title,
        }
    }
}

impl Into<ShowMetadata> for TmdbSearchShowResult {
    fn into(self) -> ShowMetadata {
        let poster = self
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        let backdrop = self
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).into());

        ShowMetadata {
            metadata_id: Some(self.id.to_string()),
            metadata_provider: "tmdb",
            poster,
            backdrop,
            rating: self.vote_average,
            plot: self.overview,
            release_date: self.first_air_date,
            language: self.original_language,
            title: self.name,
        }
    }
}

impl Into<SeasonMetadata> for TmdbShowSeason {
    fn into(self) -> SeasonMetadata {
        let poster = self
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::W342).into());
        SeasonMetadata {
            metadata_id: Some(self.id.to_string()),
            metadata_provider: "tmdb",
            release_date: self.air_date,
            episodes_amount: self.episodes.len(),
            title: self.name,
            plot: self.overview,
            poster,
            number: self.season_number,
            rating: self.vote_average,
        }
    }
}

impl Into<EpisodeMetadata> for TmdbSeasonEpisode {
    fn into(self) -> EpisodeMetadata {
        let poster = TmdbImage::new(&self.still_path, PosterSizes::W342);
        EpisodeMetadata {
            metadata_id: Some(self.id.to_string()),
            metadata_provider: "tmdb",
            release_date: self.air_date,
            number: self.episode_number,
            title: self.name,
            plot: self.overview,
            season_number: self.season_number,
            poster: poster.into(),
            rating: self.vote_average,
        }
    }
}

impl MovieMetadataProvider for TmdbApi {
    async fn movie(&self, movie: &MovieFile) -> Result<MovieMetadata, anyhow::Error> {
        let movies = self.search_movie(&movie.local_title).await?;
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
    async fn show(&self, show: &ShowFile) -> Result<ShowMetadata, anyhow::Error> {
        let contains = |parts: &Vec<&str>, name: &str| parts.iter().any(|p| name.contains(p));
        let shows = self.search_tv_show(&show.local_title).await?;
        for result in shows.results.iter().take(5) {
            let name = result.original_name.to_lowercase();
            let name_parts: Vec<&str> = name.split_whitespace().collect();
            // basic check
            if contains(&name_parts, &show.local_title) {
                return Ok(result.clone().into());
            }
            tracing::debug!(
                "Show name ({}) does not contain local name ({}). Doing metadata check",
                name,
                show.local_title
            );

            // metadata title check
            let metadata_title = show.source.metadata_title().to_lowercase();
            if contains(&name_parts, &metadata_title) {
                return Ok(result.clone().into());
            }

            tracing::debug!(
                "Show name does not contain file metadata title ({}). Doing duration check",
                show.local_title
            );

            // duration check
            let duration = show.source.origin.duration();
            let duration_match = self
                .tv_show_episode(result.id, show.season.into(), show.episode.into())
                .await
                .map_or(false, |e| {
                    let threshold = time::Duration::minutes(2);
                    let local_duration: time::Duration = duration.try_into().unwrap();
                    let tmdb_duration = time::Duration::minutes(e.runtime as i64);
                    let difference = (local_duration - tmdb_duration).abs();
                    difference <= threshold
                });
            if duration_match {
                return Ok(result.clone().into());
            }
        }
        tracing::warn!("Failed to verify match for {}", show.local_title);
        Err(anyhow::anyhow!("failed to find show"))
    }

    async fn season(
        &self,
        metadata_show_id: &str,
        season: usize,
    ) -> Result<SeasonMetadata, anyhow::Error> {
        let show_id = metadata_show_id.parse().expect("tmdb ids to be numbers");
        self.tv_show_season(show_id, season).await.map(|s| s.into())
    }

    async fn episode(
        &self,
        metadata_show_id: &str,
        season: usize,
        episode: usize,
    ) -> Result<EpisodeMetadata, anyhow::Error> {
        let show_id = metadata_show_id.parse().expect("tmdb ids to be numbers");
        self.tv_show_episode(show_id, season, episode)
            .await
            .map(|e| e.into())
    }

    fn provider_identifier(&self) -> &'static str {
        "tmdb"
    }
}

// Types

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
    pub runtime: usize,
    pub season_number: usize,
    pub still_path: String,
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
    pub first_air_date: String,
    pub origin_country: Vec<String>,
    pub genre_ids: Vec<usize>,
    pub original_language: String,
    pub vote_count: usize,
    pub name: String,
    pub original_title: String,
}
