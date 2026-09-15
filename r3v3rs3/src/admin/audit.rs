use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::audit::AuditFilter;
use crate::clock::unix_ms;
use crate::server::rpc::auth::GetAuditLog;
use axum::{
    extract::{Query, State},
    Extension, Json,
};
use r3v3rs3_api::{
    audit::{AuditEntry, AuditQuery},
    error::Error,
};
use tracing::error;

/// Lists the audit log entries, newest first. A cluster reads at most 31 days before `until`.
#[utoipa::path(
    get,
    path = "/",
    tag = "audit",
    operation_id = "list_audit_log",
    params(AuditQuery),
    responses((status = 200, description = "The audit log entries.", body = Vec<AuditEntry>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEntry>>, AppError> {
    let audit = state.call(&caller, GetAuditLog).await?;
    let filter = AuditFilter::new(&query, unix_ms());
    let entries = audit.query(&filter).await.map_err(|err| {
        error!("failed to read the audit log: {err:#}");
        Error::FailedToFetchLog
    })?;
    Ok(Json(entries))
}
