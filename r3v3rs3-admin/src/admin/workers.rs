use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::server::rpc::workers::GetWorkers;
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::app::WorkerStatusReport;

/// Lists the workers that serve the ports of this master.
#[utoipa::path(
    get,
    path = "/",
    tag = "workers",
    operation_id = "get_workers",
    responses((status = 200, description = "The workers of this master.", body = WorkerStatusReport), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<WorkerStatusReport>>, AppError> {
    Ok(Json(state.call(&caller, GetWorkers).await?))
}
