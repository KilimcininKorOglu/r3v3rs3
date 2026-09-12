use super::{AppError, AppState};
use axum::{extract::State, Json};
use r3v3rs3_api::app::AppInfo;

pub async fn get(State(state): State<AppState>) -> Result<Json<AppInfo>, AppError> {
    Ok(Json(state.data.lock().await.app_info.clone()))
}
