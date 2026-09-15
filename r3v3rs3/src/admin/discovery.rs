use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::discovery::GetDiscoveryStatus;
use axum::{extract::State, Extension, Json};
use r3v3rs3_api::discovery::DiscoveryStatus;

/// Lists the state, the proxy count and the issues of each discovery provider that has sent a
/// result.
#[utoipa::path(
    get,
    path = "/",
    tag = "discovery",
    operation_id = "list_discovery_status",
    responses((status = 200, description = "The discovery provider statuses.", body = Vec<DiscoveryStatus>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<Vec<DiscoveryStatus>>>, AppError> {
    Ok(Json(state.call(&caller, GetDiscoveryStatus).await?))
}
