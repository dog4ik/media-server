use std::time::Duration;

use anyhow::anyhow;
use reqwest::{
    Client, Method, Request, Url,
    header::{ACCEPT_ENCODING, AUTHORIZATION, HeaderMap, HeaderValue},
};
use serde::Deserialize;

use crate::app_state::AppError;
use crate::metadata::{PersonMetadata, ProviderIdentifier, RoleMetadata};

use super::{
    ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, LocaleMetadata,
    MetadataProvider, MetadataSearchResult, MovieMetadata, MovieMetadataProvider, SeasonMetadata,
    ShowMetadata, ShowMetadataProvider, request_client::LimitedRequestClient,
};
use super::{FetchParams, Language, provod_agent};

#[derive(Debug)]
pub struct TmdbApi {
    pub base_url: Url,
    client: LimitedRequestClient,
}

#[derive(Debug, Clone, Default)]
#[allow(unused)]
enum PosterSizes {
    W92,
    W154,
    W185,
    W342,
    W500,
    #[default]
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
struct TmdbImage {
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

impl std::fmt::Display for TmdbImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.url)
    }
}

fn append_language(url: &mut Url, language: Language) {
    url.query_pairs_mut()
        .append_pair("language", &language.to_string());
}

impl TmdbApi {
    const API_URL: &'static str = "http://api.themoviedb.org/3";
    const RATE_LIMIT: usize = 50;

    /// Create new instance of TMDB Api client.
    ///
    /// If provided api key, requests will go directly to tmdb.
    /// Otherwise Provod proxy will be used.
    pub fn new(api_key: Option<String>) -> anyhow::Result<Self> {
        let mut headers = HeaderMap::with_capacity(2);
        // If we don't have token use provod agent
        let (client, base_url) = match api_key {
            Some(api_key) => {
                tracing::info!("Using personal TMDB api token");
                headers.insert(
                    ACCEPT_ENCODING,
                    HeaderValue::from_str("compress").expect("ascii"),
                );
                headers.insert(
                    AUTHORIZATION,
                    HeaderValue::from_str(&format!("Bearer {api_key}")).expect("ascii"),
                );
                (
                    Client::builder()
                        .default_headers(headers)
                        .build()
                        .expect("build to succeed"),
                    Url::parse(Self::API_URL).expect("url to parse"),
                )
            }
            None => provod_agent::new_client("tmdb")?,
        };

        let limited_client =
            LimitedRequestClient::new(client, Self::RATE_LIMIT, std::time::Duration::from_secs(1));
        Ok(Self {
            client: limited_client,
            base_url,
        })
    }

    pub async fn trending_shows(
        &self,
        language: Language,
    ) -> Result<TmdbSearch<TmdbSearchShowResult>, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("trending")
            .push("tv")
            .push("day");
        url.query_pairs_mut()
            .append_pair("language", &language.to_string());
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    pub async fn trending_movies(
        &self,
        language: Language,
    ) -> Result<TmdbSearch<TmdbSearchMovieResult>, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("trending")
            .push("movie")
            .push("day");
        url.query_pairs_mut()
            .append_pair("language", &language.to_string());
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    pub async fn search_movie(
        &self,
        query: &str,
        lang: Language,
    ) -> Result<TmdbSearch<TmdbSearchMovieResult>, AppError> {
        let query = [("query", query)];
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("search")
            .push("movie");
        url.query_pairs_mut()
            .extend_pairs(query)
            .append_pair("language", &lang.to_string());
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    pub async fn search_tv_show(
        &self,
        query: &str,
        language: Language,
    ) -> Result<TmdbSearch<TmdbSearchShowResult>, AppError> {
        let query = [("query", query)];
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("search").push("tv");
        url.query_pairs_mut()
            .extend_pairs(query)
            .append_pair("language", &language.to_string());
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    async fn search_multi(
        &self,
        query: &str,
        lang: Language,
    ) -> Result<TmdbSearch<TmdbFindMultiResult>, AppError> {
        let query = [("query", query)];
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("search")
            .push("multi");
        url.query_pairs_mut()
            .extend_pairs(query)
            .append_pair("language", &lang.to_string());
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    async fn tv_show_season(
        &self,
        tmdb_show_id: usize,
        season: usize,
        fetch_params: FetchParams,
    ) -> Result<TmdbShowSeason, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("tv")
            .push(&tmdb_show_id.to_string())
            .push("season")
            .push(&season.to_string());
        url.query_pairs_mut()
            .append_pair("append_to_response", "credits");
        append_language(&mut url, fetch_params.lang);
        let req = Request::new(Method::GET, url);
        let response: TmdbShowSeason = self.client.request(req).await?;

        Ok(response)
    }

    async fn tv_show_episode(
        &self,
        tmdb_show_id: usize,
        season: usize,
        episode: usize,
        params: FetchParams,
    ) -> Result<TmdbSeasonEpisode, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("tv")
            .push(&tmdb_show_id.to_string())
            .push("season")
            .push(&season.to_string())
            .push("episode")
            .push(&episode.to_string());
        url.query_pairs_mut()
            .append_pair("append_to_response", "credits")
            .append_pair("language", &params.lang.to_string());
        let req = Request::new(Method::GET, url);
        self.client.request(req).await
    }

    async fn find_by_imdb_id(&self, imdb_id: &str) -> Result<TmdbFindByIdResult, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut().unwrap().push("find").push(imdb_id);
        url.query_pairs_mut()
            .append_pair("external_source", "imdb_id");
        let req = Request::new(Method::GET, url);
        let res = self.client.request(req).await?;
        Ok(res)
    }

    async fn movie_external_ids(&self, id: usize) -> Result<TmdbExternalIds, AppError> {
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

    async fn show_external_ids(&self, id: usize) -> Result<TmdbExternalIds, AppError> {
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

    async fn movie_details(
        &self,
        movie_id: usize,
        lang: Language,
    ) -> Result<TmdbMovieDetails, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("movie")
            .push(&movie_id.to_string());
        url.query_pairs_mut()
            .append_pair("language", &lang.to_string())
            .append_pair("append_to_response", "credits,external_ids");
        let req = Request::new(Method::GET, url);
        let res = self.client.request(req).await?;
        Ok(res)
    }

    async fn show_details(
        &self,
        show_id: usize,
        lang: Language,
    ) -> Result<TmdbShowDetails, AppError> {
        let mut url = self.base_url.clone();
        url.path_segments_mut()
            .unwrap()
            .push("tv")
            .push(&show_id.to_string());
        url.query_pairs_mut()
            .append_pair("language", &lang.to_string())
            .append_pair("append_to_response", "credits,external_ids");
        let req = Request::new(Method::GET, url);
        let res = self.client.request(req).await?;
        Ok(res)
    }
}

impl From<TmdbSearchMovieResult> for MovieMetadata {
    fn from(val: TmdbSearchMovieResult) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::default()).to_string());
        let backdrop = val
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).to_string());
        let original_title = val.original_title;
        let original_language = val.original_language;
        MovieMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            poster,
            backdrop,
            plot: val.overview,
            release_date: val.release_date,
            runtime: None,
            title: val.title,
            locale_metadata: Some(LocaleMetadata {
                original_title,
                original_language,
            }),
            cast: None,
            genres: None,
            external_ids: None,
        }
    }
}

impl From<TmdbSearchShowResult> for ShowMetadata {
    fn from(val: TmdbSearchShowResult) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::default()).to_string());
        let backdrop = val
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).to_string());
        let original_title = val.original_name;
        let original_language = val.original_language;
        ShowMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            poster,
            backdrop,
            plot: val.overview,
            release_date: val.first_air_date,
            title: val.name,
            locale_metadata: Some(LocaleMetadata {
                original_title,
                original_language,
            }),
            seasons: None,
            episodes_amount: None,
            cast: None,
            genres: None,
            external_ids: None,
        }
    }
}

impl From<TmdbShowSeason> for SeasonMetadata {
    fn from(val: TmdbShowSeason) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::default()).to_string());
        SeasonMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            release_date: val.air_date,
            plot: val.overview,
            episodes: val.episodes.into_iter().map(Into::into).collect(),
            poster,
            number: val.season_number,
            title: Some(val.name),
            cast: val
                .credits
                .map(|credits| credits.cast.into_iter().map(Into::into).collect()),
        }
    }
}

impl From<TmdbSeasonEpisode> for EpisodeMetadata {
    fn from(val: TmdbSeasonEpisode) -> Self {
        let poster = val
            .still_path
            .map(|p| TmdbImage::new(&p, PosterSizes::default()).to_string());
        EpisodeMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            release_date: val.air_date,
            number: val.episode_number,
            title: val.name,
            runtime: val
                .runtime
                .map(|t| Duration::from_secs(t as u64 * 60))
                .map(Into::into),
            plot: val.overview,
            season_number: val.season_number,
            poster,
            cast: val
                .credits
                .map(|credits| credits.cast.into_iter().map(Into::into).collect()),
        }
    }
}

impl ProviderIdentifier for TmdbApi {
    fn provider_identifier(&self) -> MetadataProvider {
        MetadataProvider::Tmdb
    }
}

#[async_trait::async_trait]
impl MovieMetadataProvider for TmdbApi {
    async fn movie(
        &self,
        metadata_id: &str,
        params: FetchParams,
    ) -> Result<MovieMetadata, AppError> {
        let movie = self
            .movie_details(metadata_id.parse()?, params.lang)
            .await?;
        Ok(movie.into())
    }

    async fn movie_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<MovieMetadata>, AppError> {
        let content = self.search_movie(query, fetch_params.lang).await?;
        Ok(content.results.into_iter().map(Into::into).collect())
    }
}

#[async_trait::async_trait]
impl ShowMetadataProvider for TmdbApi {
    async fn show(
        &self,
        metadata_show_id: &str,
        fetch_params: FetchParams,
    ) -> Result<ShowMetadata, AppError> {
        self.show_details(metadata_show_id.parse()?, fetch_params.lang)
            .await
            .map(Into::into)
    }

    async fn season(
        &self,
        metadata_show_id: &str,
        season: usize,
        fetch_params: FetchParams,
    ) -> Result<SeasonMetadata, AppError> {
        let show_id = metadata_show_id.parse().expect("tmdb ids to be numbers");
        self.tv_show_season(show_id, season, fetch_params)
            .await
            .map(Into::into)
    }

    async fn episode(
        &self,
        metadata_show_id: &str,
        season: usize,
        episode: usize,
        fetch_params: FetchParams,
    ) -> Result<EpisodeMetadata, AppError> {
        let show_id = metadata_show_id.parse().expect("tmdb ids to be numbers");
        self.tv_show_episode(show_id, season, episode, fetch_params)
            .await
            .map(Into::into)
    }

    async fn show_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<ShowMetadata>, AppError> {
        let shows = self.search_tv_show(query, fetch_params.lang).await?;
        Ok(shows.results.into_iter().map(Into::into).collect())
    }
}

#[async_trait::async_trait]
impl DiscoverMetadataProvider for TmdbApi {
    async fn multi_search(
        &self,
        query: &str,
        fetch_params: FetchParams,
    ) -> Result<Vec<MetadataSearchResult>, AppError> {
        let content = self.search_multi(query, fetch_params.lang).await?;
        Ok(content
            .results
            .into_iter()
            .filter_map(|x| x.try_into().ok())
            .collect())
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
}

impl From<TmdbMovieDetails> for MovieMetadata {
    fn from(val: TmdbMovieDetails) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::default()).to_string());
        let backdrop = val
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).to_string());
        let genres = val.genres.map(|gs| {
            gs.into_iter()
                .filter_map(|g| super::Genre::try_from(g).ok())
                .collect::<Vec<_>>()
        });
        MovieMetadata {
            metadata_id: val.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            poster,
            backdrop,
            plot: Some(val.overview),
            release_date: val.release_date,
            runtime: val
                .runtime
                .map(|t| Duration::from_secs(t as u64 * 60))
                .map(Into::into),
            title: val.title,
            locale_metadata: Some(LocaleMetadata {
                original_title: val.original_title,
                original_language: val.original_language,
            }),
            cast: val
                .credits
                .map(|credits| credits.cast.into_iter().map(Into::into).collect()),
            genres,
            external_ids: val.external_ids.map(Into::into),
        }
    }
}

impl From<TmdbShowDetails> for ShowMetadata {
    fn from(val: TmdbShowDetails) -> Self {
        let poster = val
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::default()).to_string());
        let backdrop = val
            .backdrop_path
            .map(|b| TmdbImage::new(&b, PosterSizes::Original).to_string());
        let genres = val.genres.map(|gs| {
            gs.into_iter()
                .filter_map(|g| super::Genre::try_from(g).ok())
                .collect::<Vec<_>>()
        });
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
            locale_metadata: Some(LocaleMetadata {
                original_title: val.original_name,
                original_language: val.original_language,
            }),
            cast: val
                .credits
                .map(|v| v.cast.into_iter().map(Into::into).collect()),
            genres,
            external_ids: val.external_ids.map(Into::into),
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
        let original_title;
        let original_language;
        match self {
            Self::Movie(movie) => {
                title = movie.title;
                poster = movie
                    .poster_path
                    .map(|p| TmdbImage::new(&p, PosterSizes::default()).to_string());
                tmdb_id = movie.id;
                plot = movie.overview;
                content_type = ContentType::Movie;
                original_title = movie.original_title;
                original_language = movie.original_language;
            }
            Self::Show(show) => {
                title = show.name;
                poster = show
                    .poster_path
                    .map(|p| TmdbImage::new(&p, PosterSizes::default()).to_string());
                tmdb_id = show.id;
                plot = show.overview;
                content_type = ContentType::Show;
                original_title = show.original_name;
                original_language = show.original_language;
            }
            Self::Other {} => return Err(anyhow!("Other is not implemented")),
        };
        Ok(MetadataSearchResult {
            title,
            poster,
            plot,
            metadata_id: tmdb_id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            content_type,
            locale_metadata: Some(LocaleMetadata {
                original_title,
                original_language,
            }),
        })
    }
}

impl From<TmdbCast> for PersonMetadata {
    fn from(value: TmdbCast) -> Self {
        let poster = value
            .profile_path
            .map(|p| TmdbImage::new(&p, Default::default()).to_string());
        Self {
            metadata_id: value.id.to_string(),
            metadata_provider: MetadataProvider::Tmdb,
            person_poster: poster,
            name: value.name,
            imdb_id: None,
            role: value.character.map(|character| RoleMetadata {
                character,
                poster: None,
            }),
        }
    }
}

impl From<TmdbExternalIds> for Vec<ExternalIdMetadata> {
    fn from(TmdbExternalIds { imdb_id, .. }: TmdbExternalIds) -> Self {
        imdb_id
            .map(|id| {
                vec![ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id,
                }]
            })
            .unwrap_or_default()
    }
}

// Types

#[derive(Debug, Clone, Deserialize)]
#[allow(unused)]
struct TmdbFindByIdResult {
    movie_results: Vec<TmdbSearchMovieResult>,
    tv_results: Vec<TmdbSearchShowResult>,
}

// possible bug: media_type field is not checked. if semantics of different content types are the same
// wrong content type might be selected
// consider manual deserialize implementation
#[derive(Deserialize, Debug, Clone)]
#[serde(untagged)]
enum TmdbFindMultiResult {
    Movie(TmdbSearchMovieResult),
    Show(TmdbSearchShowResult),
    // Episode(TmdbSeasonEpisode),
    Other {},
}

#[derive(Deserialize, Debug, Clone)]
struct TmdbExternalIds {
    // id: Option<usize>,
    imdb_id: Option<String>,
    // freebase_mid: Option<String>,
    // freebase_id: Option<String>,
    tvdb_id: Option<usize>,
    // tvrage_id: Option<usize>,
    // wikidata_id: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct TmdbShowDetails {
    // adult: bool,
    backdrop_path: Option<String>,
    first_air_date: Option<String>,
    genres: Option<Vec<TmdbGenre>>,
    id: usize,
    // last_air_date: Option<String>,
    name: String,
    number_of_episodes: usize,
    number_of_seasons: usize,
    original_language: String,
    original_name: String,
    overview: String,
    poster_path: Option<String>,
    credits: Option<TmdbCredits>,
    external_ids: Option<TmdbExternalIds>,
}

#[derive(Deserialize, Debug, Clone)]
struct TmdbMovieDetails {
    backdrop_path: Option<String>,
    genres: Option<Vec<TmdbGenre>>,
    id: usize,
    // imdb_id: Option<String>,
    original_language: String,
    original_title: String,
    overview: String,
    poster_path: Option<String>,
    release_date: Option<String>,
    runtime: Option<usize>,
    title: String,
    credits: Option<TmdbCredits>,
    external_ids: Option<TmdbExternalIds>,
}

#[derive(Deserialize, Debug, Clone)]
struct TmdbCredits {
    cast: Vec<TmdbCast>,
}

#[derive(Deserialize, Debug, Clone)]
struct TmdbCast {
    id: usize,
    name: String,
    // original_name: Option<String>,
    profile_path: Option<String>,
    character: Option<String>,
    // order: usize,
}

#[derive(Deserialize, Debug, Clone)]
struct TmdbGenre {
    id: usize,
    // name: String,
}

impl TryFrom<TmdbGenre> for super::Genre {
    type Error = anyhow::Error;
    fn try_from(g: TmdbGenre) -> Result<Self, Self::Error> {
        match g.id {
            // TV genres
            10759 => Ok(super::Genre::ActionAdventure),
            10762 => Ok(super::Genre::Kids),
            10763 => Ok(super::Genre::News),
            10764 => Ok(super::Genre::Reality),
            10765 => Ok(super::Genre::SciFiFantasy),
            10766 => Ok(super::Genre::Soap),
            10767 => Ok(super::Genre::Talk),
            10768 => Ok(super::Genre::War),
            // Movie genres
            28 => Ok(super::Genre::Action),
            12 => Ok(super::Genre::Adventure),
            16 => Ok(super::Genre::Animation),
            35 => Ok(super::Genre::Comedy),
            80 => Ok(super::Genre::Crime),
            99 => Ok(super::Genre::Documentary),
            18 => Ok(super::Genre::Drama),
            10751 => Ok(super::Genre::Family),
            14 => Ok(super::Genre::Fantasy),
            36 => Ok(super::Genre::History),
            27 => Ok(super::Genre::Horror),
            10402 => Ok(super::Genre::Music),
            9648 => Ok(super::Genre::Mystery),
            10749 => Ok(super::Genre::Romance),
            878 => Ok(super::Genre::ScienceFiction),
            53 => Ok(super::Genre::Thriller),
            10752 => Ok(super::Genre::War),
            37 => Ok(super::Genre::Western),
            _ => Err(anyhow::anyhow!("Unknown TMDB genre id: {}", g.id)),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[allow(unused)]
struct TmdbEpisodeToAir {
    id: usize,
    name: String,
    overview: String,
    air_date: Option<String>,
    episode_number: usize,
    runtime: Option<usize>,
    season_number: usize,
    show_id: usize,
    still_path: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[allow(unused)]
struct TmdbCrew {
    id: Option<usize>,
    credit_id: Option<String>,
    name: Option<String>,
    adult: Option<bool>,
    gender: Option<usize>,
    known_for_department: Option<String>,
    department: Option<String>,
    original_name: Option<String>,
    popularity: Option<f64>,
    job: Option<String>,
    profile_path: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[allow(unused)]
struct TmdbGuestStars {
    adult: Option<bool>,
    gender: Option<usize>,
    known_for_department: Option<String>,
    original_name: Option<String>,
    popularity: Option<f64>,
    id: Option<usize>,
    name: Option<String>,
    credit_id: Option<String>,
    character: Option<String>,
    order: Option<usize>,
    profile_path: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
struct TmdbSeasonEpisode {
    air_date: Option<String>,
    episode_number: usize,
    // crew: Vec<Option<TmdbCrew>>,
    // guest_stars: Vec<Option<TmdbGuestStars>>,
    name: String,
    overview: Option<String>,
    id: usize,
    /// Duration in minutes
    runtime: Option<usize>,
    season_number: usize,
    still_path: Option<String>,
    credits: Option<TmdbCredits>,
}
#[derive(Deserialize, Debug, Clone)]
struct TmdbShowSeason {
    air_date: Option<String>,
    episodes: Vec<TmdbSeasonEpisode>,
    name: String,
    overview: Option<String>,
    id: usize,
    poster_path: Option<String>,
    season_number: usize,
    credits: Option<TmdbCredits>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbSearchShowResult {
    poster_path: Option<String>,
    id: usize,
    backdrop_path: Option<String>,
    overview: Option<String>,
    first_air_date: Option<String>,
    name: String,
    #[serde(alias = "original_title")]
    original_name: String,
    original_language: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbSearch<T> {
    pub results: Vec<T>,
    // page: usize,
    // total_results: usize,
    // total_pages: usize,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TmdbSearchMovieResult {
    backdrop_path: Option<String>,
    poster_path: Option<String>,
    id: usize,
    overview: Option<String>,
    release_date: Option<String>,
    title: String,
    #[serde(alias = "original_name")]
    original_title: String,
    original_language: String,
}
