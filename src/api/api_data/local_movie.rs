use serde::Serialize;

use crate::{
    api::api_data::{
        LocalDataLookup,
        api_types::{Actor, CompactList, History},
    },
    db::{Db, DbQueryBuilder, query_builders},
    metadata::{
        ExternalIdMetadata, Genre, LocaleMetadata, MetadataProvider, MovieMetadata,
        metadata_api::MetadataLookup,
    },
};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Movie {
    pub provider_id: String,
    pub provider: MetadataProvider,
    pub poster: Option<String>,
    pub backdrop: Option<String>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    pub runtime: Option<crate::MediaDuration>,
    pub title: String,
    pub cast: Option<Vec<Actor>>,
    pub external_ids: Option<Vec<ExternalIdMetadata>>,
    pub genres: Option<Vec<Genre>>,
    pub locale_metadata: Option<LocaleMetadata>,
    pub local: Option<LocalMovieData>,
}

impl From<Movie> for MovieMetadata {
    fn from(value: Movie) -> Self {
        Self {
            metadata_id: value.provider_id,
            metadata_provider: value.provider,
            poster: value.poster,
            backdrop: value.backdrop,
            plot: value.plot,
            release_date: value.release_date,
            runtime: value.runtime,
            title: value.title,
            locale_metadata: value.locale_metadata,
            genres: None,
            cast: None,
            external_ids: None,
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct LocalMovieData {
    pub id: i64,
    pub metadata_id: i64,
    pub local_duration: crate::MediaDuration,
    pub videos_count: i64,
    pub lists: Vec<CompactList>,
    pub history: Option<History>,
}

impl Movie {
    pub async fn extend_with_lookup(
        mut meta: MovieMetadata,
        lookup: LocalDataLookup,
    ) -> sqlx::Result<Self> {
        // TODO: consider handling cast in extend_movies call
        let cast = if let Some(cast) = std::mem::take(&mut meta.cast) {
            Some(lookup.extend_actors(cast).await?)
        } else {
            None
        };
        let mut extended_movie = lookup
            .extend_movies_with_local_data(vec![meta])
            .await?
            .into_iter()
            .next()
            .expect("input length should match output");
        extended_movie.cast = cast;
        Ok(extended_movie)
    }

    pub async fn from_lookup(lookup: MetadataLookup<MovieMetadata>, db: Db) -> sqlx::Result<Self> {
        match lookup {
            MetadataLookup::New { metadata } => {
                Self::extend_with_lookup(metadata, LocalDataLookup { db }).await
            }
            MetadataLookup::Local(local_content_id) => {
                let mut query = DbQueryBuilder::default();
                query_builders::DbMovieQuery::build(&mut query);
                query
                    .push(" where metadata.id = ")
                    .push_bind(local_content_id.metadata_id)
                    .build_query_as::<query_builders::DbMovieQuery>()
                    .fetch_one(&db.pool)
                    .await
                    .map(Into::into)
            }
            MetadataLookup::Missing => Err(sqlx::Error::RowNotFound),
        }
    }
}
