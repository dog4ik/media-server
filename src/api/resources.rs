use axum::extract::State;

use crate::{api::Json, app_state::AppState, resources};
use utoipa_axum::{router::OpenApiRouter, routes};

/// Server resources
#[utoipa::path(
    get,
    path = "/resources",
    responses(
        (status = 200, body = resources::Resources),
    ),
    tag = "Resources",
)]
async fn resources(
    State(AppState { db, .. }): State<AppState>,
) -> crate::Result<Json<resources::Resources>> {
    Ok(Json(resources::fetch(db.clone()).await?))
}

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(resources))
}
