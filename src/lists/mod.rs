use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{
    db::{Db, DbContentType, query_builders::ExternalIdsQueryJson},
    metadata::MetadataProvider,
};

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExportedExternalId {
    pub id: String,
    pub provider: MetadataProvider,
    pub is_prime: bool,
}

impl From<ExternalIdsQueryJson> for ExportedExternalId {
    fn from(value: ExternalIdsQueryJson) -> Self {
        Self {
            id: value.external_id,
            provider: value.external_provider,
            is_prime: value.is_prime == 1,
        }
    }
}

fn de_i64_key_map<'de, D>(d: D) -> Result<HashMap<i64, Vec<i64>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    HashMap::<String, Vec<i64>>::deserialize(d)?
        .into_iter()
        .map(|(k, v)| Ok((k.parse().map_err(serde::de::Error::custom)?, v)))
        .collect()
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ExportedGroupedContentType {
    Movie,
    Show {
        /// Is the show itself in list
        self_in_list: bool,
        /// Map seasons to episode list
        #[serde(deserialize_with = "de_i64_key_map")]
        episodes: HashMap<i64, Vec<i64>>,
    },
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ExportedGroupedItem {
    #[serde(flatten)]
    pub content_type: ExportedGroupedContentType,
    pub title: String,
    pub external_ids: Vec<ExportedExternalId>,
}

/// Extract all list items
///
/// Groups the show content into a single element
pub async fn export_grouped_list(db: &Db, list_id: i64) -> sqlx::Result<Vec<ExportedGroupedItem>> {
    let list_contents = sqlx::query!(
        r#"
with resolved as (
  select coalesce(
    shows.metadata_id,
    case when episodes.metadata_id is null
      then list_items.metadata_id end
    ) as entity_metadata_id,
    seasons.number  as season_number,
    episodes.number as episode_number,
    case when episodes.metadata_id is null then 1 else 0 end as self_in_list
    from list_items
    left join episodes on episodes.metadata_id = list_items.metadata_id
    left join seasons on seasons.id = episodes.season_id
    left join shows on shows.id = seasons.show_id
    where list_items.list_id = ?
  ),
  season_episodes as (
    select entity_metadata_id, season_number,
    json_group_array(episode_number order by episode_number) as episode_numbers
    from resolved
    where episode_number is not null
    group by entity_metadata_id, season_number
  ),
  content as (
    select entity_metadata_id,
    json_group_object(cast(season_number as text), json(episode_numbers)) as content
    from season_episodes
    group by entity_metadata_id
  ),
  ext as (
    select metadata_id,
    json_group_array(json_object(
        'id', id, 'external_provider', external_provider,
        'external_id', external_id, 'is_prime', is_prime
    )) as external_ids
  from external_ids
  group by metadata_id
)
select metadata.id, metadata.title,
metadata.content_type as "media_type: DbContentType",
resolved.self_in_list as "self_in_list!: i64",
coalesce(content.content, json('{}')) as "episodes!: sqlx::types::Json<HashMap<i64, Vec<i64>>>",
coalesce(ext.external_ids, json('[]')) as "external_ids!: sqlx::types::Json<Vec<ExternalIdsQueryJson>>"
from resolved
join metadata on metadata.id = resolved.entity_metadata_id
left join content on content.entity_metadata_id = resolved.entity_metadata_id
join ext on ext.metadata_id = resolved.entity_metadata_id
group by metadata.id
order by metadata.id;
        "#,
        list_id
    )
    .fetch_all(&db.pool)
    .await?;
    Ok(list_contents
        .into_iter()
        .map(|r| ExportedGroupedItem {
            content_type: match r.media_type {
                DbContentType::Movie => ExportedGroupedContentType::Movie,
                _ => ExportedGroupedContentType::Show {
                    self_in_list: r.self_in_list == 1,
                    episodes: r.episodes.0,
                },
            },
            title: r.title,
            external_ids: r.external_ids.0.into_iter().map(Into::into).collect(),
        })
        .collect())
}
