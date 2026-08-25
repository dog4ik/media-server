use serde::Serialize;

use crate::{
    api::{
        api_data::{
            LocalDataLookup,
            api_types::{Actor, CompactList},
        },
        server::Intro,
    },
    metadata::{
        EpisodeMetadata, ExternalIdMetadata, Genre, LocaleMetadata, MetadataProvider,
        SeasonMetadata, ShowMetadata, metadata_api::MetadataLookup,
    },
};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalShowData {
    pub id: i64,
    pub lists: Vec<CompactList>,
    pub metadata_id: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalSeasonData {
    pub id: i64,
    pub metadata_id: i64,
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct LocalEpisodeData {
    pub id: i64,
    pub metadata_id: i64,
    pub lists: Vec<CompactList>,
    pub videos_count: i64,
    pub history: Option<super::api_types::History>,
    pub intro: Option<Intro>,
}

/// Show API data structure
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Show {
    pub provider_id: String,
    pub provider: MetadataProvider,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub plot: Option<String>,
    /// Array of available season numbers
    pub seasons: Option<Vec<usize>>,
    pub episodes_amount: Option<usize>,
    pub release_date: Option<String>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
    pub cast: Option<Vec<Actor>>,
    pub external_ids: Option<Vec<ExternalIdMetadata>>,
    pub genres: Option<Vec<Genre>>,
    pub next_episode_air_date: Option<crate::OffsetDateTime>,
    pub local: Option<LocalShowData>,
}

impl From<ShowMetadata> for Show {
    fn from(
        ShowMetadata {
            metadata_id,
            metadata_provider,
            poster,
            backdrop,
            plot,
            seasons,
            episodes_amount,
            release_date,
            title,
            locale_metadata,
            cast,
            genres,
            next_episode_air_date,
            external_ids,
        }: ShowMetadata,
    ) -> Self {
        Self {
            provider_id: metadata_id,
            provider: metadata_provider,
            poster,
            backdrop,
            plot,
            seasons,
            episodes_amount,
            release_date,
            title,
            locale_metadata,
            cast: cast.map(|v| v.into_iter().map(Into::into).collect()),
            external_ids,
            genres,
            next_episode_air_date,
            local: None,
        }
    }
}

impl Show {
    pub async fn extend_with_lookup(
        meta: ShowMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        let extended_show = lookup
            .extend_shows_with_local_data(vec![meta])
            .await?
            .into_iter()
            .next()
            .expect("input length should match output");

        Ok(extended_show)
    }

    pub fn from_lookup(lookup: MetadataLookup<ShowMetadata, Show>) -> Option<Self> {
        match lookup {
            MetadataLookup::New { metadata } => Some(Show::from(metadata)),
            MetadataLookup::Local(show) => Some(show),
            MetadataLookup::Missing => None,
        }
    }
}

/// Season API data structure
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Season {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub release_date: Option<String>,
    pub title: Option<String>,
    pub episodes: Vec<Episode>,
    pub plot: Option<String>,
    pub poster: Option<String>,
    pub number: usize,
    pub local: Option<LocalSeasonData>,
}

impl Season {
    pub async fn extend_from_metadata(
        meta: SeasonMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        let local = lookup
            .season_data(meta.metadata_provider, &meta.metadata_id, meta.number)
            .await?;
        let episodes = lookup
            .extend_episodes_with_local_data(meta.episodes)
            .await?;

        Ok(Self {
            metadata_id: meta.metadata_id,
            metadata_provider: meta.metadata_provider,
            release_date: meta.release_date,
            title: meta.title,
            episodes,
            plot: meta.plot,
            poster: meta.poster,
            number: meta.number,
            local,
        })
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Episode {
    pub provider_id: String,
    pub provider: MetadataProvider,
    pub release_date: Option<String>,
    pub number: usize,
    pub title: String,
    pub plot: Option<String>,
    pub season_number: usize,
    pub runtime: Option<crate::MediaDuration>,
    pub poster: Option<String>,
    pub cast: Option<Vec<Actor>>,
    pub local: Option<LocalEpisodeData>,
}

impl From<EpisodeMetadata> for Episode {
    fn from(
        EpisodeMetadata {
            metadata_id,
            metadata_provider,
            release_date,
            number,
            title,
            plot,
            season_number,
            runtime,
            poster,
            cast,
        }: EpisodeMetadata,
    ) -> Self {
        Self {
            provider_id: metadata_id,
            provider: metadata_provider,
            release_date,
            number,
            title,
            plot,
            season_number,
            runtime,
            poster,
            cast: cast.map(|cast| cast.into_iter().map(Into::into).collect()),
            local: None,
        }
    }
}

impl Episode {
    pub async fn extend_from_metadata(
        mut meta: EpisodeMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        let cast = if let Some(cast) = std::mem::take(&mut meta.cast) {
            Some(lookup.extend_actors(cast).await?)
        } else {
            None
        };
        let mut extended_episode = lookup
            .extend_episodes_with_local_data(vec![meta])
            .await?
            .into_iter()
            .next()
            .expect("input length must match output");
        extended_episode.cast = cast;
        Ok(extended_episode)
    }

    /// Convert lookup that has enough data to episode object.
    ///
    /// Note that if lookup variant is [MetadataLookup::New], cast will not contain any local references.
    pub fn from_lookup(lookup: MetadataLookup<EpisodeMetadata, Episode>) -> Option<Self> {
        match lookup {
            MetadataLookup::New { metadata } => Some(metadata.into()),
            MetadataLookup::Local(episode) => Some(episode),
            MetadataLookup::Missing => None,
        }
    }
}
