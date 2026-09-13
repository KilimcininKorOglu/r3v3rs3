use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use axum::{extract::State, Json};
use r3v3rs3_api::app::AppInfo;

/// Returns the version, build and path information of the server.
#[utoipa::path(
    get,
    path = "/",
    tag = "app_info",
    operation_id = "get_app_info",
    responses((status = 200, description = "The server information.", body = AppInfo), ErrorResponses)
)]
pub async fn get(State(state): State<AppState>) -> Result<Json<AppInfo>, AppError> {
    Ok(Json(state.data.lock().await.app_info.clone()))
}
