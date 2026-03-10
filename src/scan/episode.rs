use std::{sync::Arc, time::Duration};

use tokio::task::JoinSet;
use tracing::instrument;

use crate::{
    db::{Db, DbActions},
    library::{LibraryItem, show::ShowIdentifier},
    metadata::{EpisodeMetadata, SeasonMetadata, ShowMetadata, ShowMetadataProvider},
};

use super::{
    MetadataLookup, MetadataLookupWithIds, ScanConfig,
    fallback::{episode_fallback, season_fallback},
};

/// Provider ID pair derived from a show's external_ids after show lookup.
pub(super) struct ShowProvider {
    pub provider: &'static (dyn ShowMetadataProvider + Send + Sync),
    pub id: String,
}

/// Episode resolved to full metadata or existing local ID.
pub(super) struct ResolvedEpisode {
    pub lookup: MetadataLookup<EpisodeMetadata>,
    pub duration: Duration,
    pub videos: Vec<LibraryItem<ShowIdentifier>>,
}

/// Season resolved to full metadata or existing local ID.
pub(super) struct ResolvedSeason {
    pub season_number: usize,
    pub lookup: MetadataLookup<SeasonMetadata>,
    pub episodes: Vec<ResolvedEpisode>,
}

/// Carries everything needed to flush a complete show tree to the database.
pub(super) struct ResolvedShow {
    pub show_lookup: MetadataLookupWithIds<ShowMetadata>,
    pub seasons: Vec<ResolvedSeason>,
}

/// Fetches seasons and episodes for a single show group.
#[derive(Clone)]
pub(super) struct EpisodeScanner {
    db: Db,
    providers: Arc<[ShowProvider]>,
    config: ScanConfig,
}

impl EpisodeScanner {
    pub(super) fn new(db: Db, providers: Arc<[ShowProvider]>, config: ScanConfig) -> Self {
        Self {
            db,
            providers,
            config,
        }
    }

    pub(super) async fn resolve_show(
        &self,
        show_lookup: MetadataLookupWithIds<ShowMetadata>,
        mut videos: Vec<LibraryItem<ShowIdentifier>>,
    ) -> ResolvedShow {
        let show_id = match &show_lookup {
            MetadataLookupWithIds::Local(id) => Some(*id),
            MetadataLookupWithIds::New { .. } => None,
        };

        videos.sort_unstable_by_key(|v| v.identifier.season);

        let mut handles: JoinSet<ResolvedSeason> = JoinSet::new();
        for season_videos in videos
            .chunk_by(|a, b| a.identifier.season == b.identifier.season)
            .map(Vec::from)
        {
            let season_number = season_videos.first().unwrap().identifier.season as usize;
            let scanner = self.clone();
            handles.spawn(async move {
                scanner
                    .resolve_season(show_id, season_number, season_videos)
                    .await
            });
        }

        let seasons = handles.join_all().await;
        ResolvedShow { show_lookup, seasons }
    }

    #[instrument(skip(self, season_videos), fields(season = season_number))]
    async fn resolve_season(
        &self,
        show_id: Option<i64>,
        season_number: usize,
        mut season_videos: Vec<LibraryItem<ShowIdentifier>>,
    ) -> ResolvedSeason {
        let season_lookup = if let Some(show_id) = show_id {
            if let Ok(local_id) = self.db.get_season_id(show_id, season_number).await {
                MetadataLookup::Local(local_id)
            } else {
                self.fetch_season_metadata(season_number).await
            }
        } else {
            self.fetch_season_metadata(season_number).await
        };

        let season_id = match &season_lookup {
            MetadataLookup::Local(id) => Some(*id),
            MetadataLookup::New { .. } => None,
        };

        season_videos.sort_unstable_by_key(|v| v.identifier.episode);

        let mut episodes = Vec::new();
        for episode_videos in season_videos
            .chunk_by(|a, b| a.identifier.episode == b.identifier.episode)
            .map(Vec::from)
        {
            let episode_number = episode_videos.first().unwrap().identifier.episode as usize;
            let resolved = self
                .resolve_episode(show_id, season_id, season_number, episode_number, episode_videos)
                .await;
            episodes.push(resolved);
        }

        ResolvedSeason {
            season_number,
            lookup: season_lookup,
            episodes,
        }
    }

    async fn fetch_season_metadata(&self, season_number: usize) -> MetadataLookup<SeasonMetadata> {
        for provider in self.providers.iter() {
            if let Ok(season) = provider
                .provider
                .season(&provider.id, season_number, self.config.fetch_params)
                .await
            {
                return MetadataLookup::New { metadata: season };
            }
        }
        tracing::warn!(season = season_number, "Using season metadata fallback");
        season_fallback(season_number)
    }

    #[instrument(skip(self, videos), fields(season = season_number, episode = episode_number))]
    async fn resolve_episode(
        &self,
        show_id: Option<i64>,
        _season_id: Option<i64>,
        season_number: usize,
        episode_number: usize,
        videos: Vec<LibraryItem<ShowIdentifier>>,
    ) -> ResolvedEpisode {
        if let Some(show_id) = show_id {
            if let Ok(local_id) = self
                .db
                .get_episode_id(show_id, season_number, episode_number)
                .await
            {
                return ResolvedEpisode {
                    lookup: MetadataLookup::Local(local_id),
                    duration: Duration::ZERO,
                    videos,
                };
            }
        }

        let duration = if let Some(first) = videos.first() {
            first
                .source
                .video
                .fetch_duration()
                .await
                .unwrap_or_default()
        } else {
            Duration::ZERO
        };

        for provider in self.providers.iter() {
            if let Ok(episode) = provider
                .provider
                .episode(
                    &provider.id,
                    season_number,
                    episode_number,
                    self.config.fetch_params,
                )
                .await
            {
                return ResolvedEpisode {
                    lookup: MetadataLookup::New { metadata: episode },
                    duration,
                    videos,
                };
            }
        }

        tracing::warn!(
            season = season_number,
            episode = episode_number,
            "Using episode metadata fallback"
        );
        ResolvedEpisode {
            lookup: episode_fallback(episode_number, season_number),
            duration,
            videos,
        }
    }
}

