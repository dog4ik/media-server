use std::collections::HashMap;

use crate::{
    db::{DbActions, DbQueryBuilder, DbRole, DbTransaction},
    library::assets::{PosterAsset, PosterContentType},
    metadata::{
        MetadataProvider, PersonMetadata,
        metadata_api::asset_saver::{AssetKind, AssetSaveTask, AssetTaskSource, AssetTasks},
    },
};

pub(crate) async fn insert_roles(
    tx: &mut DbTransaction,
    metadata_id: i64,
    cast: Vec<PersonMetadata>,
    asset_tasks: &mut AssetTasks,
) -> sqlx::Result<()> {
    if cast.is_empty() {
        return Ok(());
    }

    #[derive(sqlx::FromRow)]
    struct ActorQueryRow {
        id: i64,
        external_metadata_id: String,
        external_metadata_provider: MetadataProvider,
    }

    #[derive(Debug, Hash, Eq, PartialEq)]
    struct MapKey<'a> {
        provider: MetadataProvider,
        provider_id: &'a str,
    }

    let local_actors = DbQueryBuilder::new(
        "select id, external_metadata_id, external_metadata_provider from actors where (external_metadata_id, external_metadata_provider) in ",
    )
    .push_tuples(
        cast.iter(),
        |mut b,
         PersonMetadata {
             metadata_id,
             metadata_provider,
             ..
         }| {
            b.push_bind(metadata_id).push_bind(metadata_provider);
        },
    )
    .build_query_as::<ActorQueryRow>()
    .fetch_all(&mut **tx)
    .await?;

    let local_actors_map: HashMap<_, _> = local_actors
        .iter()
        .map(|v| {
            (
                MapKey {
                    provider: v.external_metadata_provider,
                    provider_id: &v.external_metadata_id,
                },
                v.id,
            )
        })
        .collect();

    for cast in cast {
        let actor_id = match local_actors_map.get(&MapKey {
            provider: cast.metadata_provider,
            provider_id: &cast.metadata_id,
        }) {
            Some(id) => *id,
            None => {
                let actor_id = tx.insert_actor(&cast.to_db_actor()).await?;
                if let Some(poster_url) = cast.person_poster {
                    asset_tasks.push(AssetSaveTask {
                        kind: AssetKind::Poster(PosterAsset::new(
                            actor_id,
                            PosterContentType::Actor,
                        )),
                        source: AssetTaskSource::Url(poster_url),
                    });
                }
                actor_id
            }
        };

        tx.insert_role(&DbRole {
            id: None,
            actor_id,
            metadata_id,
            character: cast.role.map(|r| r.character),
        })
        .await?;
    }
    Ok(())
}
