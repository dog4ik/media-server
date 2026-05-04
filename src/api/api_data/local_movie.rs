use serde::Serialize;

use crate::{
    api::api_data::{
        LocalDataLookup,
        api_types::{Actor, History},
    },
    metadata::{ExternalIdMetadata, LocaleMetadata, MetadataProvider, MovieMetadata},
};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Movie {
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    pub runtime: Option<crate::MediaDuration>,
    pub title: String,
    pub cast: Option<Vec<Actor>>,
    pub external_ids: Option<Vec<ExternalIdMetadata>>,
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
            cast: None,
            external_ids: None,
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
        mut meta: MovieMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        let local = lookup
            .movie_data(meta.metadata_provider, &meta.metadata_id)
            .await?;
        let cast = if let Some(cast) = std::mem::take(&mut meta.cast) {
            Some(lookup.extend_actors(cast).await?)
        } else {
            None
        };
        let mut extended_movie = Self::extend_meta(meta, local);
        extended_movie.cast = cast;
        Ok(extended_movie)
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
            cast: None,
            external_ids: meta.external_ids,
            local,
        }
    }
}
