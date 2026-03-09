use std::time::Duration;

use serde::Serialize;

use crate::{
    api::api_data::{LocalDataLookup, api_types::History},
    metadata::{LocaleMetadata, MetadataImage, MetadataProvider, MovieMetadata},
};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Movie {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub poster: Option<MetadataImage>,
    pub backdrop: Option<MetadataImage>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    #[schema(value_type = Option<crate::api::SerdeDuration>)]
    pub runtime: Option<Duration>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
    pub local: Option<LocalMovieData>,
}

impl From<Movie> for MovieMetadata {
    fn from(value: Movie) -> Self {
        Self {
            metadata_id: value.metadata_id,
            metadata_provider: value.metadata_provider,
            poster: value.poster,
            backdrop: value.backdrop,
            plot: value.plot,
            release_date: value.release_date,
            runtime: value.runtime,
            title: value.title,
            locale_metadata: value.locale_metadata,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalMovieData {
    pub id: i64,
    pub history: Option<History>,
}

impl Movie {
    pub async fn extend_with_lookup(
        meta: MovieMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        let local = lookup
            .movie_data(meta.metadata_provider, &meta.metadata_id)
            .await?;

        Ok(Self::extend_meta(meta, local))
    }

    pub fn extend_meta(meta: MovieMetadata, local: Option<LocalMovieData>) -> Self {
        Self {
            metadata_id: meta.metadata_id,
            metadata_provider: meta.metadata_provider,
            poster: meta.poster,
            backdrop: meta.backdrop,
            plot: meta.plot,
            release_date: meta.release_date,
            runtime: meta.runtime,
            title: meta.title,
            locale_metadata: meta.locale_metadata,
            local,
        }
    }
}
