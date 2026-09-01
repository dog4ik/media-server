use std::{collections::HashMap, path::PathBuf, time::Duration};

use crate::{
    Db, config,
    db::LocalContentId,
    ffmpeg_abi,
    metadata::{
        MovieMetadataProvider, ShowMetadataProvider,
        metadata_api::{
            PendingInsert, ShowLookupMethod,
            asset_saver::AssetTasks,
            movie::MovieMetadataApi,
            show::{
                EpisodeInput, HasSource, LocalTree, SeasonInput, ShowMetadataApi, ShowTree,
                WrittenEpisode,
            },
        },
    },
    parser::{show::ShowIdent, tokenizer::Tokenizer},
    scan::MetadataLookup,
};

use sqlx::types::Json;

#[derive(Debug, Clone, serde::Deserialize)]
struct DbVideoItem {
    id: i64,
    path: PathBuf,
}

impl HasSource for DbVideoItem {
    fn path(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    async fn duration(&self) -> Option<std::time::Duration> {
        ffmpeg_abi::get_metadata(&self.path)
            .await
            .ok()
            .map(|m| m.duration())
    }

    fn fallback_title(&self) -> Option<String> {
        let mut ident = ShowIdent::default();
        ident.apply_name(&mut Tokenizer::new(&self.path.to_string_lossy()));
        Some(ident.title)
    }
}

#[derive(Debug, serde::Deserialize)]
struct EpisodeTreeNode {
    number: usize,
    videos: Vec<DbVideoItem>,
}

impl From<HashMap<usize, Vec<EpisodeTreeNode>>> for ShowTree<DbVideoItem> {
    fn from(value: HashMap<usize, Vec<EpisodeTreeNode>>) -> Self {
        Self {
            seasons: value
                .into_iter()
                .map(|(k, v)| SeasonInput {
                    number: k,
                    episodes: v
                        .into_iter()
                        .map(|v| EpisodeInput {
                            number: v.number,
                            items: v.videos,
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

async fn get_local_show_tree(
    db: &Db,
    show_id: i64,
) -> sqlx::Result<HashMap<usize, Vec<EpisodeTreeNode>>> {
    Ok(sqlx::query!(
        r#"
with episode_videos as (
  select
  episodes.season_id,
  episodes.number as episode_number,
  json_group_array(
      json_object(
          'path', videos.path,
          'id', videos.id
      )
  ) as videos
  from episodes
  join videos on videos.metadata_id = episodes.metadata_id
  group by episodes.id
)
select
seasons.number,
json_group_array(json_object(
    'number', episode_videos.episode_number,
    'videos', json(episode_videos.videos)
)) as "episodes!: Json<Vec<EpisodeTreeNode>>"
from seasons
join episode_videos on episode_videos.season_id = seasons.id
where seasons.show_id = ?
group by seasons.id;"#,
        show_id,
    )
    .fetch_all(&db.pool)
    .await?
    .into_iter()
    .map(|r| (r.number as usize, r.episodes.into_inner()))
    .collect())
}

#[derive(Debug)]
pub struct ShowMetadataFix<T> {
    api: ShowMetadataApi<T>,
}

impl<T> ShowMetadataFix<T>
where
    T: ShowMetadataProvider + Clone + Send + Sync + 'static,
{
    pub fn new(api: ShowMetadataApi<T>) -> Self {
        Self { api }
    }

    pub async fn show_metadata_fix(
        &self,
        show_id: i64,
        target_provider_id: &str,
    ) -> anyhow::Result<()> {
        let config::scan::MaxAssetConcurrency(assets_concurrency) = config::CONFIG.get_value();
        let local_show_tree = get_local_show_tree(&self.api.db, show_id).await?;
        let PendingInsert {
            content,
            mut tx,
            assets,
        } = self
            .api
            .get_or_insert_show_tree(ShowLookupMethod::Id(target_provider_id), local_show_tree)
            .await?;
        for episode in content.episodes() {
            for video in &episode.items {
                sqlx::query!(
                    "update videos set metadata_id = ? where videos.id = ?",
                    episode.metadata_id,
                    video.id,
                )
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        assets.save(assets_concurrency, ()).await;
        Ok(())
    }
}

#[derive(Debug)]
pub struct MovieMetadataFix<T> {
    api: MovieMetadataApi<T>,
}

impl<T> MovieMetadataFix<T>
where
    T: MovieMetadataProvider + Clone + Send + Sync + 'static,
{
    pub fn new(api: MovieMetadataApi<T>) -> Self {
        Self { api }
    }

    /// Move all videos of specified movie to the different movie metadata
    pub async fn movie_metadata_fix(
        &self,
        movie_id: i64,
        target_provider_id: &str,
    ) -> anyhow::Result<()> {
        let config::scan::MaxAssetConcurrency(assets_concurrency) = config::CONFIG.get_value();
        let videos = sqlx::query_scalar!(
            "select videos.id from videos
            join movies on movies.metadata_id = videos.metadata_id
            where movies.id = ?",
            movie_id,
        )
        .fetch_all(&self.api.db.pool)
        .await?;
        let lookup = self.api.search_movie_by_id(target_provider_id).await?;

        let mut tx = self.api.db.pool.begin_with("begin immediate").await?;
        let mut assets = AssetTasks::new(self.api.http_client.clone());
        let saved = self
            .api
            .get_or_insert_lookup(lookup, &mut tx, &mut assets)
            .await?;
        for video_id in videos {
            sqlx::query!(
                "update videos set metadata_id = ? where videos.id = ?",
                saved.metadata_id,
                video_id
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        assets.save(assets_concurrency, ()).await;
        Ok(())
    }
}
