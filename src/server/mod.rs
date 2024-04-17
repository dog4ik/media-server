use serde::Deserialize;

use crate::metadata::{ContentType, MetadataProvider};

pub mod admin_api;
pub mod content;
pub mod public_api;

#[derive(Deserialize)]
pub struct PageQuery {
    pub page: Option<usize>,
}

#[derive(Deserialize)]
pub struct IdQuery {
    pub id: i64,
}

#[derive(Deserialize)]
pub struct SearchQuery {
    pub search: String,
}

#[derive(Deserialize)]
pub struct ContentTypeQuery {
    pub content_type: ContentType,
}

#[derive(Deserialize)]
pub struct ProviderQuery {
    pub provider: MetadataProvider,
}

#[derive(Deserialize)]
pub struct VariantQuery {
    pub variant: String,
}

#[derive(Deserialize)]
pub struct StringIdQuery {
    pub id: String,
}

#[derive(Deserialize)]
pub struct SeasonQuery {
    pub season: usize,
}

#[derive(Deserialize)]
pub struct EpisodeQuery {
    pub episode: usize,
}

#[derive(Deserialize)]
pub struct NumberQuery {
    pub number: usize,
}

#[derive(Deserialize)]
pub struct LanguageQuery {
    pub lang: Option<String>,
}

#[derive(Deserialize)]
pub struct TakeParam {
    pub take: Option<usize>,
}
