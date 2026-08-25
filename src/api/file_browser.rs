use crate::app_state::AppState;
use crate::{
    AppError,
    api::{Json, Path},
    file_browser::{BrowseDirectory, BrowseFile, BrowseRootDirs, FileKey},
};
use utoipa_axum::{router::OpenApiRouter, routes};

/// Root and other related directories/drives
#[utoipa::path(
    get,
    path = "/file_browser/root_dirs",
    responses(
        (status = 200, body = BrowseRootDirs),
    ),
    tag = "FileBrowser",
)]
async fn root_dirs() -> Json<BrowseRootDirs> {
    Json(BrowseRootDirs::new())
}

/// Browse internals of the given directory
#[utoipa::path(
    get,
    path = "/file_browser/browse/{key}",
    params(
        ("key" = String, description = "Key of directory to explore. It is base64 encoded path in current implementation"),
    ),
    responses(
        (status = 200, body = BrowseDirectory),
        (status = 404, body = AppError, description = "Directory is not found"),
        (status = 500, body = AppError, description = "Invalid permissions, other errors"),
    ),
    tag = "FileBrowser",
)]
async fn browse_directory(Path(key): Path<FileKey>) -> crate::Result<Json<BrowseDirectory>> {
    let resolved_dir = BrowseDirectory::explore(key).await?;
    Ok(Json(resolved_dir))
}

/// Get parent directory. Returns same directory if parent is not found
#[utoipa::path(
    get,
    path = "/file_browser/parent/{key}",
    params(
        ("key" = String, description = "Get parent directory"),
    ),
    responses(
        (status = 200, body = BrowseFile),
    ),
    tag = "FileBrowser",
)]
async fn parent_directory(Path(mut key): Path<FileKey>) -> crate::Result<Json<BrowseFile>> {
    if let Some(parent) = key.path.parent() {
        key.path = parent.to_owned();
    }
    let resolved_dir = BrowseFile::from(key.path);
    Ok(Json(resolved_dir))
}

pub(super) fn router() -> OpenApiRouter<AppState> {
    OpenApiRouter::new()
        .routes(routes!(browse_directory))
        .routes(routes!(parent_directory))
        .routes(routes!(root_dirs))
}
