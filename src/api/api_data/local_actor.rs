#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct LocalActorData {
    pub id: i64,
}
