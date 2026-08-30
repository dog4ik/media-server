use crate::{
    api::api_data::local_show::{Episode, Season, Show},
    db::{Db, DbActions, DbQueryBuilder, LocalContentId, query_builders},
    metadata::MetadataProvider,
};

/// Trait that is implemented by all local objects connected to the metadata
/// e.g. show, season, episode, movie.
pub trait LocalMetadataIdentifier {
    fn local_id(&self) -> LocalContentId;
}

impl LocalMetadataIdentifier for LocalContentId {
    fn local_id(&self) -> LocalContentId {
        *self
    }
}

impl LocalMetadataIdentifier for Show {
    fn local_id(&self) -> LocalContentId {
        let local = self
            .local
            .as_ref()
            .expect("show object must be local for the valid usage of this trait");
        LocalContentId {
            id: local.id,
            metadata_id: local.metadata_id,
        }
    }
}

impl LocalMetadataIdentifier for Episode {
    fn local_id(&self) -> LocalContentId {
        let local = self
            .local
            .as_ref()
            .expect("episode object must be local for the valid usage of this trait");
        LocalContentId {
            id: local.id,
            metadata_id: local.metadata_id,
        }
    }
}

impl LocalMetadataIdentifier for Season {
    fn local_id(&self) -> LocalContentId {
        let local = self
            .local
            .as_ref()
            .expect("season object must be local for the valid usage of this trait");
        LocalContentId {
            id: local.id,
            metadata_id: local.metadata_id,
        }
    }
}

/// Marker trait that selects which local representation a show tree resolves its nodes into.
///
/// Picking a scope at the call site decides
/// whether a locally known show/season/episode comes back as a bare
/// [LocalContentId] or as a full api object, so callers that need the
/// latter do not have to re-query after resolving.
pub trait LocalLookupScope {
    type Show: LocalMetadataIdentifier + Send + 'static;
    type Season: LocalMetadataIdentifier + Send + 'static;
    type Episode: LocalMetadataIdentifier + Send + 'static;

    fn show_lookup(
        db: &Db,
        provider: MetadataProvider,
        metadata_id: &str,
    ) -> impl Future<Output = sqlx::Result<Option<Self::Show>>> + Send;

    /// Load the show's seasons
    fn seasons_lookup(
        db: &Db,
        show_id: i64,
        seasons_scope: Vec<usize>,
    ) -> impl Future<Output = sqlx::Result<impl IntoIterator<Item = (usize, Self::Season)> + Send>> + Send;

    /// Loads the show's episodes as `(season number, episode number, local)`
    fn episodes_lookup(
        db: &Db,
        show_id: i64,
        episodes_scope: Vec<(usize, usize)>,
    ) -> impl Future<
        Output = sqlx::Result<impl IntoIterator<Item = (usize, usize, Self::Episode)> + Send>,
    > + Send;
}

/// Id-only scope
impl LocalLookupScope for LocalContentId {
    type Show = Self;
    type Season = Self;
    type Episode = Self;

    async fn show_lookup(
        db: &Db,
        provider: MetadataProvider,
        metadata_id: &str,
    ) -> sqlx::Result<Option<Self::Show>> {
        db.crossreference_show(provider, metadata_id).await
    }

    async fn seasons_lookup(
        db: &Db,
        show_id: i64,
        seasons_scope: Vec<usize>,
    ) -> sqlx::Result<impl IntoIterator<Item = (usize, Self::Season)>> {
        let seasons = db.get_show_season_nodes(show_id).await?;
        Ok(seasons
            .into_iter()
            .map(|s| {
                (
                    s.number as usize,
                    LocalContentId {
                        id: s.id,
                        metadata_id: s.metadata_id,
                    },
                )
            })
            .filter(move |(number, _)| seasons_scope.is_empty() || seasons_scope.contains(number)))
    }

    async fn episodes_lookup(
        db: &Db,
        show_id: i64,
        episodes_scope: Vec<(usize, usize)>,
    ) -> sqlx::Result<impl IntoIterator<Item = (usize, usize, Self::Episode)> + Send> {
        let episodes = db.get_show_episode_nodes(show_id).await?;
        Ok(episodes
            .into_iter()
            .map(|s| {
                (
                    s.season_number as usize,
                    s.number as usize,
                    LocalContentId {
                        id: s.id,
                        metadata_id: s.metadata_id,
                    },
                )
            })
            .filter(move |(season, episode, _)| {
                episodes_scope.is_empty() || episodes_scope.contains(&(*season, *episode))
            }))
    }
}

/// Scope that captures full api objects in local metadata
///
/// Useful when full metadata is needed after resolving the tree
pub struct ApiObjectScope;

impl LocalLookupScope for ApiObjectScope {
    type Show = Show;
    type Season = Season;
    type Episode = Episode;

    async fn show_lookup(
        db: &Db,
        provider: MetadataProvider,
        metadata_id: &str,
    ) -> sqlx::Result<Option<Self::Show>> {
        let mut query = DbQueryBuilder::default();
        query_builders::DbShowQuery::build(&mut query);
        query
            .push(
                " where shows.metadata_id in
                (select external_ids.metadata_id from external_ids
                where external_ids.external_provider = ",
            )
            .push_bind(provider.to_string())
            .push(" and external_ids.external_id = ")
            .push_bind(metadata_id.to_string())
            .push(")");
        Ok(query
            .build_query_as::<query_builders::DbShowQuery>()
            .fetch_optional(&db.pool)
            .await?
            .map(Into::into))
    }

    async fn seasons_lookup(
        db: &Db,
        show_id: i64,
        seasons_scope: Vec<usize>,
    ) -> sqlx::Result<impl IntoIterator<Item = (usize, Self::Season)> + Send> {
        let mut query = DbQueryBuilder::default();
        query_builders::DbSeasonQuery::build(&mut query);
        query.push(" where seasons.show_id = ").push_bind(show_id);
        if !seasons_scope.is_empty() {
            query.push(" and seasons.number in (");
            let mut numbers = query.separated(", ");
            for number in seasons_scope {
                numbers.push_bind(number as i64);
            }
            query.push(")");
        }
        Ok(query
            .build_query_as::<query_builders::DbSeasonQuery>()
            .fetch_all(&db.pool)
            .await?
            .into_iter()
            .map(|season| (season.season.number as usize, season.into())))
    }

    async fn episodes_lookup(
        db: &Db,
        show_id: i64,
        episodes_scope: Vec<(usize, usize)>,
    ) -> sqlx::Result<impl IntoIterator<Item = (usize, usize, Self::Episode)> + Send> {
        let mut query = DbQueryBuilder::default();
        query_builders::DbEpisodeQuery::build(&mut query);
        query.push(" where seasons.show_id = ").push_bind(show_id);
        if !episodes_scope.is_empty() {
            query.push(" and (seasons.number, episodes.number) in ");
            query.push_tuples(episodes_scope, |mut tuple, (season, episode)| {
                tuple.push_bind(season as i64);
                tuple.push_bind(episode as i64);
            });
        }
        Ok(query
            .build_query_as::<query_builders::DbEpisodeQuery>()
            .fetch_all(&db.pool)
            .await?
            .into_iter()
            .map(|episode| {
                let season_number = episode.season_number as usize;
                let episode_number = episode.episode.number as usize;
                (season_number, episode_number, episode.into())
            }))
    }
}

/// Scope that never resolves local metadata
///
/// Useful only when fresh tree is required
#[allow(dead_code)]
pub struct NeverScope;

#[allow(dead_code)]
impl LocalLookupScope for NeverScope {
    type Show = LocalContentId;
    type Season = LocalContentId;
    type Episode = LocalContentId;

    async fn show_lookup(
        _db: &Db,
        _provider: MetadataProvider,
        _metadata_id: &str,
    ) -> sqlx::Result<Option<Self::Show>> {
        Ok(None)
    }

    async fn seasons_lookup(
        _db: &Db,
        _show_id: i64,
        _seasons_scope: Vec<usize>,
    ) -> sqlx::Result<impl IntoIterator<Item = (usize, Self::Season)> + Send> {
        Ok([])
    }

    async fn episodes_lookup(
        _db: &Db,
        _show_id: i64,
        _episodes_scope: Vec<(usize, usize)>,
    ) -> sqlx::Result<impl IntoIterator<Item = (usize, usize, Self::Episode)> + Send> {
        Ok([])
    }
}
