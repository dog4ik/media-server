use std::path::{Path, PathBuf};

use tokio::fs;
use tracing::Instrument;

use crate::{
    config::{self, APP_RESOURCES},
    db::Db,
};

mod disks;

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct MediaDirContent {
    pub metadata_id: i64,
    /// Id of the content, movies.id or shows.id
    pub id: i64,
    pub size: u64,
    pub title: String,
    pub poster: Option<String>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct MediaDirStats {
    #[schema(value_type = String)]
    pub path: PathBuf,
    /// Contents of the directory, ordered by size in descending order (heavy first)
    pub contents: Vec<MediaDirContent>,
    /// Total size of the directory including files other than content
    pub size: u64,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct Resources {
    #[schema(value_type = String)]
    pub db_path: PathBuf,
    pub db_size: u64,
    #[schema(value_type = String)]
    pub tmp_path: PathBuf,
    pub tmp_size: u64,
    #[schema(value_type = String)]
    pub resources_path: PathBuf,
    pub resources_size: u64,
    pub show_media_dirs: Vec<MediaDirStats>,
    pub movie_media_dirs: Vec<MediaDirStats>,
    #[schema(value_type = String)]
    pub config_path: PathBuf,
    /// Stored but not used metadata items
    pub metadata_orphan_count: i64,
    pub disks: Vec<disks::Disk>,
}

#[tracing::instrument]
async fn dir_contents_len(path: &Path) -> std::io::Result<u64> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || dir_contents_len_sync(&path))
        .await
        .expect("blocking task panicked")
}

fn dir_contents_len_sync(path: &Path) -> std::io::Result<u64> {
    let mut total = 0;
    let mut stack = vec![std::fs::read_dir(path)?];
    while let Some(iter) = stack.last_mut() {
        match iter.next() {
            None => {
                stack.pop();
            }
            Some(Err(_)) => continue,
            Some(Ok(entry)) => {
                let ft = entry.file_type()?;
                if ft.is_dir() {
                    stack.push(std::fs::read_dir(entry.path())?);
                } else if ft.is_file() {
                    total += entry.metadata()?.len();
                }
            }
        }
    }
    Ok(total)
}

#[derive(Debug, sqlx::FromRow)]
struct ContentQuery {
    metadata_id: i64,
    id: i64,
    title: String,
    poster: Option<String>,
    size: i64,
}

impl From<ContentQuery> for MediaDirContent {
    fn from(
        ContentQuery {
            metadata_id,
            id,
            title,
            poster,
            size,
        }: ContentQuery,
    ) -> Self {
        Self {
            metadata_id,
            id,
            size: size as u64,
            title,
            poster,
        }
    }
}

#[tracing::instrument(name = "fetch_resources", skip_all)]
pub async fn fetch(db: Db) -> anyhow::Result<Resources> {
    let (config::MovieFolders(movie_dirs), config::ShowFolders(show_dirs)) =
        config::CONFIG.get_values();
    let db_meta = fs::metadata(&APP_RESOURCES.database_path).await?;
    let mut movie_media_dirs = Vec::with_capacity(movie_dirs.len());
    let mut show_media_dirs = Vec::with_capacity(show_dirs.len());
    let tmp_size = dir_contents_len(&APP_RESOURCES.temp_path).await?;
    let resources_size = dir_contents_len(&APP_RESOURCES.resources_path).await?;

    for dir in movie_dirs {
        let contents = sqlx::query_as!(
            ContentQuery,
            r#"select
            metadata.id as metadata_id, movies.id, metadata.title, metadata.poster, sum(videos.size) as "size"
            from videos
            join movies on movies.metadata_id = videos.metadata_id
            join metadata on metadata.id = videos.metadata_id
            where videos.path like ? || '%'
            group by videos.metadata_id order by size desc"#,
            &dir.display().to_string()
        )
        .fetch_all(&db.pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

        let size = dir_contents_len(&dir).await?;
        movie_media_dirs.push(MediaDirStats {
            path: dir,
            size,
            contents,
        })
    }

    for dir in show_dirs {
        let contents = sqlx::query_as!(
            ContentQuery,
            r#"select
            metadata.id as metadata_id, shows.id, metadata.title, metadata.poster, sum(videos.size) as size from videos
            join episodes on episodes.metadata_id = videos.metadata_id
            join seasons on seasons.id = episodes.season_id
            join shows on shows.id = seasons.show_id
            join metadata on metadata.id = shows.metadata_id
            where videos.path like ? || '%'
            group by metadata.id order by size desc"#,
            &dir.display().to_string()
        )
        .fetch_all(&db.pool)
        .await?
        .into_iter()
        .map(Into::into)
        .collect();

        let size = dir_contents_len(&dir).await?;
        show_media_dirs.push(MediaDirStats {
            path: dir,
            size,
            contents,
        })
    }

    let metadata_orphan_count = sqlx::query_scalar!(
        "
with useful_seasons as (
  select distinct seasons.metadata_id from episodes
  join metadata on metadata.id = episodes.metadata_id
  join seasons on seasons.id = episodes.season_id
  left join videos on videos.metadata_id = episodes.metadata_id
  left join list_items on list_items.metadata_id = episodes.metadata_id
  left join history on history.metadata_id = episodes.metadata_id
  left join torrent_files on torrent_files.metadata_id = episodes.metadata_id
  where
  videos.id is not null or
  list_items.id is not null or
  history.id is not null or
  torrent_files.id is not null
),
useful_shows as (
  select distinct shows.metadata_id from seasons
  join shows on shows.id = seasons.show_id
  where seasons.metadata_id in useful_seasons
)
select count(metadata.id) from metadata
left join videos on videos.metadata_id = metadata.id
left join list_items on list_items.metadata_id = metadata.id
left join history on history.metadata_id = metadata.id
left join torrent_files on torrent_files.metadata_id = metadata.id
left join shows on shows.metadata_id = metadata.id
left join seasons on seasons.metadata_id = metadata.id
where
metadata.id not in useful_seasons and
metadata.id not in useful_shows and
videos.id is null and
list_items.id is null and
history.id is null and
torrent_files.id is null;
"
    )
    .fetch_one(&db.pool)
    .instrument(tracing::debug_span!("query_metadata_orphans"))
    .await?;

    Ok(Resources {
        show_media_dirs,
        movie_media_dirs,
        tmp_size,
        db_size: db_meta.len(),
        db_path: APP_RESOURCES.database_path.clone(),
        resources_size,
        resources_path: APP_RESOURCES.resources_path.clone(),
        tmp_path: APP_RESOURCES.temp_path.clone(),
        config_path: APP_RESOURCES.config_path.clone(),
        metadata_orphan_count,
        disks: disks::disk_list(),
    })
}
