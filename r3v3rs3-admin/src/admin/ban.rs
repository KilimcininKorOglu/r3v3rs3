//! The admin API of the IP brute-force ban tracker (`crate::security::ban`).

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{audit::AuditAction, auth::BannedIpEntry, error::Error};
use std::net::IpAddr;
use std::time::Instant;

/// Lists the currently banned client IP addresses.
#[utoipa::path(
    get,
    path = "/",
    tag = "auth",
    operation_id = "list_banned_ips",
    responses((status = 200, description = "The banned IP addresses.", body = Vec<BannedIpEntry>), ErrorResponses)
)]
pub async fn list_banned_ips(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<BannedIpEntry>>, AppError> {
    caller.authorize(Permission::Admin)?;
    let mut data = state.data.lock().await;
    let list = data.ban_tracker.banned_ips(Instant::now());
    Ok(Json(list))
}

/// Unbans a client IP address.
#[utoipa::path(
    delete,
    path = "/{ip}",
    tag = "auth",
    operation_id = "unban_ip",
    params(("ip" = String, Path, description = "IP address to unban.")),
    responses((status = 200, description = "The IP address was unbanned."), NotFoundResponse, ErrorResponses)
)]
pub async fn unban_ip(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(ip_str): Path<String>,
) -> Result<Json<()>, AppError> {
    caller.authorize(Permission::Admin)?;
    let ip: IpAddr = ip_str
        .parse()
        .map_err(|_| Error::InvalidCidr { cidr: ip_str })?;
    let mut data = state.data.lock().await;
    let was_banned = data.ban_tracker.unban(&ip);
    drop(data);
    if was_banned {
        let record = AuditRecord::new(AuditAction::UnbanIp).id(ip.to_string());
        state
            .record_audit(&caller.username, caller.client, record)
            .await;
    }
    Ok(Json(()))
}
