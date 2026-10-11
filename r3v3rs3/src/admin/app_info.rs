use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use axum::{Json, extract::State};
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
    let mut info = state.data.lock().await.app_info.clone();
    // The platform opens and closes while the server runs, so the state comes from the
    // server loop and not from the snapshot that the admin API read at its start.
    let platform = state
        .call_system(crate::server::rpc::auth::GetPlatform)
        .await?;
    info.platform_enabled = platform.get().is_ok();
    info.update_available = *state
        .call_system(crate::server::rpc::auth::GetUpdateAvailable)
        .await?;
    Ok(Json(info))
}
