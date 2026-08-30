use std::collections::HashMap;

use serde::Serialize;
use torrent::{Info, OutputFile};

use crate::{
    api::{
        api_data::{
            local_movie::Movie,
            local_show::{Episode, Show},
        },
        torrent::DownloadContentHint,
    },
    db::Db,
    library::{Media, is_format_supported},
    metadata::{
        ParentMediaType,
        metadata_api::{
            local_scope,
            movie::MovieMetadataApi,
            show::{ShowItem, ShowMetadataApi, ShowTree},
        },
        metadata_stack::MetadataProvidersStack,
    },
    parser::{movie::MovieIdentifier, show::ShowIdentifier},
    torrent::Priority,
};

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TorrentInfo {
    pub name: String,
    pub content: Option<TorrentContent>,
    pub other_files: Vec<ResolvedTorrentFile>,
    pub piece_length: u32,
    pub pieces_amount: usize,
    pub total_size: u64,
}

impl TorrentInfo {
    pub async fn new(
        info: &Info,
        db: &'static Db,
        http_client: reqwest::Client,
        content_type_hint: Option<DownloadContentHint>,
        providers_stack: &'static MetadataProvidersStack,
    ) -> Self {
        let all_files = info.output_files("");
        let (other_files, content) = resolve_torrent_files(
            providers_stack,
            db,
            http_client,
            &all_files,
            content_type_hint,
        )
        .await;

        TorrentInfo {
            content,
            other_files,
            name: info.name.clone(),
            piece_length: info.piece_length,
            pieces_amount: info.pieces.len(),
            total_size: info.total_size(),
        }
    }
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "media_type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum TorrentContent {
    Show {
        show: Show,
        seasons: HashMap<u16, Vec<TorrentEpisode>>,
    },
    Movie {
        movies: Vec<TorrentMovie>,
    },
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ResolvedTorrentFile {
    pub file_idx: usize,
    pub offset: u64,
    pub size: u64,
    pub path: Vec<String>,
    pub priority: Priority,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TorrentMovie {
    #[serde(flatten)]
    pub file: ResolvedTorrentFile,
    pub metadata: Movie,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct TorrentEpisode {
    #[serde(flatten)]
    pub file: ResolvedTorrentFile,
    pub metadata: Episode,
}

pub fn path_components(path: impl AsRef<std::path::Path>) -> Vec<String> {
    let mut out = Vec::new();
    for component in path.as_ref().components() {
        if let std::path::Component::Normal(component) = component {
            out.push(component.to_string_lossy().to_string())
        }
    }
    out
}

#[derive(Debug)]
struct TorrentContentItem<T> {
    ident: T,
    file: ResolvedTorrentFile,
}

impl ShowItem for TorrentContentItem<ShowIdentifier> {
    fn season(&self) -> usize {
        self.ident.season as usize
    }

    fn episode(&self) -> usize {
        self.ident.episode as usize
    }

    fn fallback_source(&self) -> Option<crate::library::Source> {
        None
    }
}

fn detect_media_type(files: &[OutputFile]) -> ParentMediaType {
    if files
        .iter()
        .filter(|f| is_format_supported(f.path()))
        .any(|f| ShowIdentifier::from_path(f.path()).is_ok())
    {
        ParentMediaType::Show
    } else {
        ParentMediaType::Movie
    }
}

fn group_files<T: Media>(
    files: &[OutputFile],
) -> (Vec<ResolvedTorrentFile>, Vec<TorrentContentItem<T>>) {
    let mut other_files: Vec<ResolvedTorrentFile> = Vec::new();
    let mut identifiers: Vec<_> = Vec::new();
    let mut offset = 0;
    for (file_idx, file) in files.iter().enumerate() {
        let path = file.path();
        let size = file.length();
        let file = ResolvedTorrentFile {
            file_idx,
            offset,
            size,
            path: path_components(path),
            priority: Priority::Disabled,
        };
        match T::identify(path) {
            Ok(ident) => identifiers.push(TorrentContentItem { ident, file }),
            _ => other_files.push(file),
        }
        offset += size;
    }
    (other_files, identifiers)
}

async fn resolve_torrent_files(
    providers_stack: &'static MetadataProvidersStack,
    db: &'static Db,
    http_client: reqwest::Client,
    files: &[OutputFile],
    content_hint: Option<DownloadContentHint>,
) -> (Vec<ResolvedTorrentFile>, Option<TorrentContent>) {
    let content_type = content_hint
        .map(|v| v.content_type)
        .unwrap_or_else(|| detect_media_type(files));
    match content_type {
        ParentMediaType::Movie => {
            let (mut other_files, content) = group_files::<MovieIdentifier>(files);
            let api = MovieMetadataApi::new(providers_stack.tmdb.unwrap(), db, http_client);
            let mut resolved_movies = Vec::new();
            for content in content {
                if let Ok(Some(movie)) = api
                    .search_movie_title(content.ident.title(), content.ident.year)
                    .await
                    && let Ok(movie) = Movie::from_lookup(movie, db.clone()).await
                {
                    resolved_movies.push(TorrentMovie {
                        file: content.file,
                        metadata: movie,
                    })
                } else {
                    other_files.push(content.file);
                };
            }
            (
                other_files,
                (!resolved_movies.is_empty()).then_some(TorrentContent::Movie {
                    movies: resolved_movies,
                }),
            )
        }
        ParentMediaType::Show => {
            let (mut other_files, content) = group_files::<ShowIdentifier>(files);
            let Some(title) = content.first().map(|v| v.ident.title()) else {
                return (other_files, None);
            };
            let api = ShowMetadataApi::new(providers_stack.tmdb.unwrap(), db, http_client);
            if let Ok(Some(show)) = api
                .search_show_title::<local_scope::ApiObjectScope>(title)
                .await
                && let Ok(show_tree) = api
                    .fetch_show_tree::<_, local_scope::ApiObjectScope>(
                        show,
                        ShowTree::from_flat(content),
                    )
                    .await
                && let Some(show) = Show::from_lookup(show_tree.show_lookup)
            {
                let seasons = show_tree
                    .seasons
                    .into_iter()
                    .map(|s| {
                        (
                            s.number as u16,
                            s.episodes
                                .into_iter()
                                .filter_map(|resolved_episode| {
                                    if let Some(metadata) =
                                        Episode::from_lookup(resolved_episode.lookup)
                                    {
                                        let priority = if metadata.local.is_some() {
                                            Priority::Disabled
                                        } else {
                                            Priority::Medium
                                        };
                                        Some(resolved_episode.items.into_iter().map(
                                            move |mut item| {
                                                item.file.priority = priority;
                                                TorrentEpisode {
                                                    file: item.file,
                                                    metadata: metadata.clone(),
                                                }
                                            },
                                        ))
                                    } else {
                                        other_files.extend(
                                            resolved_episode.items.into_iter().map(|v| v.file),
                                        );
                                        None
                                    }
                                })
                                .flatten()
                                .collect(),
                        )
                    })
                    .collect();
                (other_files, Some(TorrentContent::Show { show, seasons }))
            } else {
                (other_files, None)
            }
        }
    }
}
