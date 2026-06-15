use std::{sync::Arc, time::Duration};

use tokio::task::JoinSet;
use tracing::instrument;

use crate::{
    db::{Db, DbActions},
    library::{LibraryItem, Media, show::ShowIdentifier},
    metadata::{ContentType, EpisodeMetadata, SeasonMetadata, ShowMetadata, ShowMetadataProvider},
    scan::scan_progress::FailedContent,
};

use super::{
    MetadataLookup, MetadataLookupWithIds, ScanConfig,
    fallback::{episode_fallback, season_fallback},
    scan_progress::MetadataProgressEmitter,
};

/// Provider ID pair derived from a show's external_ids after show lookup.
pub struct ShowProvider {
    pub provider: &'static (dyn ShowMetadataProvider + Send + Sync),
    pub id: String,
}

/// Episode resolved to full metadata or existing local ID.
pub struct ResolvedEpisode {
    pub lookup: MetadataLookup<EpisodeMetadata>,
    pub duration: Duration,
    pub videos: Vec<LibraryItem<ShowIdentifier>>,
}

/// Season resolved to full metadata or existing local ID.
pub struct ResolvedSeason {
    pub lookup: MetadataLookup<SeasonMetadata>,
    pub episodes: Vec<ResolvedEpisode>,
}

/// Carries everything needed to flush a complete show tree to the database.
pub struct ResolvedShow {
    pub show_lookup: MetadataLookupWithIds<ShowMetadata>,
    pub seasons: Vec<ResolvedSeason>,
}

/// Fetches seasons and episodes for a single show group.
#[derive(Clone)]
pub struct EpisodeScanner {
    db: Db,
    providers: Arc<[ShowProvider]>,
    config: ScanConfig,
    progress: MetadataProgressEmitter,
}

impl EpisodeScanner {
    pub(super) fn new(
        db: Db,
        providers: Arc<[ShowProvider]>,
        config: ScanConfig,
        progress: MetadataProgressEmitter,
    ) -> Self {
        Self {
            db,
            providers,
            config,
            progress,
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
        ResolvedShow {
            show_lookup,
            seasons,
        }
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

        let fresh_season_episodes = match &season_lookup {
            MetadataLookup::Local(_) => None,
            MetadataLookup::New { metadata } => Some(&metadata.episodes),
        };

        season_videos.sort_unstable_by_key(|v| v.identifier.episode);

        let mut episodes = Vec::new();
        for episode_videos in season_videos
            .chunk_by(|a, b| a.identifier.episode == b.identifier.episode)
            .map(Vec::from)
        {
            let episode_number = episode_videos.first().unwrap().identifier.episode as usize;
            let resolved = match self.config.use_season_episodes {
                true if let Some(fresh_episode) = fresh_season_episodes
                    .into_iter()
                    .flatten()
                    .find(|ep| ep.number == episode_number) =>
                {
                    let duration = if let Some(first) = episode_videos.first() {
                        first
                            .source
                            .video
                            .fetch_duration()
                            .await
                            .unwrap_or_default()
                    } else {
                        Duration::ZERO
                    };

                    self.progress.dispatch_success(episode_videos.len());
                    ResolvedEpisode {
                        lookup: MetadataLookup::New {
                            metadata: fresh_episode.clone(),
                        },
                        duration,
                        videos: episode_videos,
                    }
                }
                _ => {
                    self.resolve_episode(show_id, season_number, episode_number, episode_videos)
                        .await
                }
            };
            episodes.push(resolved);
        }

        ResolvedSeason {
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
        season_number: usize,
        episode_number: usize,
        videos: Vec<LibraryItem<ShowIdentifier>>,
    ) -> ResolvedEpisode {
        let content_type = ContentType::Show;
        if let Some(show_id) = show_id {
            if let Ok(local_id) = self
                .db
                .get_episode_id(show_id, season_number, episode_number)
                .await
            {
                self.progress.dispatch_success(videos.len());
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
                self.progress.dispatch_success(videos.len());
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
        let title = videos[0].identifier.title();
        self.progress.dispatch_fail(
            FailedContent {
                title: format!("{title} S{:0>2}E{:0>2}", season_number, episode_number),
                videos: videos
                    .iter()
                    .map(|v| v.source.video.path().to_path_buf())
                    .collect(),
                content_type,
            },
            videos.len(),
        );
        ResolvedEpisode {
            lookup: episode_fallback(episode_number, season_number),
            duration,
            videos,
        }
    }
}
