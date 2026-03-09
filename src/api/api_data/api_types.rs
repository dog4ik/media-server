use std::time::Duration;

use serde::Serialize;

use crate::{
    db,
    metadata::{LocaleMetadata, MetadataImage},
};

#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct History {
    pub id: i64,
    pub time: i64,
    pub is_finished: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub update_time: time::OffsetDateTime,
}

impl From<db::DbHistory> for History {
    fn from(
        db::DbHistory {
            id,
            time,
            is_finished,
            update_time,
            content_id,
        }: db::DbHistory,
    ) -> Self {
        Self {
            id: id.expect("id is not null"),
            time,
            is_finished,
            update_time,
        }
    }
}

/// Any content base data
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Content {
    pub poster: Option<MetadataImage>,
    pub plot: Option<String>,
    pub release_date: Option<String>,
    pub title: String,
    pub locale_metadata: Option<LocaleMetadata>,
}

impl From<db::DbContent> for Content {
    fn from(value: db::DbContent) -> Self {
        Self {
            poster: value
                .poster
                .map(|u| MetadataImage::new(u.parse().expect("poster is a valid url"))),
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

/// Show specific data
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Show {
    pub backdrop: Option<MetadataImage>,
    /// Array of available season numbers
    pub seasons: Option<Vec<usize>>,
    pub episodes_amount: Option<usize>,
}

/// Season specific data
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Season {
    pub number: usize,
}

/// Episode specific data
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Episode {
    pub number: usize,
    #[schema(value_type = Option<crate::api::SerdeDuration>)]
    pub runtime: Option<Duration>,
}

/// Movie specific data parts
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Movie {
    pub backdrop: Option<MetadataImage>,
    #[schema(value_type = Option<crate::api::SerdeDuration>)]
    pub runtime: Option<Duration>,
}
