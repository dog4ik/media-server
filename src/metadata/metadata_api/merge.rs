use std::collections::hash_map;

use crate::metadata::{
    ExternalIdMetadata, MovieMetadata, ShowMetadata,
    metadata_api::{MetadataLookup, local_scope::LocalMetadataIdentifier},
};

/// Metadata that carries the provider ids two chunks can be matched on.
pub trait HasExternalIds {
    fn external_ids(&self) -> &[ExternalIdMetadata];
}

impl HasExternalIds for ShowMetadata {
    fn external_ids(&self) -> &[ExternalIdMetadata] {
        self.external_ids.as_deref().unwrap_or_default()
    }
}

impl HasExternalIds for MovieMetadata {
    fn external_ids(&self) -> &[ExternalIdMetadata] {
        self.external_ids.as_deref().unwrap_or_default()
    }
}

/// What a resolved chunk can be matched against its neighbours on.
pub enum MergeKey<'a> {
    External(&'a [ExternalIdMetadata]),
    Local(i64),
    /// Nothing to match on, the chunk always stands alone
    Unmergeable,
}

/// A resolved lookup that [try_merge_chunks] can group by.
pub trait Mergeable {
    fn merge_key(&self) -> MergeKey<'_>;
}

impl<M, L> Mergeable for MetadataLookup<M, L>
where
    M: HasExternalIds,
    L: LocalMetadataIdentifier,
{
    fn merge_key(&self) -> MergeKey<'_> {
        match self {
            MetadataLookup::New { metadata } => MergeKey::External(metadata.external_ids()),
            MetadataLookup::Local(local) => MergeKey::Local(local.local_id().id),
            MetadataLookup::Missing => MergeKey::Unmergeable,
        }
    }
}

pub fn try_merge_chunks<S, T>(statuses: &[S], items: &mut [Vec<T>])
where
    S: Mergeable,
{
    let mut id_to_chunk_idx: hash_map::HashMap<ExternalIdMetadata, usize> =
        hash_map::HashMap::new();
    let mut local_id_to_chunk_idx: hash_map::HashMap<i64, usize> = hash_map::HashMap::new();
    for (i, current_status) in statuses.iter().enumerate() {
        match current_status.merge_key() {
            MergeKey::External(external_ids) => {
                let mut moved_to_chunk: Option<usize> = None;
                for id in external_ids {
                    match id_to_chunk_idx.entry(id.clone()) {
                        hash_map::Entry::Occupied(occupied_entry) => {
                            let chunk_idx = *occupied_entry.get();
                            moved_to_chunk = Some(chunk_idx);
                            let (before, after) = items.split_at_mut(i);
                            before[chunk_idx].append(&mut after[0]);
                            break;
                        }
                        hash_map::Entry::Vacant(vacant_entry) => {
                            vacant_entry.insert(i);
                        }
                    };
                }
                if let Some(moved_to_chunk) = moved_to_chunk {
                    for id in external_ids {
                        *id_to_chunk_idx.entry(id.clone()).or_default() = moved_to_chunk;
                    }
                }
            }
            MergeKey::Local(local_id) => match local_id_to_chunk_idx.entry(local_id) {
                hash_map::Entry::Occupied(occupied_entry) => {
                    let chunk_idx = *occupied_entry.get();
                    let (before, after) = items.split_at_mut(i);
                    before[chunk_idx].append(&mut after[0]);
                }
                hash_map::Entry::Vacant(vacant_entry) => {
                    vacant_entry.insert(i);
                }
            },
            MergeKey::Unmergeable => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        db::LocalContentId,
        metadata::{ExternalIdMetadata, MetadataProvider, ShowMetadata},
    };

    use super::{MetadataLookup, try_merge_chunks};

    /// A freshly resolved show carrying `ids` as its external ids.
    fn new(ids: &[(MetadataProvider, &str)]) -> MetadataLookup<ShowMetadata> {
        MetadataLookup::New {
            metadata: ShowMetadata {
                external_ids: Some(
                    ids.iter()
                        .map(|(provider, id)| ExternalIdMetadata {
                            provider: *provider,
                            id: id.to_string(),
                        })
                        .collect(),
                ),
                ..Default::default()
            },
        }
    }

    /// A show already present in the database under content id `id`.
    fn local(id: i64) -> MetadataLookup<ShowMetadata> {
        MetadataLookup::Local(LocalContentId {
            id,
            metadata_id: id,
        })
    }

    #[test]
    fn merge_chunks_do_nothing() {
        let statuses = vec![
            new(&[
                (MetadataProvider::Tmdb, "0"),
                (MetadataProvider::Tvdb, "0"),
                (MetadataProvider::Imdb, "0"),
            ]),
            new(&[
                (MetadataProvider::Tmdb, "1"),
                (MetadataProvider::Tvdb, "1"),
                (MetadataProvider::Imdb, "1"),
            ]),
            new(&[
                (MetadataProvider::Tmdb, "2"),
                (MetadataProvider::Tvdb, "2"),
                (MetadataProvider::Imdb, "2"),
            ]),
        ];

        // items are unique library items chunked by title
        let mut items = vec![vec![0, 1], vec![2, 3], vec![4, 5]];
        assert_eq!(
            statuses.len(),
            items.len(),
            "Test is broken, statuses length should be equal to items"
        );
        let copy = items.clone();
        try_merge_chunks(&statuses, &mut items);
        assert_eq!(items, copy);
    }

    #[test]
    fn merge_chunks_simple() {
        let first_ids = [
            (MetadataProvider::Tmdb, "0"),
            (MetadataProvider::Tvdb, "0"),
            (MetadataProvider::Imdb, "0"),
        ];
        let statuses = vec![
            new(&[
                (MetadataProvider::Tmdb, "1"),
                (MetadataProvider::Tvdb, "1"),
                (MetadataProvider::Imdb, "1"),
            ]),
            new(&first_ids),
            new(&first_ids),
        ];

        // items are unique library items chunked by title
        let mut items = vec![vec![0, 1], vec![2, 3], vec![4, 5]];
        assert_eq!(
            statuses.len(),
            items.len(),
            "Test is broken, statuses length should be equal to items"
        );
        try_merge_chunks(&statuses, &mut items);
        assert_eq!(items[0].len(), 2);
        assert_eq!(items[1].len(), 4);
        assert!(items[2].is_empty());
    }

    /// Test situation where second_ids(2) point to first_ids(1)
    /// and the third_ids(3) points to second_ids(2)
    ///
    /// In this situation because 2 are moved into 1, 3 should move in 1.
    #[test]
    fn merge_chunks_transitional() {
        let statuses = vec![
            new(&[(MetadataProvider::Tmdb, "0"), (MetadataProvider::Tvdb, "0")]),
            new(&[
                (MetadataProvider::Tmdb, "0"),
                (MetadataProvider::Tvdb, "1"),
                (MetadataProvider::Imdb, "2"),
            ]),
            new(&[(MetadataProvider::Imdb, "2")]),
        ];

        // items are unique library items chunked by title
        let mut items = vec![vec![0, 1], vec![2, 3], vec![4, 5]];
        assert_eq!(
            statuses.len(),
            items.len(),
            "Test is broken, statuses length should be equal to items"
        );
        try_merge_chunks(&statuses, &mut items);
        assert_eq!(items[0].len(), 6);
        assert!(items[1].is_empty());
        assert!(items[2].is_empty());
    }

    #[test]
    fn merge_chunks_local() {
        let statuses = vec![
            local(0),
            new(&[
                (MetadataProvider::Tmdb, "0"),
                (MetadataProvider::Tvdb, "1"),
                (MetadataProvider::Imdb, "2"),
            ]),
            local(1),
            local(0),
        ];

        // items are unique library items chunked by title
        let mut items = vec![vec![0, 1], vec![2, 3], vec![4, 5], vec![6, 7]];
        assert_eq!(
            statuses.len(),
            items.len(),
            "Test is broken, statuses length should be equal to items"
        );
        try_merge_chunks(&statuses, &mut items);

        assert_eq!(items[0].len(), 4);
        assert_eq!(items[1].len(), 2);
        assert_eq!(items[2].len(), 2);
        assert!(items[3].is_empty());
    }

    #[test]
    fn merge_chunks_mixed() {
        let statuses = vec![
            local(0),
            new(&[
                (MetadataProvider::Tmdb, "0"),
                (MetadataProvider::Tvdb, "1"),
                (MetadataProvider::Imdb, "2"),
            ]),
            local(1),
            local(0),
            new(&[(MetadataProvider::Imdb, "2")]),
        ];

        // items are unique library items chunked by title
        let mut items = vec![vec![0, 1], vec![2, 3], vec![4, 5], vec![6, 7], vec![8, 9]];
        assert_eq!(
            statuses.len(),
            items.len(),
            "Test is broken, statuses length should be equal to items"
        );
        try_merge_chunks(&statuses, &mut items);

        assert_eq!(items[0].len(), 4);
        assert_eq!(items[1].len(), 4);
        assert_eq!(items[2].len(), 2);
        assert!(items[3].is_empty());
        assert!(items[4].is_empty());
    }
}
