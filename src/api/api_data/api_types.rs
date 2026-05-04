use serde::Serialize;

use crate::{
    api::api_data::local_actor::LocalActorData,
    db,
    metadata::{LocaleMetadata, MetadataProvider, PersonMetadata},
};

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct History {
    pub id: i64,
    pub time: i64,
    pub is_finished: bool,
    pub update_time: crate::OffsetDateTime,
}

impl From<db::DbHistory> for History {
    fn from(
        db::DbHistory {
            id,
            time,
            is_finished,
            update_time,
            content_id: _,
        }: db::DbHistory,
    ) -> Self {
        Self {
            id: id.expect("id is not null"),
            time,
            is_finished,
            update_time: update_time.unwrap().into(),
        }
    }
}

/// Any content base data
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Content {
    pub poster: Option<String>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
}

impl From<db::DbContent> for Content {
    fn from(value: db::DbContent) -> Self {
        Self {
            poster: value.poster,
            plot: value.plot,
            release_date: value.release_date,
            title: value.title,
            locale_metadata: value.original_title.zip(value.original_language).map(
                |(original_title, original_language)| LocaleMetadata {
                    original_title,
                    original_language,
                },
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Actor {
    pub name: String,
    pub poster: Option<String>,
    pub metadata_id: String,
    pub metadata_provider: MetadataProvider,
    pub imdb_id: Option<String>,
    pub character: Option<String>,
    pub local: Option<LocalActorData>,
}

impl From<PersonMetadata> for Actor {
    fn from(
        PersonMetadata {
            metadata_id,
            metadata_provider,
            person_poster,
            name,
            imdb_id,
            role,
        }: PersonMetadata,
    ) -> Self {
        Self {
            name,
            poster: person_poster,
            metadata_id,
            metadata_provider,
            imdb_id,
            character: role.map(|r| r.character),
            local: None,
        }
    }
}

impl Actor {
    pub fn extend_meta(meta: PersonMetadata, local: Option<LocalActorData>) -> Self {
        Self {
            name: meta.name,
            poster: meta.person_poster,
            metadata_id: meta.metadata_id,
            metadata_provider: meta.metadata_provider,
            imdb_id: meta.imdb_id,
            character: meta.role.map(|r| r.character),
            local,
        }
    }
}
