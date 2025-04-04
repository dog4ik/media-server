use std::{sync::Mutex, time::Duration};

use anyhow::Context;
use serde::{Serialize, ser::SerializeStruct};

use crate::{
    app_state::AppError,
    config,
    db::Db,
    torrent_index::{
        Torrent, TorrentIndex, TorrentIndexIdentifier, rutracker::ProvodRuTrackerAdapter,
        tpb::TpbApi,
    },
};

use super::{
    ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, FetchParams,
    MetadataProvider, MetadataSearchResult, MovieMetadata, MovieMetadataProvider, SeasonMetadata,
    ShowMetadata, ShowMetadataProvider, tmdb_api::TmdbApi, tvdb_api::TvdbApi,
};

pub struct MetadataProvidersStack {
    pub tmdb: Option<&'static TmdbApi>,
    pub tvdb: Option<&'static TvdbApi>,
    pub local: &'static Db,
    pub tpb: Option<&'static TpbApi>,
    pub rutracker: Option<&'static ProvodRuTrackerAdapter>,
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
    pub fn new(db: &'static Db) -> Self {
        Self {
            local: db,
            tvdb: None,
            tmdb: None,
            tpb: None,
            rutracker: None,
            discover_providers_stack: Mutex::new(Vec::new()),
            movie_providers_stack: Mutex::new(Vec::new()),
            show_providers_stack: Mutex::new(Vec::new()),
            torrent_indexes_stack: Mutex::new(Vec::new()),
        }
    }

    pub fn apply_config_order(&self) {
        let discover_order = config::CONFIG.get_value::<config::DiscoverProvidersOrder>();
        let show_order = config::CONFIG.get_value::<config::ShowProvidersOrder>();
        let movie_order = config::CONFIG.get_value::<config::MovieProvidersOrder>();
        let torrent_order = config::CONFIG.get_value::<config::TorrentIndexesOrder>();
        self.order_discover_providers(discover_order.0);
        self.order_movie_providers(movie_order.0);
        self.order_show_providers(show_order.0);
        self.order_torrent_indexes(torrent_order.0);
    }

    pub async fn search_movie(&self, query: &str) -> anyhow::Result<Vec<MovieMetadata>> {
        let discover_providers = { self.discover_providers_stack.lock().unwrap().clone() };
        let lang: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: lang.0 };
        let mut out = Vec::new();
        let handles: Vec<_> = discover_providers
            .into_iter()
            .map(|p| {
                let query = query.to_string();
                tokio::spawn(async move { p.movie_search(&query, fetch_params).await })
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
        let lang: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: lang.0 };
        let handles: Vec<_> = discover_providers
            .into_iter()
            .map(|p| {
                let query = query.to_string();
                tokio::spawn(async move { p.show_search(&query, fetch_params).await })
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
        let lang: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: lang.0 };
        let handles: Vec<_> = discover_providers
            .into_iter()
            .map(|p| {
                let query = query.to_string();
                tokio::spawn(async move { p.multi_search(&query, fetch_params).await })
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
            .find(|p| p.provider_identifier() == provider)
            .context("provider is not supported")?;

        let language: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: language.0 };

        provider.movie(movie_id, fetch_params).await
    }

    pub async fn get_show(
        &self,
        show_id: &str,
        provider: MetadataProvider,
    ) -> Result<ShowMetadata, AppError> {
        let show_providers = { self.show_providers_stack.lock().unwrap().clone() };
        let provider = show_providers
            .into_iter()
            .find(|p| p.provider_identifier() == provider)
            .context("provider is not supported")?;
        let language: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: language.0 };
        provider.show(show_id, fetch_params).await
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
            .find(|p| p.provider_identifier() == provider)
            .context("provider is not supported")?;
        let language: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: language.0 };
        provider.season(show_id, season, fetch_params).await
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
            .find(|p| p.provider_identifier() == provider)
            .context("provider is not supported")?;
        let language: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: language.0 };
        provider
            .episode(show_id, season, episode, fetch_params)
            .await
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
            .find(|p| p.provider_identifier() == provider)
            .context("provider is not supported")?;
        provider.external_ids(id, content_type).await
    }

    pub async fn get_torrents(
        &self,
        query: &str,
        content_type: Option<ContentType>,
    ) -> Vec<Torrent> {
        let torrent_indexes = { self.torrent_indexes_stack.lock().unwrap().clone() };
        let mut out = Vec::new();
        let lang: config::MetadataLanguage = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang: lang.0 };
        let handles: Vec<_> = torrent_indexes
            .into_iter()
            .map(|p| {
                let query = query.to_owned();
                tokio::spawn(async move {
                    tokio::time::timeout(
                        Duration::from_secs(5),
                        match content_type {
                            Some(ContentType::Show) => p.search_show_torrent(&query, &fetch_params),
                            Some(ContentType::Movie) => {
                                p.search_movie_torrent(&query, &fetch_params)
                            }
                            None => p.search_any_torrent(&query, &fetch_params),
                        },
                    )
                    .await
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
                    tracing::error!("Torrent index task panicked: {e}");
                }
            };
        }
        out
    }

    // Can do something smarter here if extract provider_identifier() in its own trait
    pub fn order_discover_providers(
        &self,
        new_order: Vec<MetadataProvider>,
    ) -> Vec<&'static (dyn DiscoverMetadataProvider + Send + Sync)> {
        let out: Vec<_> = new_order
            .into_iter()
            .filter_map(|o| self.discover_provider(o))
            .collect();
        *self.discover_providers_stack.lock().unwrap() = out.clone();
        out
    }

    pub fn order_movie_providers(
        &self,
        new_order: Vec<MetadataProvider>,
    ) -> Vec<&'static (dyn MovieMetadataProvider + Send + Sync)> {
        let out: Vec<_> = new_order
            .into_iter()
            .filter_map(|o| self.movie_provider(o))
            .collect();
        *self.movie_providers_stack.lock().unwrap() = out.clone();
        out
    }

    pub fn order_show_providers(
        &self,
        new_order: Vec<MetadataProvider>,
    ) -> Vec<&'static (dyn ShowMetadataProvider + Send + Sync)> {
        let out: Vec<_> = new_order
            .into_iter()
            .filter_map(|o| self.show_provider(o))
            .collect();
        *self.show_providers_stack.lock().unwrap() = out.clone();
        out
    }

    pub fn order_torrent_indexes(
        &self,
        new_order: Vec<TorrentIndexIdentifier>,
    ) -> Vec<&'static (dyn TorrentIndex + Send + Sync)> {
        let out: Vec<_> = new_order
            .into_iter()
            .filter_map(|o| self.torrent_index(o))
            .collect();
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

    pub fn discover_provider(
        &self,
        provider: MetadataProvider,
    ) -> Option<&'static (dyn DiscoverMetadataProvider + Send + Sync)> {
        match provider {
            MetadataProvider::Local => {
                Some(self.local as &(dyn DiscoverMetadataProvider + Send + Sync))
            }
            MetadataProvider::Tmdb => self
                .tmdb
                .map(|p| p as &(dyn DiscoverMetadataProvider + Send + Sync)),
            MetadataProvider::Tvdb => self
                .tvdb
                .map(|p| p as &(dyn DiscoverMetadataProvider + Send + Sync)),
            MetadataProvider::Imdb => None,
        }
    }

    pub fn movie_provider(
        &self,
        provider: MetadataProvider,
    ) -> Option<&'static (dyn MovieMetadataProvider + Send + Sync)> {
        match provider {
            MetadataProvider::Local => {
                Some(self.local as &(dyn MovieMetadataProvider + Send + Sync))
            }
            MetadataProvider::Tmdb => self
                .tmdb
                .map(|p| p as &(dyn MovieMetadataProvider + Send + Sync)),
            MetadataProvider::Tvdb => self
                .tvdb
                .map(|p| p as &(dyn MovieMetadataProvider + Send + Sync)),
            MetadataProvider::Imdb => None,
        }
    }

    pub fn show_provider(
        &self,
        provider: MetadataProvider,
    ) -> Option<&'static (dyn ShowMetadataProvider + Send + Sync)> {
        match provider {
            MetadataProvider::Local => {
                Some(self.local as &(dyn ShowMetadataProvider + Send + Sync))
            }
            MetadataProvider::Tmdb => self
                .tmdb
                .map(|p| p as &(dyn ShowMetadataProvider + Send + Sync)),
            MetadataProvider::Tvdb => self
                .tvdb
                .map(|p| p as &(dyn ShowMetadataProvider + Send + Sync)),
            MetadataProvider::Imdb => None,
        }
    }

    pub fn torrent_index(
        &self,
        provider: TorrentIndexIdentifier,
    ) -> Option<&'static (dyn TorrentIndex + Send + Sync)> {
        match provider {
            TorrentIndexIdentifier::Tpb => self.tpb.map(|p| p as &(dyn TorrentIndex + Send + Sync)),
            TorrentIndexIdentifier::RuTracker => self
                .rutracker
                .map(|p| p as &(dyn TorrentIndex + Send + Sync)),
        }
    }
}
