use std::{fmt::Display, str::FromStr};

use anyhow::Context;
use upnp::{
    action::ActionError,
    content_directory::{
        properties::{self, upnp_class::ItemType, Container, DidlResponse, Item},
        ContentDirectoryHandler, ProtocolInfo, Resource,
    },
};

use crate::db::Db;

#[derive(Clone)]
pub struct MediaServerContentDirectory {
    db: Db,
    server_location: String,
}

impl MediaServerContentDirectory {
    pub fn new(db: Db, server_location: String) -> Self {
        Self {
            db,
            server_location,
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

    pub async fn all_shows(&self) -> anyhow::Result<DidlResponse> {
        let shows = self.db.all_shows().await?;
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
            containers.push(container);
        }
        Ok(DidlResponse {
            containers,
            items: vec![],
        })
    }

    pub async fn show(&self, show_id: i64) -> anyhow::Result<DidlResponse> {
        let show = self.db.get_show(show_id).await?;
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
        let season_metadata = self.db.get_season(show_id, season as usize).await?;
        let episodes = season_metadata.episodes;
        let mut items = Vec::with_capacity(episodes.len());
        for episode in episodes {
            let poster_url = format!(
                "{server_url}/api/episode/{episode_id}/poster",
                server_url = self.server_location,
                episode_id = episode.metadata_id,
            );
            let watch_url = format!(
                "{server_url}/api/local_episode/{episode_id}/watch",
                server_url = self.server_location,
                episode_id = episode.metadata_id
            );
            let season_id = ContentId::Season { show_id, season };
            let container_id =
                format!("{season_id}.{episode_id}", episode_id = episode.metadata_id);
            let mut item = Item::new(
                container_id,
                ContentId::Season { show_id, season }.to_string(),
                episode.title.clone(),
            );
            item.set_property(properties::AlbumArtUri(poster_url));
            item.set_property(properties::ProgramTitle(episode.title));
            item.set_property(properties::EpisodeNumber(episode.number as u32));
            item.set_property(properties::EpisodeSeason(season as u32));
            if let Some(description) = episode.plot {
                item.set_property(properties::LongDescription(description));
            }
            let watch_resource =
                Resource::new(watch_url, ProtocolInfo::http_get("video/matroska".into()));
            item.set_property(watch_resource);
            item.base.set_upnp_class(ItemType::VideoItem(None));
            items.push(item);
        }
        Ok(DidlResponse {
            containers: vec![],
            items,
        })
    }

    pub async fn all_movies(&self) -> anyhow::Result<DidlResponse> {
        let movies = self.db.all_movies().await?;
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
            let container_id = format!("movie.{}", movie.metadata_id);
            let mut item = Item::new(container_id, "movies".into(), movie.title);
            item.base.set_upnp_class(Some(ItemType::VideoItem(None)));
            item.set_property(properties::AlbumArtUri(poster_url));
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
}

#[derive(Debug, Clone, Copy)]
enum ContentId {
    Root,
    AllMovies,
    AllShows,
    Show(i64),
    Season { show_id: i64, season: i64 },
}

impl Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContentId::Root => write!(f, "0"),
            ContentId::AllMovies => write!(f, "movies"),
            ContentId::AllShows => write!(f, "shows"),
            ContentId::Show(id) => write!(f, "show.{id}"),
            ContentId::Season { show_id, season } => write!(f, "show.{show_id}.{season}"),
        }
    }
}

impl FromStr for ContentId {
    type Err = anyhow::Error;

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
            if let Some((show_id, season)) = show.split_once('.') {
                let show_id = show_id.parse().context("parse show id")?;
                let season = season.parse().context("parse season")?;
                return Ok(Self::Season { show_id, season });
            } else {
                let show_id = show.parse().context("parse show id")?;
                return Ok(Self::Show(show_id));
            }
        }
        Err(anyhow::anyhow!("failed to parse content id: {s}"))
    }
}

impl ContentDirectoryHandler for MediaServerContentDirectory {
    async fn browse_direct_children(
        &self,
        object_id: &str,
        requested_count: u32,
    ) -> Result<DidlResponse, ActionError> {
        let content_id = object_id.parse()?;
        match content_id {
            ContentId::Root => return Ok(Self::root()),
            ContentId::AllMovies => return Ok(self.all_movies().await?),
            ContentId::AllShows => return Ok(self.all_shows().await?),
            ContentId::Show(id) => return Ok(self.show(id).await?),
            ContentId::Season { show_id, season } => {
                return Ok(self.show_season(show_id, season).await?)
            }
        }
    }

    async fn browse_metadata(&self, object_id: &str) -> Result<DidlResponse, ActionError> {
        todo!()
    }
}
