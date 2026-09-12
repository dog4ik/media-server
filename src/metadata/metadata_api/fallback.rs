//! Locally generated metadata for content no provider could resolve.

use crate::metadata::{
    EpisodeMetadata, MetadataProvider, MovieMetadata, SeasonMetadata, ShowMetadata,
};

pub fn show_fallback_meta(title: &str) -> ShowMetadata {
    ShowMetadata {
        metadata_provider: MetadataProvider::Local,
        title: title.to_string(),
        ..Default::default()
    }
}

pub fn season_fallback_meta(season_number: usize) -> SeasonMetadata {
    SeasonMetadata {
        number: season_number,
        title: Some(format!("Season {season_number}")),
        ..Default::default()
    }
}

pub fn episode_fallback_meta(episode_number: usize, season_number: usize) -> EpisodeMetadata {
    EpisodeMetadata {
        number: episode_number,
        season_number,
        title: format!("Episode {episode_number}"),
        ..Default::default()
    }
}

pub fn movie_fallback_meta(title: &str) -> MovieMetadata {
    let mut chars = title.chars();
    let capitalized: String = chars
        .next()
        .and_then(|c| c.to_uppercase().next())
        .into_iter()
        .chain(chars)
        .collect();
    MovieMetadata {
        metadata_provider: MetadataProvider::Local,
        title: capitalized,
        ..Default::default()
    }
}
