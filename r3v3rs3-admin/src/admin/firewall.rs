use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::firewall::GetFirewallStatus;
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::firewall::FirewallStatus;

/// Shows the host firewall table that r3v3rs3 keeps: the open ports, the warnings about other
/// firewalls of the host and the error of the last update.
#[utoipa::path(
    get,
    path = "/",
    tag = "firewall",
    operation_id = "get_firewall_status",
    responses((status = 200, description = "The state of the firewall table.", body = FirewallStatus), ErrorResponses)
)]
pub async fn status(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<FirewallStatus>>, AppError> {
    Ok(Json(state.call(&caller, GetFirewallStatus).await?))
}
