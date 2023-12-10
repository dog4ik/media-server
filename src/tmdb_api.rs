use std::{collections::HashMap, time::Duration};

use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT_ENCODING},
    Client, Method, Request, Response, Url,
};
use serde::Deserialize;
use tokio::sync::{mpsc, oneshot, Mutex};
use tower::{Service, ServiceExt};

use crate::metadata_provider::{
    EpisodeMetadata, MetadataImage, MovieMetadata, SeasonMetadata, ShowMetadata,
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
#[derive(Debug, Clone)]
pub struct LimitedRequestClient {
    request_tx: mpsc::Sender<(Request, oneshot::Sender<anyhow::Result<Response>>)>,
}

impl LimitedRequestClient {
    pub fn new(client: Client, limit_number: u64, limit_duration: Duration) -> Self {
        let (tx, mut rx) =
            mpsc::channel::<(Request, oneshot::Sender<anyhow::Result<Response>>)>(100);
        tokio::spawn(async move {
            let service = tower::ServiceBuilder::new()
                .buffer(100)
                .rate_limit(limit_number, limit_duration)
                .service(client.clone());
            while let Some((req, resp_tx)) = rx.recv().await {
                let mut inner_service = service.clone();
                tokio::spawn(async move {
                    let resp = match inner_service.ready().await {
                        Ok(srv) => match srv.call(req).await {
                            Ok(r) => Ok(r),
                            Err(e) => Err(anyhow::anyhow!("Service call request failed: {}", e)),
                        },
                        Err(e) => Err(anyhow::anyhow!("Service ready failed: {}", e)),
                    };

                    if let Err(_) = resp_tx.send(resp) {
                        tracing::error!("Send resp to resp_tx failed: channel closed")
                    }
                });
            }
        });
        Self { request_tx: tx }
    }

    pub async fn request(&self, req: Request) -> anyhow::Result<Response> {
        let (tx, rx) = oneshot::channel::<anyhow::Result<Response>>();
        self.request_tx.clone().send((req, tx)).await?;
        rx.await?
    }
}

#[tokio::test]
async fn rate_limit() {
    use axum::routing::post;
    use axum::{Json, Router};
    use serde::Serialize;
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

    async fn root(count: Json<Count>) -> Json<Count> {
        count
    }

    let server_handle = tokio::spawn(async move {
        let app = Router::new().route("/", post(root));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:32402")
            .await
            .unwrap();
        axum::serve(listener, app).await.unwrap();
    });

    let reqwest = Client::new();
    let client = LimitedRequestClient::new(reqwest.clone(), 50, Duration::from_secs(1));
    let mut handles = JoinSet::new();
    let amount = 150;
    for i in 0..amount {
        let client = client.clone();
        let count = Count::new(i);
        let req = reqwest
            .post("http://127.0.0.1:32402/")
            .json(&count)
            .build()
            .unwrap();
        handles.spawn(async move {
            let count = client
                .request(req)
                .await
                .unwrap()
                .json::<Count>()
                .await
                .unwrap();
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

impl TmdbApi {
    const API_URL: &'static str = "http://api.themoviedb.org/3";
    const RATE_LIMIT: u64 = 50;
    pub fn new(api_key: String) -> Self {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_str("compress").unwrap());

        let params = [("api_key", api_key.clone())];
        let client = Client::builder()
            .default_headers(headers)
            .build()
            .expect("build to succeed");
        let limited_client =
            LimitedRequestClient::new(client, Self::RATE_LIMIT, Duration::from_secs(1));
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
        let res = self.client.request(req).await?;
        Ok(res.json().await?)
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
        let res = self.client.request(req).await?;
        Ok(res.json().await?)
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
        let response: TmdbShowSeason = self.client.request(req).await?.json().await?;

        self.update_cache(tmdb_show_id, season, response.episodes.clone())
            .await;

        Ok(response)
    }

    pub async fn tv_show_episode(
        &self,
        tmdb_show_id: usize,
        season: usize,
        episode: usize,
    ) -> anyhow::Result<TmdbSeasonEpisode> {
        if let Some(cache_episode) = self.get_from_cache(tmdb_show_id, season, episode).await {
            return Ok(cache_episode);
        } else {
            let response = self.tv_show_season(tmdb_show_id, season).await?;
            self.update_cache(tmdb_show_id, season, response.episodes)
                .await;
            Ok(self
                .get_from_cache(tmdb_show_id, season, episode)
                .await
                .expect("cache to contain episode"))
        }
    }

    async fn update_cache(
        &self,
        tmdb_show_id: usize,
        season: usize,
        episodes: Vec<TmdbSeasonEpisode>,
    ) {
        let mut episodes_cache = self.episodes_cache.lock().await;
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

    async fn get_from_cache(
        &self,
        tmdb_show_id: usize,
        season: usize,
        episode: usize,
    ) -> Option<TmdbSeasonEpisode> {
        let episodes_cache = self.episodes_cache.lock().await;
        let show = episodes_cache.get(&tmdb_show_id)?;
        let season = show.get(&season)?;
        season.get(episode - 1).cloned()
    }
}

impl Into<MovieMetadata> for TmdbSearchMovieResult {
    fn into(self) -> MovieMetadata {
        let poster = self
            .poster_path
            .map(|p| TmdbImage::new(&p, PosterSizes::Original).into());
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
            .map(|p| TmdbImage::new(&p, PosterSizes::Original).into());
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
            .map(|p| TmdbImage::new(&p, PosterSizes::Original).into());
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
        let poster = TmdbImage::new(&self.still_path, PosterSizes::Original);
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
