use std::collections::hash_map;

use crate::metadata::ExternalIdMetadata;

use super::MetadataLookupWithIds;

pub fn try_merge_chunks<T, M>(statuses: &[MetadataLookupWithIds<M>], items: &mut Vec<Vec<T>>) {
    let mut id_to_chunk_idx: hash_map::HashMap<ExternalIdMetadata, usize> =
        hash_map::HashMap::new();
    let mut local_id_to_chunk_idx: hash_map::HashMap<i64, usize> = hash_map::HashMap::new();
    for (i, current_status) in statuses.iter().enumerate() {
        match current_status {
            MetadataLookupWithIds::New { external_ids, .. } => {
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
            MetadataLookupWithIds::Local(local_id) => {
                match local_id_to_chunk_idx.entry(*local_id) {
                    hash_map::Entry::Occupied(occupied_entry) => {
                        let chunk_idx = *occupied_entry.get();
                        let (before, after) = items.split_at_mut(i);
                        before[chunk_idx].append(&mut after[0]);
                    }
                    hash_map::Entry::Vacant(vacant_entry) => {
                        vacant_entry.insert(i);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::metadata::{ExternalIdMetadata, MetadataProvider, ShowMetadata};

    use super::{MetadataLookupWithIds, try_merge_chunks};

    #[test]
    fn merge_chunks_do_nothing() {
        let first_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "0".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "0".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id: "0".to_string(),
                },
            ],
        };
        let second_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "1".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "1".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id: "1".to_string(),
                },
            ],
        };
        let third_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "2".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "2".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id: "2".to_string(),
                },
            ],
        };
        let statuses = vec![first_ids, second_ids, third_ids];

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
        let first_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "0".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "0".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id: "0".to_string(),
                },
            ],
        };
        let second_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "1".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "1".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id: "1".to_string(),
                },
            ],
        };
        let statuses = vec![second_ids, first_ids.clone(), first_ids];

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
        let first_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "0".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "0".to_string(),
                },
            ],
        };

        let second_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "0".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "1".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id: "2".to_string(),
                },
            ],
        };

        let third_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![ExternalIdMetadata {
                provider: MetadataProvider::Imdb,
                id: "2".to_string(),
            }],
        };
        let statuses = vec![first_ids, second_ids, third_ids];

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
        let first_ids = MetadataLookupWithIds::Local(0);

        let second_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "0".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "1".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id: "2".to_string(),
                },
            ],
        };

        let third_ids = MetadataLookupWithIds::Local(1);
        let fourth_ids = MetadataLookupWithIds::Local(0);
        let statuses = vec![first_ids, second_ids, third_ids, fourth_ids];

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
        let first_ids = MetadataLookupWithIds::Local(0);

        let second_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![
                ExternalIdMetadata {
                    provider: MetadataProvider::Tmdb,
                    id: "0".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Tvdb,
                    id: "1".to_string(),
                },
                ExternalIdMetadata {
                    provider: MetadataProvider::Imdb,
                    id: "2".to_string(),
                },
            ],
        };

        let third_ids = MetadataLookupWithIds::Local(1);
        let fourth_ids = MetadataLookupWithIds::Local(0);
        let fifth_ids = MetadataLookupWithIds::New {
            metadata: ShowMetadata::default(),
            external_ids: vec![ExternalIdMetadata {
                provider: MetadataProvider::Imdb,
                id: "2".to_string(),
            }],
        };
        let statuses = vec![first_ids, second_ids, third_ids, fourth_ids, fifth_ids];

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
