use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::cluster::GetClusterStatus;
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::cluster::ClusterStatus;

/// Shows how this node follows the cluster store.
#[utoipa::path(
    get,
    path = "/status",
    tag = "cluster",
    operation_id = "get_cluster_status",
    responses((status = 200, description = "The cluster status of this node.", body = ClusterStatus), ErrorResponses)
)]
pub async fn status(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<ClusterStatus>>, AppError> {
    Ok(Json(state.call(&caller, GetClusterStatus).await?))
}
