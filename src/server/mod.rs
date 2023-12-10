use serde::Deserialize;

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
pub struct StringIdQuery {
    pub id: String,
}

#[derive(Deserialize)]
pub struct SeasonQuery {
    pub season: i32,
}

#[derive(Deserialize)]
pub struct EpisodeQuery {
    pub episode: i32,
}

#[derive(Deserialize)]
pub struct NumberQuery {
    pub number: i32,
}

#[derive(Deserialize)]
pub struct LanguageQuery {
    pub lang: Option<String>,
}
