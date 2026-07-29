use crate::{
    db::{Db, DbTransaction, LocalContentId},
    metadata::{
        MovieMetadataProvider, ShowMetadataProvider,
        metadata_api::{
            asset_saver::AssetTasks,
            movie::{BatchMovieApi, MovieMetadataApi},
            show::{BatchShowApi, HasSource, ShowMetadataApi, ShowTree, WrittenShow},
        },
    },
};

pub struct BatchApi<T, S, M = ()> {
    show_batch: BatchShowApi<T, S>,
    movie_batch: BatchMovieApi<M>,
    assets: AssetTasks,
    db: Db,
}

impl<T, S, M> BatchApi<T, S, M>
where
    M: Send + 'static,
    S: Send + 'static,
    T: HasSource + Send + 'static,
{
    pub fn new(db: Db, http_client: reqwest::Client) -> Self {
        Self {
            show_batch: BatchShowApi::new(),
            movie_batch: BatchMovieApi::new(),
            assets: AssetTasks::new(http_client),
            db,
        }
    }

    pub fn spawn_show(
        &mut self,
        api: ShowMetadataApi<&'static (dyn ShowMetadataProvider + Send + Sync + 'static)>,
        show_id: String,
        tree: impl Into<ShowTree<T>>,
        state: S,
    ) {
        self.show_batch.spawn(api, show_id, tree, state);
    }

    pub fn spawn_movie(
        &mut self,
        api: MovieMetadataApi<&'static (dyn MovieMetadataProvider + Send + Sync + 'static)>,
        movie_id: String,
        state: M,
    ) {
        self.movie_batch.spawn(api, movie_id, state);
    }

    pub async fn join_all<C, MovieFn, ShowFn>(
        mut self,
        ctx: &mut C,
        mut join_movie: MovieFn,
        mut join_show: ShowFn,
    ) -> anyhow::Result<()>
    where
        MovieFn: for<'a> FnMut(
            LocalContentId,
            &'a mut DbTransaction,
            M,
            &'a mut C,
        ) -> crate::BoxedFuture<'a, anyhow::Result<()>>,
        ShowFn: for<'a> FnMut(
            WrittenShow<T>,
            &'a mut DbTransaction,
            S,
            &'a mut C,
        ) -> crate::BoxedFuture<'a, anyhow::Result<()>>,
    {
        let movies = self.movie_batch.join_set.join_all().await;
        let shows = self.show_batch.join_set.join_all().await;

        let mut tx = self.db.pool.begin_with("BEGIN IMMEDIATE").await?;
        for super::movie::BatchResult {
            resolved,
            api,
            state,
        } in movies.into_iter().filter_map(Result::ok)
        {
            let local_id = api
                .get_or_insert_lookup(resolved, &mut tx, &mut self.assets)
                .await?;
            join_movie(local_id, &mut tx, state, ctx).await?;
        }

        for super::show::BatchResult {
            resolved,
            api,
            state,
        } in shows.into_iter().filter_map(Result::ok)
        {
            let resolved_tree = api
                .flush_show_tree(&mut tx, &mut self.assets, resolved)
                .await?;
            join_show(resolved_tree, &mut tx, state, ctx).await?;
        }
        tx.commit().await?;
        self.assets.save(16, ()).await;
        Ok(())
    }
}
