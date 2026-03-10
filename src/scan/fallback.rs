use crate::metadata::{
    EpisodeMetadata, MetadataProvider, MovieMetadata, SeasonMetadata, ShowMetadata,
};

use super::{MetadataLookup, MetadataLookupWithIds};

pub(super) fn show_fallback(title: &str) -> MetadataLookupWithIds<ShowMetadata> {
    MetadataLookupWithIds::New {
        metadata: ShowMetadata {
            metadata_provider: MetadataProvider::Local,
            title: title.to_string(),
            ..Default::default()
        },
        external_ids: vec![],
    }
}

pub(super) fn season_fallback(season_number: usize) -> MetadataLookup<SeasonMetadata> {
    MetadataLookup::New {
        metadata: SeasonMetadata {
            number: season_number,
            title: Some(format!("Season {season_number}")),
            ..Default::default()
        },
    }
}

pub(super) fn episode_fallback(
    episode_number: usize,
    season_number: usize,
) -> MetadataLookup<EpisodeMetadata> {
    MetadataLookup::New {
        metadata: EpisodeMetadata {
            number: episode_number,
            season_number,
            title: format!("Episode {episode_number}"),
            ..Default::default()
        },
    }
}

pub(super) fn movie_fallback(title: &str) -> MetadataLookupWithIds<MovieMetadata> {
    let mut chars = title.chars();
    let capitalized: String = chars
        .next()
        .and_then(|c| c.to_uppercase().next())
        .into_iter()
        .chain(chars)
        .collect();
    MetadataLookupWithIds::New {
        metadata: MovieMetadata {
            metadata_provider: MetadataProvider::Local,
            title: capitalized,
            ..Default::default()
        },
        external_ids: vec![],
    }
}
