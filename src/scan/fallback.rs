use crate::metadata::{
    EpisodeMetadata, MovieMetadata, SeasonMetadata, ShowMetadata, metadata_api::fallback,
};

use super::{MetadataLookup, MetadataLookupWithIds};

pub(super) fn show_fallback(title: &str) -> MetadataLookupWithIds<ShowMetadata> {
    MetadataLookupWithIds::New {
        metadata: fallback::show_fallback_meta(title),
        external_ids: vec![],
    }
}

pub(super) fn season_fallback(season_number: usize) -> MetadataLookup<SeasonMetadata> {
    MetadataLookup::New {
        metadata: fallback::season_fallback_meta(season_number),
    }
}

pub(super) fn episode_fallback(
    episode_number: usize,
    season_number: usize,
) -> MetadataLookup<EpisodeMetadata> {
    MetadataLookup::New {
        metadata: fallback::episode_fallback_meta(episode_number, season_number),
    }
}

pub(super) fn movie_fallback(title: &str) -> MetadataLookupWithIds<MovieMetadata> {
    MetadataLookupWithIds::New {
        metadata: fallback::movie_fallback_meta(title),
        external_ids: vec![],
    }
}
