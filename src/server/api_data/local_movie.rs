use std::time::Duration;

use serde::Serialize;

use crate::{
    metadata::{LocaleMetadata, MetadataImage, MetadataProvider, MovieMetadata},
    server::api_data::{LocalDataLookup, api_types::History},
};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Movie {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub poster: Option<MetadataImage>,
    pub backdrop: Option<MetadataImage>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    pub runtime: Option<Duration>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
    pub local: Option<LocalMovieData>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalMovieData {
    pub local_id: i64,
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
