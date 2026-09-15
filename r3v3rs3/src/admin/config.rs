use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::config::{GetConfig, SetConfig};
use axum::{extract::State, Extension, Json};
use r3v3rs3_api::app::AppConfig;

/// Returns the application settings.
#[utoipa::path(
    get,
    path = "/",
    tag = "config",
    operation_id = "get_config",
    responses((status = 200, description = "The settings.", body = AppConfig), ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<AppConfig>>, AppError> {
    Ok(Json(state.call(&caller, GetConfig).await?))
}

/// Replaces the application settings.
#[utoipa::path(
    put,
    path = "/",
    tag = "config",
    operation_id = "update_config",
    request_body = AppConfig,
    responses((status = 200, description = "The settings are saved."), ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(config): Json<AppConfig>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, SetConfig { config }).await?))
}
