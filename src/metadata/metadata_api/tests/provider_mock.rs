use std::{
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    time::Duration,
};

use crate::{
    app_state::AppError,
    db::{Db, DbActions, DbExternalId, DbMetadata, DbTransaction},
    metadata::{
        EpisodeMetadata, FetchParams, MetadataProvider, MovieMetadata, MovieMetadataProvider,
        ProviderIdentifier, SeasonMetadata, ShowMetadata, ShowMetadataProvider,
    },
};

trait TestKey {
    fn unique_local_id(&self) -> i64;
}

#[derive(Debug, Clone, Copy, Hash)]
pub struct MovieKey {
    movie_id: usize,
}

impl MovieKey {
    pub fn new(movie_id: usize) -> MovieKey {
        Self { movie_id }
    }
    pub fn external_metadata(&self) -> MovieMetadata {
        MovieMetadata {
            metadata_id: format!("movie{}", self.movie_id),
            metadata_provider: MetadataProvider::Tmdb,
            plot: Some(format!("Movie {} plot.", self.movie_id)),
            runtime: Some(crate::MediaDuration(Duration::from_mins(10))),
            title: format!("Movie {}", self.movie_id),
            ..Default::default()
        }
    }
}

impl TestKey for MovieKey {
    fn unique_local_id(&self) -> i64 {
        let mut s = DefaultHasher::new();
        "movie".hash(&mut s);
        self.hash(&mut s);
        s.finish().cast_signed()
    }
}

#[derive(Debug, Clone, Copy, Hash)]
pub struct EpisodeKey {
    show_id: usize,
    season: usize,
    episode: usize,
}

impl TestKey for EpisodeKey {
    fn unique_local_id(&self) -> i64 {
        let mut s = DefaultHasher::new();
        self.hash(&mut s);
        s.finish().cast_signed()
    }
}

impl EpisodeKey {
    pub fn external_metadata(&self) -> EpisodeMetadata {
        let EpisodeKey {
            show_id,
            season,
            episode,
        } = *self;
        EpisodeMetadata {
            metadata_id: format!("show{show_id}S{season}E{episode}"),
            metadata_provider: MetadataProvider::Tmdb,
            number: episode,
            title: format!("Episode {episode}"),
            plot: Some(format!(
                "show {show_id}, season {season}, episode {episode} plot"
            )),
            season_number: season,
            runtime: Some(Duration::from_mins(10).into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Hash)]
pub struct SeasonKey {
    show_id: usize,
    season: usize,
}

impl TestKey for SeasonKey {
    fn unique_local_id(&self) -> i64 {
        let mut s = DefaultHasher::new();
        self.hash(&mut s);
        s.finish().cast_signed()
    }
}

impl SeasonKey {
    pub fn season_metadata(&self) -> SeasonMetadata {
        SeasonMetadata {
            metadata_id: format!("show{}S{}", self.show_id, self.season),
            metadata_provider: MetadataProvider::Tmdb,
            plot: Some(format!("Season {} plot", self.season)),
            number: self.season,
            ..Default::default()
        }
    }

    pub fn episode_key(&self, episode: usize) -> EpisodeKey {
        EpisodeKey {
            show_id: self.show_id,
            season: self.season,
            episode,
        }
    }
}

#[derive(Debug, Clone, Copy, Hash)]
pub struct ShowKey {
    show_id: usize,
}

impl TestKey for ShowKey {
    fn unique_local_id(&self) -> i64 {
        let mut s = DefaultHasher::new();
        // Feed it "show" so id does not conflict with movies
        "show".hash(&mut s);
        self.hash(&mut s);
        s.finish().cast_signed()
    }
}

impl ShowKey {
    pub fn show_metadata(&self) -> ShowMetadata {
        ShowMetadata {
            metadata_id: format!("show_{}", self.show_id),
            metadata_provider: MetadataProvider::Tmdb,
            plot: Some(format!("show {} plot", self.show_id)),
            title: format!("Show {}", self.show_id),
            ..Default::default()
        }
    }

    pub fn season_key(&self, season: usize) -> SeasonKey {
        SeasonKey {
            show_id: self.show_id,
            season,
        }
    }
}

/// Builds an artificial show tree from the [`ShowKey`] / [`SeasonKey`] /
/// [`EpisodeKey`] structs and persists it to the database.
///
/// Metadata rows are inserted with the deterministic [`TestKey::unique_local_id`]
/// as their primary key, so assertions can reference `saved.*.metadata_id`
/// directly. Content-table rows (shows/seasons/episodes) auto-increment and their
/// ids are returned in the [`SavedShow`] tree.
pub struct ShowTreeBuilder {
    show: ShowKey,
    seasons: Vec<(SeasonKey, Vec<EpisodeKey>)>,
}

impl ShowTreeBuilder {
    pub fn new(show_id: usize) -> Self {
        Self {
            show: ShowKey { show_id },
            seasons: Vec::new(),
        }
    }

    pub fn show_key(&self) -> ShowKey {
        self.show
    }

    pub fn add_episodes(
        &mut self,
        season: usize,
        episode_numbers: impl IntoIterator<Item = usize>,
    ) {
        let (season_key, episodes) = match self
            .seasons
            .iter_mut()
            .find(|(key, _)| key.season == season)
        {
            Some(v) => v,
            None => self
                .seasons
                .push_mut((self.show.season_key(season), Vec::new())),
        };
        for episode in episode_numbers {
            episodes.push(season_key.episode_key(episode));
        }
    }

    /// Adds a season with the given episode numbers (e.g. `1..=3` or `[1, 2, 5]`).
    pub fn season(mut self, season: usize, episodes: impl IntoIterator<Item = usize>) -> Self {
        let season_key = self.show.season_key(season);
        let episode_keys = episodes
            .into_iter()
            .map(|episode| season_key.episode_key(episode))
            .collect();
        self.seasons.push((season_key, episode_keys));
        self
    }

    /// Persists the whole tree (metadata + content rows + external ids) in a
    /// single transaction and returns the created ids.
    pub async fn save(&self, db: &Db) -> anyhow::Result<SavedShow> {
        let mut tx = db.begin().await?;

        let show_metadata_id = self.show.unique_local_id();
        let show_meta = self.show.show_metadata();
        insert_metadata_with_id(&mut tx, show_metadata_id, &show_meta.into_db_metadata()).await?;
        let show_id = tx
            .insert_show(&show_meta.into_db_show(show_metadata_id))
            .await?;
        insert_external_id(
            &mut tx,
            show_metadata_id,
            show_meta.metadata_provider,
            &show_meta.metadata_id,
        )
        .await?;

        let mut saved_seasons = Vec::new();
        for (season_key, episode_keys) in &self.seasons {
            let season_metadata_id = season_key.unique_local_id();
            let season_meta = season_key.season_metadata();
            insert_metadata_with_id(&mut tx, season_metadata_id, &season_meta.into_db_metadata())
                .await?;
            let season_id = tx
                .insert_season(season_meta.into_db_season(season_metadata_id, show_id))
                .await?;

            let mut saved_episodes = Vec::new();
            for episode_key in episode_keys {
                let episode_metadata_id = episode_key.unique_local_id();
                let episode_meta = episode_key.external_metadata();
                insert_metadata_with_id(
                    &mut tx,
                    episode_metadata_id,
                    &episode_meta.into_db_metadata(),
                )
                .await?;
                let duration = episode_meta
                    .runtime
                    .as_ref()
                    .map(|r| r.0)
                    .unwrap_or_default();
                let episode_id = tx
                    .insert_episode(&episode_meta.into_db_episode(
                        episode_metadata_id,
                        season_id,
                        duration,
                    ))
                    .await?;
                insert_external_id(
                    &mut tx,
                    episode_metadata_id,
                    episode_meta.metadata_provider,
                    &episode_meta.metadata_id,
                )
                .await?;
                saved_episodes.push(SavedEpisode {
                    key: *episode_key,
                    content_id: episode_id,
                    metadata_id: episode_metadata_id,
                });
            }

            saved_seasons.push(SavedSeason {
                key: *season_key,
                content_id: season_id,
                metadata_id: season_metadata_id,
                episodes: saved_episodes,
            });
        }

        tx.commit().await?;

        Ok(SavedShow {
            key: self.show,
            content_id: show_id,
            metadata_id: show_metadata_id,
            seasons: saved_seasons,
        })
    }
}

/// A persisted show tree. `content_id` is the row id in the `shows`/`seasons`/
/// `episodes` table; `metadata_id` is the (deterministic) `metadata` row id.
#[derive(Debug)]
pub struct SavedShow {
    pub key: ShowKey,
    pub content_id: i64,
    pub metadata_id: i64,
    pub seasons: Vec<SavedSeason>,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct SavedSeason {
    pub key: SeasonKey,
    pub content_id: i64,
    pub metadata_id: i64,
    pub episodes: Vec<SavedEpisode>,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct SavedEpisode {
    pub key: EpisodeKey,
    pub content_id: i64,
    pub metadata_id: i64,
}

impl SavedShow {
    /// Finds a saved episode by season/episode number, for assertions.
    pub fn episode(&self, season: usize, episode: usize) -> Option<&SavedEpisode> {
        self.seasons
            .iter()
            .find(|s| s.key.season == season)?
            .episodes
            .iter()
            .find(|e| e.key.episode == episode)
    }
}

/// Inserts a `metadata` row with an explicit primary key (the deterministic
/// `unique_local_id`), unlike [`DbActions::insert_metadata`] which auto-increments.
async fn insert_metadata_with_id(
    tx: &mut DbTransaction,
    id: i64,
    metadata: &DbMetadata,
) -> anyhow::Result<()> {
    sqlx::query!(
        "INSERT INTO metadata
            (id, content_type, title, release_date, poster, plot, original_language, original_title)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?);",
        id,
        metadata.content_type,
        metadata.title,
        metadata.release_date,
        metadata.poster,
        metadata.plot,
        metadata.original_language,
        metadata.original_title,
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_external_id(
    tx: &mut DbTransaction,
    metadata_id: i64,
    provider: MetadataProvider,
    external_id: &str,
) -> anyhow::Result<()> {
    tx.insert_external_id(DbExternalId {
        external_provider: provider,
        external_id: external_id.to_string(),
        metadata_id: Some(metadata_id),
        ..Default::default()
    })
    .await?;
    Ok(())
}

/// A metadata provider that serves a fixed set of content supplied at construction time.
#[derive(Debug, Clone)]
pub struct MockProvider {
    /// Provider show id
    shows: HashMap<String, MockShowEntry>,
    movies: HashMap<String, MovieMetadata>,
}

#[derive(Debug, Clone)]
struct MockShowEntry {
    metadata: ShowMetadata,
    seasons: HashMap<usize, SeasonMetadata>,
    episodes: HashMap<(usize, usize), EpisodeMetadata>,
}

impl MockProvider {
    pub fn new(
        shows: impl IntoIterator<Item = ShowTreeBuilder>,
        movies: impl IntoIterator<Item = MovieKey>,
    ) -> Self {
        let mut map = HashMap::new();
        for builder in shows {
            let mut metadata = builder.show.show_metadata();
            let mut season_numbers: Vec<usize> = builder
                .seasons
                .iter()
                .map(|(season, _)| season.season)
                .collect();
            season_numbers.sort_unstable();
            metadata.episodes_amount = Some(builder.seasons.iter().map(|(_, eps)| eps.len()).sum());
            metadata.seasons = Some(season_numbers);

            let seasons = builder
                .seasons
                .iter()
                .map(|(season, _)| (season.season, season.season_metadata()))
                .collect();
            let episodes = builder
                .seasons
                .iter()
                .flat_map(|(season, episodes)| {
                    let season_number = season.season;
                    episodes.iter().map(move |episode| {
                        (
                            (season_number, episode.episode),
                            episode.external_metadata(),
                        )
                    })
                })
                .collect();

            map.insert(
                metadata.metadata_id.clone(),
                MockShowEntry {
                    metadata,
                    seasons,
                    episodes,
                },
            );
        }
        Self {
            shows: map,
            movies: movies
                .into_iter()
                .map(|v| {
                    let metadata = v.external_metadata();
                    (metadata.metadata_id.clone(), metadata)
                })
                .collect(),
        }
    }

    fn get_show(&self, show_id: &str) -> Result<&MockShowEntry, AppError> {
        self.shows
            .get(show_id)
            .ok_or_else(|| anyhow::anyhow!("mock provider has no show {show_id}").into())
    }

    fn get_movie(&self, movie_id: &str) -> Result<&MovieMetadata, AppError> {
        self.movies
            .get(movie_id)
            .ok_or_else(|| anyhow::anyhow!("mock provider has no movie {movie_id}").into())
    }
}

impl ProviderIdentifier for MockProvider {
    fn provider_identifier(&self) -> MetadataProvider {
        MetadataProvider::Tmdb
    }
}

#[async_trait::async_trait]
impl ShowMetadataProvider for MockProvider {
    async fn show(&self, show_id: &str, _: FetchParams) -> Result<ShowMetadata, AppError> {
        Ok(self.get_show(show_id)?.metadata.clone())
    }

    async fn season(
        &self,
        show_id: &str,
        season: usize,
        _: FetchParams,
    ) -> Result<SeasonMetadata, AppError> {
        self.get_show(show_id)?
            .seasons
            .get(&season)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("mock provider has no show {show_id} season {season}").into()
            })
    }

    async fn episode(
        &self,
        show_id: &str,
        season: usize,
        episode: usize,
        _: FetchParams,
    ) -> Result<EpisodeMetadata, AppError> {
        self.get_show(show_id)?
            .episodes
            .get(&(season, episode))
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!("mock provider has no show {show_id} s{season}e{episode}").into()
            })
    }

    async fn show_search(
        &self,
        query: &str,
        _: FetchParams,
    ) -> Result<Vec<ShowMetadata>, AppError> {
        let query = query.to_lowercase();
        Ok(self
            .shows
            .values()
            .filter(|show| show.metadata.title.to_lowercase().contains(&query))
            .map(|show| show.metadata.clone())
            .collect())
    }
}

#[async_trait::async_trait]
impl MovieMetadataProvider for MockProvider {
    async fn movie(
        &self,
        movie_metadata_id: &str,
        _: FetchParams,
    ) -> Result<MovieMetadata, AppError> {
        self.get_movie(movie_metadata_id).cloned()
    }

    async fn movie_search(
        &self,
        query: &str,
        _: FetchParams,
    ) -> Result<Vec<MovieMetadata>, AppError> {
        Ok(self
            .movies
            .values()
            .filter(|&movie| movie.title.to_lowercase().contains(&query))
            .cloned()
            .collect())
    }
}
