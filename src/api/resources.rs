use axum::{extract::State, routing::get};

use crate::{api::Json, app_state::AppState, resources};

/// Server resources
#[utoipa::path(
    get,
    path = "/api/resources",
    responses(
        (status = 200, body = resources::Resources),
    ),
    tag = "Resources",
)]
pub async fn resources(
    State(AppState { db, .. }): State<AppState>,
) -> crate::Result<Json<resources::Resources>> {
    Ok(Json(resources::fetch(db.clone()).await?))
}

pub fn router() -> axum::Router<AppState> {
    axum::Router::new().route("/", get(resources))
}
