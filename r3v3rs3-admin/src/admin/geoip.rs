//! Admin API handlers for GeoIP status, settings, and database updates.

use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::geoip::{GetGeoIpStatus, RefreshGeoIpDatabase, UpdateGeoIpConfig};
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::geoip::{GeoIpStatus, UpdateGeoIpConfigRequest};

/// Gets the current status and metadata of the loaded GeoIP database.
#[utoipa::path(
    get,
    path = "/status",
    tag = "security",
    operation_id = "get_geoip_status",
    responses(
        (status = 200, description = "Status and metadata of the loaded GeoIP database.", body = GeoIpStatus),
        ErrorResponses
    )
)]
pub async fn status(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<GeoIpStatus>>, AppError> {
    Ok(Json(state.call(&caller, GetGeoIpStatus).await?))
}

/// Updates the GeoIP configuration.
#[utoipa::path(
    put,
    path = "/config",
    tag = "security",
    operation_id = "update_geoip_config",
    request_body = UpdateGeoIpConfigRequest,
    responses(
        (status = 200, description = "GeoIP configuration updated.", body = GeoIpStatus),
        ErrorResponses
    )
)]
pub async fn update_config(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<UpdateGeoIpConfigRequest>,
) -> Result<Json<Box<GeoIpStatus>>, AppError> {
    Ok(Json(
        state.call(&caller, UpdateGeoIpConfig { request }).await?,
    ))
}

/// Downloads and hot-reloads the latest GeoLite2-Country database from MaxMind.
#[utoipa::path(
    post,
    path = "/update",
    tag = "security",
    operation_id = "update_geoip_database",
    responses(
        (status = 200, description = "GeoIP database downloaded and hot-reloaded successfully.", body = GeoIpStatus),
        ErrorResponses
    )
)]
pub async fn update_database(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<GeoIpStatus>>, AppError> {
    Ok(Json(state.call(&caller, RefreshGeoIpDatabase).await?))
}
