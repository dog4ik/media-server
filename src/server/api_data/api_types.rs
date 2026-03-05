use serde::Serialize;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct History {
    pub id: i64,
    pub time: i64,
    pub is_finished: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub update_time: time::OffsetDateTime,
}
