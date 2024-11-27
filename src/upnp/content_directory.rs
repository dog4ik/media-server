use std::{
    fmt::Display,
    str::FromStr,
    sync::{
        atomic::{self, AtomicU32},
        Arc,
    },
};

use anyhow::Context;
use upnp::{
    action::ActionError,
    content_directory::{
        class::{ItemType, VideoItemType},
        error,
        properties::{
            self,
            res::{ProtocolInfo, Resource},
            DidlResponse,
        },
        Container, ContentDirectoryHandler, Item, UpnpResolution,
    },
};

use crate::{
    app_state::AppState,
    db::{self, DbActions},
};

#[derive(Clone)]
pub struct MediaServerContentDirectory {
    app_state: AppState,
    server_location: String,
    update_id: Arc<AtomicU32>,
}

impl MediaServerContentDirectory {
    pub fn new(app_state: AppState, server_location: String) -> Self {
        Self {
            app_state,
            server_location,
            update_id: AtomicU32::new(0).into(),
        }
    }

    pub fn root() -> DidlResponse {
        let shows = Container::new(
            ContentId::AllShows.to_string(),
            ContentId::Root.to_string(),
            "Shows".to_string(),
        );
        let movies = Container::new(
            ContentId::AllMovies.to_string(),
            ContentId::Root.to_string(),
            "Movies".to_string(),
        );
        DidlResponse {
            containers: vec![shows, movies],
            items: vec![],
        }
    }

    pub fn root_metadata() -> DidlResponse {
        let root = Container::new(
            ContentId::Root.to_string(),
            "-1".into(),
            "Media server".into(),
        );
        DidlResponse {
            containers: vec![root],
            items: vec![],
        }
    }

    pub async fn all_shows(&self, requested_count: i64) -> anyhow::Result<DidlResponse> {
        let shows = self.app_state.db.all_shows(requested_count).await?;
        let mut containers = Vec::with_capacity(shows.len());
        for show in shows {
            let poster_url = format!(
                "{server_url}/api/show/{show_id}/poster",
                server_url = self.server_location,
                show_id = show.metadata_id
            );
            let show_id = show.metadata_id.parse().expect("db ids to be integers");
            let mut container = Container::new(
                ContentId::Show(show_id).to_string(),
                ContentId::AllShows.to_string(),
                show.title,
            );
            container.set_property(properties::AlbumArtUri(poster_url));
            if let Some(plot) = show.plot {
                container.set_property(properties::Description(plot));
            }
            containers.push(container);
        }
        Ok(DidlResponse {
            containers,
            items: vec![],
        })
    }

    pub async fn show(&self, show_id: i64) -> anyhow::Result<DidlResponse> {
        let show = self.app_state.db.get_show(show_id).await?;
        let seasons = show.seasons.unwrap_or_default();
        let mut containers = Vec::with_capacity(seasons.len());
        for season in seasons {
            let container = Container::new(
                ContentId::Season {
                    show_id,
                    season: season as i64,
                }
                .to_string(),
                ContentId::Show(show_id).to_string(),
                format!("Season {}", season),
            );
            containers.push(container);
        }
        Ok(DidlResponse {
            containers,
            items: vec![],
        })
    }

    pub async fn show_season(&self, show_id: i64, season: i64) -> anyhow::Result<DidlResponse> {
        let db = self.app_state.db;
        let show_metadata = db.get_show(show_id).await?;
        let episodes = db
            .get_local_season_episodes(show_id, season as usize)
            .await?;
        let mut items = Vec::with_capacity(episodes.len());
        for episode in episodes {
            let Ok(video_ids) = sqlx::query!(
                "SELECT id FROM videos WHERE videos.episode_id = ?",
                episode.id
            )
            .fetch_all(&db.pool)
            .await
            .map(|r| r.into_iter().map(|r| r.id).collect::<Vec<_>>()) else {
                continue;
            };

            let id = episode.id.unwrap();
            let poster_url = format!(
                "{server_url}/api/episode/{episode_id}/poster",
                server_url = self.server_location,
                episode_id = id,
            );
            let season_id = ContentId::Season { show_id, season };
            let item_id = ContentId::Episode {
                show_id,
                season,
                episode: episode.number as i64,
            };
            let mut item = Item::new(
                item_id.to_string(),
                season_id.to_string(),
                episode.title.clone(),
            );
            {
                for id in video_ids {
                    let watch_url = format!(
                        "{server_url}/api/video/{video_id}/watch",
                        server_url = self.server_location,
                        video_id = id,
                    );
                    let source = {
                        let library = self.app_state.library.lock().unwrap();
                        library.get_source(id).unwrap().clone()
                    };

                    let metadata = source.video.metadata().await;
                    let mut watch_resource = Resource::new(
                        watch_url.clone(),
                        ProtocolInfo::http_get("video/matroska".into()),
                    );
                    if let Ok(size) = source.video.async_file_size().await {
                        watch_resource.set_size(size);
                    }
                    let runtime = std::time::Duration::from_secs(episode.duration as u64);
                    watch_resource.set_duartion(runtime);
                    if let Ok(metadata) = metadata {
                        if let Some(res) = metadata.resolution() {
                            watch_resource
                                .set_resoulution(UpnpResolution::new(res.width(), res.height()));
                        };
                        if let Some(audio_channels) = metadata.default_audio().map(|a| a.channels) {
                            watch_resource.set_audio_channels(audio_channels as usize);
                        };
                        watch_resource.set_bitrate(metadata.bitrate());
                    }
                    item.set_property(watch_resource);
                    item.set_property(properties::RecordedDuration(runtime));
                }
            }

            item.set_property(properties::AlbumArtUri(poster_url));
            item.set_property(properties::ProgramTitle(episode.title));
            item.set_property(properties::EpisodeNumber(episode.number as u32));
            item.set_property(properties::EpisodeSeason(season as u32));
            item.set_property(properties::SeriesTitle(show_metadata.title.clone()));
            //if let Some(release_date) = episode.release_date {
            //    item.set_property(
            //        properties::Date::from_str(&release_date).expect("rfc 3339 date"),
            //    );
            //}
            if let Some(amount) = show_metadata.episodes_amount {
                item.set_property(properties::EpisodeCount(amount as u32));
            }
            if let Some(description) = episode.plot {
                item.set_property(properties::Description(description));
            }

            item.base.set_upnp_class(ItemType::VideoItem(None));
            items.push(item);
        }
        Ok(DidlResponse {
            containers: vec![],
            items,
        })
    }

    pub async fn episode_metadata(
        &self,
        show_id: i64,
        season: i64,
        episode: i64,
    ) -> anyhow::Result<DidlResponse> {
        let episode_metadata = self
            .app_state
            .db
            .get_episode(show_id, season as usize, episode as usize)
            .await?;
        let poster_url = format!(
            "{server_url}/api/{show_id}/{season}/{episode}/poster",
            server_url = self.server_location,
        );
        let watch_url = format!(
            "{server_url}/api/local_episode/{episode_id}/watch",
            server_url = self.server_location,
            episode_id = episode_metadata.metadata_id
        );
        let item_id = ContentId::Episode {
            show_id,
            season,
            episode,
        };
        let mut item = Item::new(
            item_id.to_string(),
            ContentId::Season { show_id, season }.to_string(),
            episode_metadata.title,
        );
        item.base.set_upnp_class(Some(ItemType::VideoItem(None)));
        if let Some(plot) = episode_metadata.plot {
            item.set_property(properties::Description(plot));
        }
        item.set_property(properties::EpisodeNumber(episode_metadata.number as u32));
        item.set_property(properties::EpisodeSeason(
            episode_metadata.season_number as u32,
        ));
        item.set_property(properties::AlbumArtUri(poster_url));
        let watch_resource =
            Resource::new(watch_url, ProtocolInfo::http_get("video/matroska".into()));
        item.set_property(watch_resource);
        Ok(DidlResponse {
            containers: vec![],
            items: vec![item],
        })
    }

    pub async fn all_movies(&self, requested_count: i64) -> anyhow::Result<DidlResponse> {
        let movies = self.app_state.db.all_movies(requested_count).await?;
        let mut items = Vec::with_capacity(movies.len());
        for movie in movies {
            let poster_url = format!(
                "{server_url}/api/movie/{movie_id}/poster",
                server_url = self.server_location,
                movie_id = movie.metadata_id
            );
            let watch_url = format!(
                "{server_url}/api/local_movie/{movie_id}/watch",
                server_url = self.server_location,
                movie_id = movie.metadata_id
            );
            let container_id =
                ContentId::Movie(movie.metadata_id.parse().expect("local ids to be integers"));
            let mut item = Item::new(
                container_id.to_string(),
                ContentId::AllMovies.to_string(),
                movie.title,
            );
            item.base
                .set_upnp_class(Some(ItemType::VideoItem(Some(VideoItemType::Movie))));
            item.set_property(properties::AlbumArtUri(poster_url));
            if let Some(plot) = movie.plot {
                item.set_property(properties::Description(plot));
            }
            let watch_resource =
                Resource::new(watch_url, ProtocolInfo::http_get("video/matroska".into()));
            item.set_property(watch_resource);
            items.push(item);
        }
        Ok(DidlResponse {
            containers: vec![],
            items,
        })
    }

    pub fn all_movies_metadata() -> DidlResponse {
        let all_movies = Container::new(
            ContentId::AllMovies.to_string(),
            ContentId::Root.to_string(),
            "Movies".to_string(),
        );
        DidlResponse {
            containers: vec![all_movies],
            items: vec![],
        }
    }

    pub fn all_shows_metadata() -> DidlResponse {
        let all_movies = Container::new(
            ContentId::AllShows.to_string(),
            ContentId::Root.to_string(),
            "Shows".to_string(),
        );
        DidlResponse {
            containers: vec![all_movies],
            items: vec![],
        }
    }

    pub async fn movie_metadata(&self, movie_id: i64) -> anyhow::Result<DidlResponse> {
        let movie = self.app_state.db.get_movie(movie_id).await?;
        let poster_url = format!(
            "{server_url}/api/movie/{movie_id}/poster",
            server_url = self.server_location,
            movie_id = movie.metadata_id
        );
        let watch_url = format!(
            "{server_url}/api/local_movie/{movie_id}/watch",
            server_url = self.server_location,
            movie_id = movie.metadata_id
        );
        let movie_id = ContentId::Movie(movie_id);
        let mut item = Item::new(
            movie_id.to_string(),
            ContentId::AllMovies.to_string(),
            movie.title,
        );
        item.base.set_upnp_class(Some(ItemType::VideoItem(None)));
        item.set_property(properties::AlbumArtUri(poster_url));
        if let Some(plot) = movie.plot {
            item.set_property(properties::Description(plot));
        }
        let watch_resource =
            Resource::new(watch_url, ProtocolInfo::http_get("video/matroska".into()));
        item.set_property(watch_resource);
        Ok(DidlResponse {
            containers: vec![],
            items: vec![item],
        })
    }

    pub async fn show_metadata(&self, show_id: i64) -> anyhow::Result<DidlResponse> {
        let show = self.app_state.db.get_show(show_id).await?;
        let poster_url = format!(
            "{server_url}/api/show/{show_id}/poster",
            server_url = self.server_location,
            show_id = show.metadata_id
        );
        let show_id = ContentId::Show(show_id);
        let mut container = Container::new(
            show_id.to_string(),
            ContentId::AllShows.to_string(),
            show.title,
        );
        container.set_property(properties::AlbumArtUri(poster_url));
        if let Some(plot) = show.plot {
            container.set_property(properties::Description(plot));
        }
        Ok(DidlResponse {
            containers: vec![container],
            items: vec![],
        })
    }

    pub async fn season_metadata(&self, show_id: i64, season: i64) -> anyhow::Result<DidlResponse> {
        let season_metadata = self
            .app_state
            .db
            .get_season(show_id, season as usize)
            .await?;
        let season_id = ContentId::Season { show_id, season };
        let mut container = Container::new(
            season_id.to_string(),
            ContentId::Show(show_id).to_string(),
            format!("Season {}", season),
        );
        if let Some(plot) = season_metadata.plot {
            container.set_property(properties::Description(plot));
        }
        Ok(DidlResponse {
            containers: vec![container],
            items: vec![],
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum ContentId {
    Root,
    AllMovies,
    AllShows,
    Movie(i64),
    Show(i64),
    Season {
        show_id: i64,
        season: i64,
    },
    Episode {
        show_id: i64,
        season: i64,
        episode: i64,
    },
}

impl Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentId::Root => write!(f, "0"),
            ContentId::AllMovies => write!(f, "movies"),
            ContentId::AllShows => write!(f, "shows"),
            ContentId::Show(id) => write!(f, "show.{id}"),
            ContentId::Movie(id) => write!(f, "movie.{id}"),
            ContentId::Season { show_id, season } => write!(f, "show.{show_id}.{season}"),
            ContentId::Episode {
                show_id,
                season,
                episode,
            } => write!(f, "show.{show_id}.{season}.{episode}"),
        }
    }
}

impl FromStr for ContentId {
    type Err = error::NoSuchObjectError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "0" {
            return Ok(Self::Root);
        }
        if s == "movies" {
            return Ok(Self::AllMovies);
        }
        if s == "shows" {
            return Ok(Self::AllShows);
        }
        if let Some(show) = s.strip_prefix("show.") {
            let mut split = show.split('.');
            let show_id = split.next().and_then(|s| s.parse().ok());
            let season = split.next().and_then(|s| s.parse().ok());
            let episode = split.next().and_then(|e| e.parse().ok());
            match (show_id, season, episode) {
                (Some(show_id), None, None) => return Ok(Self::Show(show_id)),
                (Some(show_id), Some(season), None) => return Ok(Self::Season { show_id, season }),
                (Some(show_id), Some(season), Some(episode)) => {
                    return Ok(Self::Episode {
                        show_id,
                        season,
                        episode,
                    })
                }
                _ => {}
            }
        }
        if let Some(movie) = s.strip_prefix("movie.") {
            let movie_id = movie.parse().context("parse movie id")?;
            return Ok(Self::Movie(movie_id));
        }
        Err(anyhow::anyhow!("failed to parse content id: {s}"))?
    }
}

impl ContentDirectoryHandler for MediaServerContentDirectory {
    async fn browse_direct_children(
        &self,
        object_id: &str,
        requested_count: u32,
    ) -> Result<DidlResponse, ActionError> {
        let content_id = object_id.parse()?;
        let requested_count = if requested_count == 0 {
            db::DEFAULT_LIMIT
        } else {
            requested_count as i64
        };
        match content_id {
            ContentId::Root => Ok(Self::root()),
            ContentId::AllMovies => Ok(self.all_movies(requested_count).await?),
            ContentId::AllShows => Ok(self.all_shows(requested_count).await?),
            ContentId::Show(id) => Ok(self.show(id).await?),
            ContentId::Season { show_id, season } => Ok(self.show_season(show_id, season).await?),
            ContentId::Movie(_) => Ok(DidlResponse::default()),
            ContentId::Episode { .. } => Ok(DidlResponse::default()),
        }
    }

    async fn browse_metadata(&self, object_id: &str) -> Result<DidlResponse, ActionError> {
        let content_id = object_id.parse()?;
        match content_id {
            ContentId::Root => Ok(Self::root_metadata()),
            ContentId::AllMovies => Ok(Self::all_movies_metadata()),
            ContentId::AllShows => Ok(Self::all_shows_metadata()),
            ContentId::Movie(movie_id) => Ok(self.movie_metadata(movie_id).await?),
            ContentId::Show(show_id) => Ok(self.show_metadata(show_id).await?),
            ContentId::Season { show_id, season } => {
                Ok(self.season_metadata(show_id, season).await?)
            }
            ContentId::Episode {
                show_id,
                season,
                episode,
            } => Ok(self.episode_metadata(show_id, season, episode).await?),
        }
    }

    async fn system_update_id(&self) -> u32 {
        self.update_id.load(atomic::Ordering::Acquire)
    }
}
