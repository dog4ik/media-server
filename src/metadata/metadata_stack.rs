use std::{collections::HashMap, sync::Mutex, time::Duration};

use anyhow::Context;
use serde::{ser::SerializeStruct, Serialize};

use crate::{
    app_state::AppError,
    config,
    torrent_index::{Torrent, TorrentIndex},
};

use super::{
    ContentType, DiscoverMetadataProvider, EpisodeMetadata, ExternalIdMetadata, FetchParams,
    MetadataProvider, MetadataSearchResult, MovieMetadata, MovieMetadataProvider, SeasonMetadata,
    ShowMetadata, ShowMetadataProvider,
};

#[derive(Default)]
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
    pub fn new(
        discover_providers_stack: Vec<&'static (dyn DiscoverMetadataProvider + Send + Sync)>,
        movie_providers_stack: Vec<&'static (dyn MovieMetadataProvider + Send + Sync)>,
        show_providers_stack: Vec<&'static (dyn ShowMetadataProvider + Send + Sync)>,
        torrent_indexes_stack: Vec<&'static (dyn TorrentIndex + Send + Sync)>,
    ) -> Self {
        let discover_order = config::CONFIG.get_value::<config::DiscoverProvidersOrder>();
        let show_order = config::CONFIG.get_value::<config::ShowProvidersOrder>();
        let movie_order = config::CONFIG.get_value::<config::MovieProvidersOrder>();
        let torrent_order = config::CONFIG.get_value::<config::TorrentIndexesOrder>();
        let stack = Self {
            discover_providers_stack: discover_providers_stack.into(),
            movie_providers_stack: movie_providers_stack.into(),
            show_providers_stack: show_providers_stack.into(),
            torrent_indexes_stack: torrent_indexes_stack.into(),
        };
        stack.order_discover_providers(discover_order.0);
        stack.order_movie_providers(movie_order.0);
        stack.order_show_providers(show_order.0);
        stack.order_torrent_indexes(torrent_order.0);
        stack
    }

    pub fn add_discover_provider(
        &mut self,
        provider: &'static (dyn DiscoverMetadataProvider + Send + Sync),
    ) {
        let mut stack = self.discover_providers_stack.lock().unwrap();
        stack.push(provider);
    }

    pub fn add_show_provider(
        &mut self,
        provider: &'static (dyn ShowMetadataProvider + Send + Sync),
    ) {
        let mut stack = self.show_providers_stack.lock().unwrap();
        stack.push(provider);
    }

    pub fn add_movie_provider(
        &mut self,
        provider: &'static (dyn MovieMetadataProvider + Send + Sync),
    ) {
        let mut stack = self.movie_providers_stack.lock().unwrap();
        stack.push(provider);
    }

    pub fn add_torrent_provider(&mut self, provider: &'static (dyn TorrentIndex + Send + Sync)) {
        let mut stack = self.torrent_indexes_stack.lock().unwrap();
        stack.push(provider);
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
            .find(|p| p.provider_identifier() == provider.to_string())
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
            .find(|p| p.provider_identifier() == provider.to_string())
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
            .find(|p| p.provider_identifier() == provider.to_string())
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
            .find(|p| p.provider_identifier() == provider.to_string())
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
            .find(|p| p.provider_identifier() == provider.to_string())
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
