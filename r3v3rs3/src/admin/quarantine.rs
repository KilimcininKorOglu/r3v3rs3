//! Admin API handlers for threat detection and auto-quarantine management.

use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::quarantine::{
    GetQuarantineConfig, GetQuarantinedList, QuarantineIp, UnquarantineIp, UpdateQuarantineConfig,
};
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::quarantine::{AutoQuarantineConfig, QuarantineIpRequest, QuarantinedIpEntry};
use std::net::IpAddr;

/// Lists all currently quarantined IP addresses.
#[utoipa::path(
    get,
    path = "/",
    tag = "security",
    operation_id = "list_quarantined_ips",
    responses(
        (status = 200, description = "List of quarantined IP entries.", body = Vec<QuarantinedIpEntry>),
        ErrorResponses
    )
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<QuarantinedIpEntry>>>, AppError> {
    Ok(Json(state.call(&caller, GetQuarantinedList).await?))
}

/// Manually places an IP address into quarantine.
#[utoipa::path(
    post,
    path = "/",
    tag = "security",
    operation_id = "quarantine_ip",
    request_body = QuarantineIpRequest,
    responses(
        (status = 200, description = "The IP address was quarantined.", body = QuarantinedIpEntry),
        ErrorResponses
    )
)]
pub async fn add(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<QuarantineIpRequest>,
) -> Result<Json<Box<QuarantinedIpEntry>>, AppError> {
    let reason = request
        .reason
        .unwrap_or_else(|| "Manual quarantine via Admin API".to_string());
    let duration = request.duration_seconds.unwrap_or(3600);
    Ok(Json(
        state
            .call(
                &caller,
                QuarantineIp {
                    ip: request.ip,
                    reason,
                    duration_seconds: duration,
                },
            )
            .await?,
    ))
}

/// Releases an IP address from quarantine.
#[utoipa::path(
    delete,
    path = "/{ip}",
    tag = "security",
    operation_id = "unquarantine_ip",
    params(("ip" = String, Path, description = "IP address to release.")),
    responses(
        (status = 200, description = "The IP address was released from quarantine.", body = bool),
        ErrorResponses
    )
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(ip_str): Path<String>,
) -> Result<Json<Box<bool>>, AppError> {
    let ip: IpAddr = ip_str
        .parse()
        .map_err(|_| Error::InvalidAutoQuarantineConfig {
            reason: format!("invalid IP address: {ip_str}"),
        })?;
    Ok(Json(state.call(&caller, UnquarantineIp { ip }).await?))
}

/// Gets the auto-quarantine threat detection configuration.
#[utoipa::path(
    get,
    path = "/config",
    tag = "security",
    operation_id = "get_quarantine_config",
    responses(
        (status = 200, description = "Auto-quarantine configuration.", body = AutoQuarantineConfig),
        ErrorResponses
    )
)]
pub async fn get_config(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<AutoQuarantineConfig>>, AppError> {
    Ok(Json(state.call(&caller, GetQuarantineConfig).await?))
}

/// Updates and persists the auto-quarantine threat detection configuration.
#[utoipa::path(
    put,
    path = "/config",
    tag = "security",
    operation_id = "update_quarantine_config",
    request_body = AutoQuarantineConfig,
    responses(
        (status = 200, description = "Auto-quarantine configuration updated.", body = ()),
        ErrorResponses
    )
)]
pub async fn update_config(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(config): Json<AutoQuarantineConfig>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(
        state
            .call(&caller, UpdateQuarantineConfig { config })
            .await?,
    ))
}
