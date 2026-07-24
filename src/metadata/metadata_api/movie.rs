use std::time::Duration;

use crate::{
    config,
    db::{Db, DbActions, DbExternalId, DbTransaction, LocalContentId},
    library::assets::{BackdropAsset, BackdropContentType, PosterAsset, PosterContentType},
    metadata::{
        ExternalIdMetadata, FetchParams, MovieMetadata, MovieMetadataProvider,
        metadata_api::asset_saver::AssetTasks,
    },
    scan::{AssetKind, AssetSaveTask, AssetTaskSource, insert_roles},
};

use super::{MetadataLookup, PendingInsert};

/// Resolves a movie against a single metadata provider, reusing local database metadata when
/// it already exists and fetching from the provider otherwise.
#[derive(Debug, Clone)]
pub struct MovieMetadataApi<T> {
    provider: T,
    fetch_params: FetchParams,
    db: &'static Db,
    http_client: reqwest::Client,
}

#[cfg(test)]
impl MovieMetadataApi<crate::metadata::metadata_api::tests::provider_mock::MockProvider> {
    pub fn new_test(
        provider: crate::metadata::metadata_api::tests::provider_mock::MockProvider,
        db: &'static Db,
    ) -> Self {
        let fetch_params = FetchParams::default();

        Self {
            provider,
            db,
            fetch_params,
            http_client: reqwest::Client::new(),
        }
    }
}

impl<T> MovieMetadataApi<T>
where
    T: MovieMetadataProvider,
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

    pub async fn search_movie_title(
        &self,
        title: &str,
    ) -> anyhow::Result<Option<MetadataLookup<MovieMetadata>>> {
        let search_results = self.provider.movie_search(title, self.fetch_params).await?;
        let Some(first_result) = search_results.into_iter().next() else {
            return Ok(None);
        };
        match self
            .db
            .crossreference_movie(first_result.metadata_provider, &first_result.metadata_id)
            .await
        {
            Ok(Some(local)) => Ok(Some(MetadataLookup::Local(local))),
            Ok(None) | Err(_) => {
                let mut metadata = self
                    .provider
                    .movie(&first_result.metadata_id, self.fetch_params)
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

    pub async fn search_movie_by_id(
        &self,
        id: &str,
    ) -> anyhow::Result<MetadataLookup<MovieMetadata>> {
        match self
            .db
            .crossreference_movie(self.provider.provider_identifier(), &id)
            .await
        {
            Ok(Some(local)) => Ok(MetadataLookup::Local(local)),
            Ok(None) | Err(_) => {
                let metadata = self.provider.movie(id, self.fetch_params).await?;
                Ok(MetadataLookup::New { metadata })
            }
        }
    }

    /// Ensures the movie identified by `id` exists locally, resolving the prime metadata provider
    /// and inserting it if absent.
    pub async fn get_or_insert_movie(
        &self,
        id: &str,
    ) -> anyhow::Result<PendingInsert<LocalContentId>> {
        for _ in 0..crate::db::MAX_INSERT_RETRIES {
            let movie = self.search_movie_by_id(id).await?;
            let mut tx = self.db.pool.begin_with("BEGIN IMMEDIATE").await?;
            let mut assets = AssetTasks::new(self.http_client.clone());
            let outcome: sqlx::Result<LocalContentId> = match movie {
                MetadataLookup::Local(local) => Ok(local),
                MetadataLookup::New { metadata } => {
                    self.insert_movie_metadata(metadata, &mut tx, &mut assets)
                        .await
                }
                MetadataLookup::Missing => anyhow::bail!("requested movie metadata not found"),
            };
            match outcome {
                Ok(content) => {
                    return Ok(PendingInsert {
                        content,
                        tx,
                        assets,
                    });
                }
                Err(e) if crate::db::is_unique_violation(&e) => {
                    tracing::debug!("Concurrent movie insert detected, retrying");
                }
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::bail!("could not insert movie after concurrent insert retries")
    }

    pub async fn insert_movie_metadata(
        &self,
        metadata: MovieMetadata,
        tx: &mut DbTransaction,
        assets: &mut AssetTasks,
    ) -> sqlx::Result<LocalContentId> {
        let poster = metadata.poster.clone();
        let backdrop = metadata.backdrop.clone();
        let metadata_id = tx.insert_metadata(&metadata.into_db_metadata()).await?;
        let movie_id = tx
            .insert_movie(&metadata.into_db_movie(
                metadata_id,
                metadata.runtime.as_ref().map_or(Duration::ZERO, |v| v.0),
            ))
            .await?;
        if let Some(cast) = metadata.cast {
            insert_roles(tx, metadata_id, cast, assets).await?;
        }
        tx.insert_external_id(DbExternalId {
            id: None,
            external_provider: metadata.metadata_provider,
            external_id: metadata.metadata_id,
            metadata_id: Some(metadata_id),
            is_prime: true.into(),
        })
        .await?;
        for ext_id in metadata.external_ids.iter().flatten() {
            tx.try_insert_external_id(DbExternalId {
                external_provider: ext_id.provider,
                external_id: ext_id.id.clone(),
                metadata_id: Some(metadata_id),
                is_prime: (metadata.metadata_provider == ext_id.provider).into(),
                ..Default::default()
            })
            .await?;
        }
        for genre in metadata.genres.into_iter().flatten() {
            let _ = tx.insert_content_genre(metadata_id, genre.into()).await;
        }
        if let Some(url) = poster {
            let task_source = AssetTaskSource::Url(url);
            assets.push(AssetSaveTask {
                kind: AssetKind::Poster(PosterAsset::new(movie_id, PosterContentType::Movie)),
                source: task_source,
            });
        }
        if let Some(url) = backdrop {
            assets.push(AssetSaveTask {
                kind: AssetKind::Backdrop(BackdropAsset::new(movie_id, BackdropContentType::Movie)),
                source: AssetTaskSource::Url(url),
            });
        }
        Ok(LocalContentId {
            id: movie_id,
            metadata_id,
        })
    }
}
