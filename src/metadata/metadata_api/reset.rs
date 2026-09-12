use std::path::PathBuf;

use anyhow::Context;
use tokio::task::JoinSet;

use crate::{
    Db, config,
    db::{DbVideo, LocalContentId},
    library::Media,
    metadata::{
        ShowMetadata, ShowMetadataProvider,
        metadata_api::{
            MetadataLookup,
            bucket::{self, BucketItem},
            show::ShowMetadataApi,
        },
    },
    parser::show::ShowIdentifier,
};

#[derive(Debug, Clone)]
struct ShowVideoItem {
    id: i64,
    path: PathBuf,
    identifier: ShowIdentifier,
}

impl BucketItem for ShowVideoItem {
    fn title(&self) -> &str {
        &self.identifier.title
    }

    fn year(&self) -> Option<u16> {
        self.identifier.year
    }
}

/// Procduce groups of show items grouped by title
///
/// These buckets can be used
async fn bucket_show_videos(db: &Db, show_id: i64) -> sqlx::Result<Vec<Vec<ShowVideoItem>>> {
    #[derive(Debug, sqlx::FromRow)]
    pub struct DbShowVideo {
        path: PathBuf,
        id: i64,
        season: i64,
        episode: i64,
    }
    let videos = sqlx::query_as!(
            DbShowVideo,
            "select videos.path, videos.id, seasons.number as season, episodes.number as episode from shows
    join seasons on seasons.show_id = shows.id
    join episodes on episodes.season_id = seasons.id
    join videos on videos.metadata_id = episodes.metadata_id
        where shows.id = ?",
              show_id
    )
        .fetch_all(&db.pool)
        .await?
        .into_iter()
        .map(|DbShowVideo { path, id, season, episode }| {
            let identifier = ShowIdentifier::from_path(&path)
                .unwrap_or_else(|ident| {
                    tracing::warn!(path = %path.display(), "Failed to create identifier from path for episode");
                    ShowIdentifier {
                        episode: ident.episode.unwrap_or(episode as u16),
                        season: ident.season.unwrap_or(season as u16),
                        title: ident.title,
                        year: ident.year,
                        attributes: ident.attributes
                    }
                });
            ShowVideoItem {
                id,
                identifier,
                path,
            }
        });
    let buckets = bucket::bucket_items(videos);
    Ok(buckets.into_values().collect())
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

    pub async fn fix(&self, show_id: i64) -> anyhow::Result<()> {
        let buckets = bucket_show_videos(self.api.db, show_id).await?;
        let mut set = JoinSet::new();
        for bucket in buckets {
            let api = self.api.clone();
            set.spawn(async move {
                let title = bucket[0].title();
                api.search_show_title::<LocalContentId>(title).await
            });
        }
        let items = set
            .join_all()
            .await
            .into_iter()
            .map(|v| v)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(())
    }
}
