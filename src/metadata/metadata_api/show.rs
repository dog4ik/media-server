//! Cases to cover:
//! - Mark as watched for a single episode (possibly outside of library):
//!  1. Fetch resolve external show.
//!  2. Resolve the tree with the target episode
//!  3. Add it to the history, attach resolved metadata_id
//!
//! - Metadata fix
//!  1. Extract all episodes for old metadata.
//!  2. Attach library.sources to it (for fallback image generation)
//!  3. Resolve tree for corrected metadata
//!  4. Write updated tree to the old metadata, save fallbacks if needed
//!
//! - Metadata reset
//!  1. Extract all episodes for old metadata.
//!  2. Attach library.sources, create ShowIdentifiers from the videos path.
//!  4. Lookup show using ShowIdentifier
//!  5. Write updated tree over new metadata
//!
//! - Saved list save (only shows/movies)
//!  1. Resolve show/movie If new save it
//!  2. Create entry in saved list, attach resolved metadata_id
//!
//! - Torrent download file metadata identification
//!  1. Construct ShowIdentifiers from resolved torrent paths.
//!  2. Resolve the show tree, save it.
//!  3. Attach resolved metadata to download.
//!
//! - Refreshing metadata when stale
//!  1. Collect current metadata tree
//!  2. Fetch fresh metadata
//!  3. Write new metadata tree

use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::bail;
use tokio::task::JoinSet;

use crate::{
    config,
    db::{Db, DbActions, DbExternalId, DbTransaction, LocalContentId},
    library::{
        Source,
        assets::{BackdropAsset, BackdropContentType, PosterAsset, PosterContentType},
    },
    metadata::{
        EpisodeMetadata, ExternalIdMetadata, FetchParams, SeasonMetadata, ShowMetadata,
        ShowMetadataProvider, metadata_api::asset_saver::AssetTasks,
    },
    scan::{AssetKind, AssetSaveTask, AssetTaskSource, insert_roles},
};

use super::{MetadataLookup, PendingInsert};

/// A leaf carried through the resolve/write pipeline. The resolver only ever asks a
/// leaf whether it has a local video, used both for episode duration and for poster
/// frame-fallback generation. `None` models a node with no local file (e.g. marking
/// an episode watched outside the library).
pub trait HasSource {
    fn fallback_source(&self) -> Option<Source>;
}

/// An item (e.g. a video) that knows which season/episode of a show it belongs to.
pub trait ShowItem {
    fn season(&self) -> usize;
    fn episode(&self) -> usize;
    fn fallback_source(&self) -> Option<Source>;
}

impl<T: ShowItem> HasSource for T {
    fn fallback_source(&self) -> Option<Source> {
        ShowItem::fallback_source(self)
    }
}

/// The simplest type of tv show leaf node.
#[derive(Debug, Clone, Copy, serde::Deserialize, utoipa::ToSchema)]
pub struct EpisodeNumber(pub usize);

impl HasSource for EpisodeNumber {
    fn fallback_source(&self) -> Option<Source> {
        None
    }
}

/// Grouped input to the resolver: a show's seasons/episodes and the leaf items that
/// belong to each episode, before any metadata is fetched.
#[derive(Debug, Clone)]
pub struct ShowTree<T> {
    pub seasons: Vec<SeasonInput<T>>,
}

#[derive(Debug, Clone)]
pub struct SeasonInput<T> {
    pub number: usize,
    pub episodes: Vec<EpisodeInput<T>>,
}

#[derive(Debug, Clone)]
pub struct EpisodeInput<T> {
    pub number: usize,
    pub items: Vec<T>,
}

impl<T> ShowTree<T> {
    /// Create empty show tree
    pub fn empty() -> Self {
        Self {
            seasons: Vec::new(),
        }
    }
}

impl<T: ShowItem> ShowTree<T> {
    /// Group a flat, unstructured set of items into a season/episode tree.
    pub fn from_flat(mut items: Vec<T>) -> Self {
        items.sort_unstable_by_key(|i| (i.season(), i.episode()));
        let mut seasons: Vec<SeasonInput<T>> = Vec::new();
        for item in items {
            let (season_number, episode_number) = (item.season(), item.episode());
            if seasons.last().map(|s| s.number) != Some(season_number) {
                seasons.push(SeasonInput {
                    number: season_number,
                    episodes: Vec::new(),
                });
            }
            let season = seasons.last_mut().unwrap();
            if season.episodes.last().map(|e| e.number) != Some(episode_number) {
                season.episodes.push(EpisodeInput {
                    number: episode_number,
                    items: Vec::new(),
                });
            }
            season.episodes.last_mut().unwrap().items.push(item);
        }
        Self { seasons }
    }
}

impl<T> Default for ShowTree<T> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<T: ShowItem> From<Vec<T>> for ShowTree<T> {
    fn from(items: Vec<T>) -> Self {
        Self::from_flat(items)
    }
}

/// Implementation assumes that number conversion will succeed without errors
impl<K, V> From<HashMap<K, Vec<V>>> for ShowTree<EpisodeNumber>
where
    K: TryInto<usize, Error: std::fmt::Debug>,
    V: TryInto<usize, Error: std::fmt::Debug>,
{
    fn from(value: HashMap<K, Vec<V>>) -> Self {
        Self {
            seasons: value
                .into_iter()
                .filter_map(|(season, episodes)| {
                    Some(SeasonInput {
                        number: season.try_into().ok()?,
                        episodes: episodes
                            .into_iter()
                            .filter_map(|ep| {
                                Some(EpisodeInput {
                                    number: ep.try_into().ok()?,
                                    items: Vec::new(),
                                })
                            })
                            .collect(),
                    })
                })
                .collect(),
        }
    }
}

/// Carries everything needed to flush a complete show tree to the database.
#[derive(Debug)]
pub struct ResolvedShow<T> {
    pub show_lookup: MetadataLookup<ShowMetadata>,
    pub seasons: Vec<ResolvedSeason<T>>,
}

/// Season resolved to full metadata or existing local ID.
#[derive(Debug)]
pub struct ResolvedSeason<T> {
    pub number: usize,
    pub lookup: MetadataLookup<SeasonMetadata>,
    pub episodes: Vec<ResolvedEpisode<T>>,
}

/// Episode resolved to full metadata or existing local ID.
#[derive(Debug)]
pub struct ResolvedEpisode<T> {
    pub number: usize,
    pub lookup: MetadataLookup<EpisodeMetadata>,
    /// Duration probed from the item's video during the resolve phase (never
    /// inside the flush transaction). `ZERO` for reused/local or video-less nodes.
    pub duration: Duration,
    pub items: Vec<T>,
}

/// A show tree after it has been written to the database. Mirrors [`ResolvedShow`]
/// but every node carries its concrete content/metadata ids. Nodes the provider could not
/// resolve ([`MetadataLookup::Missing`]) are omitted.
#[derive(Debug)]
pub struct WrittenShow<T> {
    pub show_id: i64,
    pub metadata_id: i64,
    pub seasons: Vec<WrittenSeason<T>>,
}

#[derive(Debug)]
pub struct WrittenSeason<T> {
    pub season_id: i64,
    pub metadata_id: i64,
    pub number: usize,
    pub episodes: Vec<WrittenEpisode<T>>,
}

#[derive(Debug)]
pub struct WrittenEpisode<T> {
    pub episode_id: i64,
    pub metadata_id: i64,
    pub number: usize,
    pub items: Vec<T>,
}

impl<T> WrittenShow<T> {
    /// Walks every written episode across all seasons, for callers that only care
    /// about the leaf nodes (e.g. attaching items to history).
    pub fn episodes(&self) -> impl Iterator<Item = &WrittenEpisode<T>> {
        self.seasons.iter().flat_map(|s| s.episodes.iter())
    }
}

/// In-memory snapshot of an existing show's season/episode ids, loaded up front
/// so tree resolution and reconciliation can match nodes by number without per-node database lookups.
pub(super) struct LocalTree {
    pub seasons: HashMap<usize, LocalContentId>,
    pub episodes: HashMap<(usize, usize), LocalContentId>,
}

impl LocalTree {
    pub(super) async fn load(db: &Db, show_id: i64) -> anyhow::Result<Self> {
        let (season_nodes, episode_nodes) = tokio::try_join!(
            db.get_show_season_nodes(show_id),
            db.get_show_episode_nodes(show_id)
        )?;
        let seasons = season_nodes
            .into_iter()
            .map(|n| {
                (
                    n.number as usize,
                    LocalContentId {
                        id: n.id,
                        metadata_id: n.metadata_id,
                    },
                )
            })
            .collect();
        let episodes = episode_nodes
            .into_iter()
            .map(|n| {
                (
                    (n.season_number as usize, n.number as usize),
                    LocalContentId {
                        id: n.id,
                        metadata_id: n.metadata_id,
                    },
                )
            })
            .collect();
        Ok(Self { seasons, episodes })
    }
}

/// Queues the poster asset task for an episode, mirroring the scan behaviour:
/// a poster url falls back to a video frame when the download fails; with no
/// poster but a local video we pull a frame; with neither we queue nothing.
pub(super) fn queue_episode_poster(
    asset_tasks: &mut AssetTasks,
    episode_id: i64,
    poster: Option<String>,
    source: Option<Source>,
) {
    let kind = AssetKind::Poster(PosterAsset::new(episode_id, PosterContentType::Episode));
    let task_source = match (poster, source) {
        (Some(url), Some(source)) => AssetTaskSource::UrlWithFrameFallback { url, source },
        (Some(url), None) => AssetTaskSource::Url(url),
        (None, Some(source)) => AssetTaskSource::VideoFrame(source),
        (None, None) => return,
    };
    asset_tasks.push(AssetSaveTask {
        kind,
        source: task_source,
    });
}

/// Resolves a show tree (show -> seasons -> episodes) against a single metadata provider,
/// reusing local database metadata when it already exists and fetching from the provider
/// otherwise. Nodes the provider cannot resolve become [`MetadataLookup::Missing`].
#[derive(Debug, Clone)]
pub struct ShowMetadataApi<T> {
    provider: T,
    fetch_params: FetchParams,
    db: &'static Db,
    http_client: reqwest::Client,
}

#[cfg(test)]
impl ShowMetadataApi<crate::metadata::metadata_api::tests::provider_mock::MockProvider> {
    pub fn new_test(
        provider: crate::metadata::metadata_api::tests::provider_mock::MockProvider,
        db: &'static Db,
    ) -> Self {
        let fetch_params = FetchParams::default();
        Self {
            provider,
            fetch_params,
            db,
            http_client: reqwest::Client::new(),
        }
    }
}

impl<T> ShowMetadataApi<T>
where
    T: ShowMetadataProvider + Clone + Send + Sync + 'static,
{
    pub fn new(provider: T, db: &'static Db, http_client: reqwest::Client) -> Self {
        let config::MetadataLanguage(lang) = config::CONFIG.get_value();
        let fetch_params = FetchParams { lang };

        Self {
            provider,
            db,
            fetch_params,
            http_client,
        }
    }

    pub async fn search_show_title(
        &self,
        title: &str,
    ) -> anyhow::Result<Option<MetadataLookup<ShowMetadata>>> {
        let search_results = self.provider.show_search(title, self.fetch_params).await?;
        let Some(first_result) = search_results.into_iter().next() else {
            return Ok(None);
        };
        match self
            .db
            .crossreference_show(first_result.metadata_provider, &first_result.metadata_id)
            .await
        {
            Ok(Some(local)) => Ok(Some(MetadataLookup::Local(local))),
            Ok(None) | Err(_) => {
                let mut metadata = self
                    .provider
                    .show(&first_result.metadata_id, self.fetch_params)
                    .await?;
                let external_ids = metadata.external_ids.get_or_insert_default();
                external_ids.insert(
                    0,
                    ExternalIdMetadata {
                        provider: first_result.metadata_provider,
                        id: first_result.metadata_id.clone(),
                    },
                );
                Ok(Some(MetadataLookup::New { metadata }))
            }
        }
    }

    pub async fn search_show_by_id(
        &self,
        id: &str,
    ) -> anyhow::Result<MetadataLookup<ShowMetadata>> {
        let mut show = self.provider.show(id, self.fetch_params).await?;
        match self
            .db
            .crossreference_show(show.metadata_provider, &show.metadata_id)
            .await
        {
            Ok(Some(local)) => Ok(MetadataLookup::Local(local)),
            Ok(None) | Err(_) => {
                let external_ids = show.external_ids.get_or_insert_default();
                external_ids.insert(
                    0,
                    ExternalIdMetadata {
                        provider: show.metadata_provider,
                        id: show.metadata_id.clone(),
                    },
                );
                Ok(MetadataLookup::New { metadata: show })
            }
        }
    }

    /// Resolve the full season/episode tree for a show.
    ///
    /// `items` are grouped by season and episode; each leaf carries the items that belong to
    /// that episode. Local nodes are reused; missing nodes are fetched from the provider.
    pub async fn fetch_show_tree<C>(
        &self,
        show: MetadataLookup<ShowMetadata>,
        tree: impl Into<ShowTree<C>>,
    ) -> anyhow::Result<ResolvedShow<C>>
    where
        C: HasSource + Send + 'static,
    {
        let tree = tree.into();
        // Provider-side id of the show, used to fetch seasons/episodes.
        let provider_show_id: Option<String> = match &show {
            MetadataLookup::New { metadata } => Some(metadata.metadata_id.clone()),
            MetadataLookup::Local(local) => self
                .db
                .get_external_id(local.metadata_id, self.provider.provider_identifier())
                .await?
                .map(|ext| ext.id),
            MetadataLookup::Missing => None,
        };

        // For a local show, load its whole tree once so per-node lookups become
        // in-memory hits instead of one query per season/episode.
        let local_tree = match &show {
            MetadataLookup::Local(local) => {
                Some(Arc::new(LocalTree::load(self.db, local.id).await?))
            }
            MetadataLookup::New { .. } | MetadataLookup::Missing => None,
        };

        let mut handles: JoinSet<ResolvedSeason<C>> = JoinSet::new();
        for season in tree.seasons {
            let api = self.clone();
            let provider_show_id = provider_show_id.clone();
            let local_tree = local_tree.clone();
            handles.spawn(async move {
                api.resolve_season(local_tree.as_deref(), provider_show_id.as_deref(), season)
                    .await
            });
        }

        let seasons = handles.join_all().await;
        Ok(ResolvedShow {
            show_lookup: show,
            seasons,
        })
    }

    /// Resolves the show tree for `id`/`items` and writes it to the database
    ///
    /// A racing writer can claim a node's prime `external_id` between resolve and flush, tripping
    /// the UNIQUE constraint. In sutations like this automatic retry picks up winner's local id
    pub async fn get_or_insert_show_tree<C>(
        &self,
        id: &str,
        items: impl Into<ShowTree<C>>,
    ) -> anyhow::Result<PendingInsert<WrittenShow<C>>>
    where
        C: HasSource + Clone + Send + 'static,
    {
        let tree = items.into();
        for attempt in 0..crate::db::MAX_INSERT_RETRIES {
            if attempt != 0 {
                tracing::warn!(%attempt, "External id unique constraint violated, retrying show tree lookup");
            }
            let show = self.search_show_by_id(id).await?;
            let resolved = self.fetch_show_tree(show, tree.clone()).await?;
            let mut tx = self.db.pool.begin_with("BEGIN IMMEDIATE").await?;
            let mut assets = AssetTasks::new(self.http_client.clone());
            match self.flush_show_tree(&mut tx, &mut assets, resolved).await {
                Ok(content) => {
                    return Ok(PendingInsert {
                        content,
                        tx,
                        assets,
                    });
                }
                Err(e)
                    if e.downcast_ref::<sqlx::Error>()
                        .and_then(|e| e.as_database_error())
                        .is_some_and(|e| e.is_unique_violation()) =>
                {
                    tracing::debug!("Concurrent show insert detected, retrying");
                }
                Err(e) => return Err(e),
            }
        }
        bail!("could not insert show after concurrent insert retries")
    }

    /// Writes a resolved tree to the database and returns the concrete ids.
    ///
    /// Missing nodes are skipped and dropped
    pub async fn flush_show_tree<C>(
        &self,
        tx: &mut DbTransaction,
        asset_tasks: &mut AssetTasks,
        resolved: ResolvedShow<C>,
    ) -> anyhow::Result<WrittenShow<C>>
    where
        C: HasSource,
    {
        let (show_id, show_metadata_id) = match resolved.show_lookup {
            MetadataLookup::Local(local) => (local.id, local.metadata_id),
            MetadataLookup::Missing => bail!("cannot flush a show without metadata"),
            MetadataLookup::New { metadata } => {
                let poster = metadata.poster.clone();
                let backdrop = metadata.backdrop.clone();
                let metadata_id = tx.insert_metadata(&metadata.into_db_metadata()).await?;
                let show_id = tx.insert_show(&metadata.into_db_show(metadata_id)).await?;
                if let Some(cast) = metadata.cast {
                    insert_roles(tx, metadata_id, cast, asset_tasks).await?;
                }
                for ext in metadata.external_ids.into_iter().flatten() {
                    let is_prime = metadata.metadata_provider == ext.provider;
                    let db_ext = DbExternalId {
                        id: None,
                        external_provider: ext.provider,
                        external_id: ext.id,
                        metadata_id: Some(metadata_id),
                        is_prime: is_prime.into(),
                    };
                    if is_prime {
                        // The prime id is the idempotency anchor: a collision means another writer
                        // already inserted this show, so propagate it to trigger a retry.
                        tx.insert_external_id(db_ext).await?;
                    } else if let Err(e) = tx.try_insert_external_id(db_ext).await {
                        tracing::error!(provider = %ext.provider, "Failed to insert external id: {e}");
                    }
                }
                for genre in metadata.genres.into_iter().flatten() {
                    let _ = tx.insert_content_genre(metadata_id, genre.into()).await;
                }
                if let Some(url) = poster {
                    asset_tasks.push(AssetSaveTask {
                        kind: AssetKind::Poster(PosterAsset::new(show_id, PosterContentType::Show)),
                        source: AssetTaskSource::Url(url),
                    });
                }
                if let Some(url) = backdrop {
                    asset_tasks.push(AssetSaveTask {
                        kind: AssetKind::Backdrop(BackdropAsset::new(
                            show_id,
                            BackdropContentType::Show,
                        )),
                        source: AssetTaskSource::Url(url),
                    });
                }
                (show_id, metadata_id)
            }
        };

        let mut written_seasons = Vec::new();
        for resolved_season in resolved.seasons {
            let ResolvedSeason {
                number: season_number,
                lookup,
                episodes,
            } = resolved_season;

            let (season_id, season_metadata_id) = match lookup {
                MetadataLookup::Local(local) => (local.id, local.metadata_id),
                MetadataLookup::Missing => {
                    tracing::warn!(season = season_number, "Skipping season without metadata");
                    continue;
                }
                MetadataLookup::New { metadata } => {
                    let poster = metadata.poster.clone();
                    let metadata_id = tx.insert_metadata(&metadata.into_db_metadata()).await?;
                    let season_id = tx
                        .insert_season(metadata.into_db_season(metadata_id, show_id))
                        .await?;
                    if let Some(cast) = metadata.cast {
                        insert_roles(tx, metadata_id, cast, asset_tasks).await?;
                    }
                    if let Some(url) = poster {
                        asset_tasks.push(AssetSaveTask {
                            kind: AssetKind::Poster(PosterAsset::new(
                                season_id,
                                PosterContentType::Season,
                            )),
                            source: AssetTaskSource::Url(url),
                        });
                    }
                    (season_id, metadata_id)
                }
            };

            let mut written_episodes = Vec::new();
            for resolved_episode in episodes {
                let ResolvedEpisode {
                    number: episode_number,
                    lookup,
                    duration,
                    items,
                } = resolved_episode;

                let (episode_id, episode_metadata_id) = match lookup {
                    MetadataLookup::Local(local) => (local.id, local.metadata_id),
                    MetadataLookup::Missing => {
                        tracing::warn!(
                            season = season_number,
                            episode = episode_number,
                            "Skipping episode without metadata"
                        );
                        continue;
                    }
                    MetadataLookup::New { metadata } => {
                        let poster = metadata.poster.clone();
                        let ext_provider = metadata.metadata_provider;
                        let ext_id = metadata.metadata_id.clone();
                        let metadata_id = tx.insert_metadata(&metadata.into_db_metadata()).await?;
                        if !ext_provider.is_local() {
                            tx.insert_external_id(DbExternalId {
                                id: None,
                                external_provider: ext_provider,
                                external_id: ext_id,
                                metadata_id: Some(metadata_id),
                                is_prime: true.into(),
                            })
                            .await?;
                        }
                        let episode_id = tx
                            .insert_episode(&metadata.into_db_episode(
                                metadata_id,
                                season_id,
                                duration,
                            ))
                            .await?;
                        if let Some(cast) = metadata.cast {
                            insert_roles(tx, metadata_id, cast, asset_tasks).await?;
                        }
                        let source = items.first().and_then(|i| i.fallback_source());
                        queue_episode_poster(asset_tasks, episode_id, poster, source);
                        (episode_id, metadata_id)
                    }
                };

                written_episodes.push(WrittenEpisode {
                    episode_id,
                    metadata_id: episode_metadata_id,
                    number: episode_number,
                    items,
                });
            }

            written_seasons.push(WrittenSeason {
                season_id,
                metadata_id: season_metadata_id,
                number: season_number,
                episodes: written_episodes,
            });
        }

        Ok(WrittenShow {
            show_id,
            metadata_id: show_metadata_id,
            seasons: written_seasons,
        })
    }

    async fn season_lookup(
        &self,
        local_tree: Option<&LocalTree>,
        provider_show_id: Option<&str>,
        season_number: usize,
    ) -> MetadataLookup<SeasonMetadata> {
        match local_tree.and_then(|t| t.seasons.get(&season_number)) {
            Some(local) => MetadataLookup::Local(*local),
            None => {
                self.fetch_season_metadata(provider_show_id, season_number)
                    .await
            }
        }
    }

    async fn resolve_season<C>(
        &self,
        local_tree: Option<&LocalTree>,
        provider_show_id: Option<&str>,
        season: SeasonInput<C>,
    ) -> ResolvedSeason<C>
    where
        C: HasSource,
    {
        let SeasonInput {
            number: season_number,
            episodes,
        } = season;
        let season_lookup = self
            .season_lookup(local_tree, provider_show_id, season_number)
            .await;

        let mut resolved_episodes = Vec::new();
        for episode in episodes {
            resolved_episodes.push(
                self.resolve_episode(local_tree, provider_show_id, season_number, episode)
                    .await,
            );
        }

        ResolvedSeason {
            number: season_number,
            lookup: season_lookup,
            episodes: resolved_episodes,
        }
    }

    async fn fetch_season_metadata(
        &self,
        provider_show_id: Option<&str>,
        season_number: usize,
    ) -> MetadataLookup<SeasonMetadata> {
        let Some(provider_show_id) = provider_show_id else {
            return MetadataLookup::Missing;
        };
        match self
            .provider
            .season(provider_show_id, season_number, self.fetch_params)
            .await
        {
            Ok(metadata) => MetadataLookup::New { metadata },
            Err(_) => MetadataLookup::Missing,
        }
    }

    async fn resolve_episode<C>(
        &self,
        local_tree: Option<&LocalTree>,
        provider_show_id: Option<&str>,
        season_number: usize,
        episode: EpisodeInput<C>,
    ) -> ResolvedEpisode<C>
    where
        C: HasSource,
    {
        let EpisodeInput {
            number: episode_number,
            items,
        } = episode;
        if let Some(local) =
            local_tree.and_then(|t| t.episodes.get(&(season_number, episode_number)))
        {
            return ResolvedEpisode {
                number: episode_number,
                lookup: MetadataLookup::Local(*local),
                duration: Duration::ZERO,
                items,
            };
        }

        // Probe duration here (resolve phase) so the flush transaction never blocks on ffprobe.
        // Bind the source first so the (non-Send) iterator is dropped before the await.
        let fallback_source = items.iter().find_map(|i| i.fallback_source());
        let duration = match fallback_source {
            Some(source) => source.video.fetch_duration().await.unwrap_or_default(),
            None => Duration::ZERO,
        };

        let lookup = match provider_show_id {
            Some(provider_show_id) => match self
                .provider
                .episode(
                    provider_show_id,
                    season_number,
                    episode_number,
                    self.fetch_params,
                )
                .await
            {
                Ok(metadata) => MetadataLookup::New { metadata },
                Err(_) => MetadataLookup::Missing,
            },
            None => MetadataLookup::Missing,
        };

        ResolvedEpisode {
            number: episode_number,
            lookup,
            duration,
            items,
        }
    }
}

pub(super) struct BatchResult<T, S = ()> {
    pub api: ShowMetadataApi<&'static (dyn ShowMetadataProvider + Send + Sync + 'static)>,
    pub resolved: ResolvedShow<T>,
    pub state: S,
}

/// Wrapper around [ShowMetadataApi] that allows processing many shows
pub(super) struct BatchShowApi<T, S> {
    pub join_set: JoinSet<anyhow::Result<BatchResult<T, S>>>,
}

impl<T, S> BatchShowApi<T, S>
where
    T: HasSource + Send + 'static,
    S: Send + 'static,
{
    pub fn new() -> Self {
        Self {
            join_set: JoinSet::new(),
        }
    }

    /// Spawn a resolving task for a show tree
    pub fn spawn(
        &mut self,
        api: ShowMetadataApi<&'static (dyn ShowMetadataProvider + Send + Sync + 'static)>,
        show_id: String,
        tree: impl Into<ShowTree<T>>,
        state: S,
    ) {
        let tree = tree.into();
        self.join_set.spawn(async move {
            let show = api.search_show_by_id(&show_id).await?;
            let resolved = api.fetch_show_tree(show, tree).await?;
            Ok(BatchResult {
                api,
                resolved,
                state,
            })
        });
    }
}
