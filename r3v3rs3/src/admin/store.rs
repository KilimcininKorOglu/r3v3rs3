use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::{
    accounts::{Caller, Permission},
    appstore,
    audit::AuditRecord,
    command::ServerCommand,
};
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::{audit::AuditAction, error::Error, store::StoreResponse};
use tracing::error;

/// Returns the app store catalog in use and its state.
#[utoipa::path(
    get,
    path = "/",
    tag = "store",
    operation_id = "get_store",
    responses((status = 200, description = "The app store catalog.", body = StoreResponse), ErrorResponses)
)]
pub async fn get() -> Json<StoreResponse> {
    Json(appstore::response())
}

/// Downloads the app store catalog from the repository and applies it.
#[utoipa::path(
    post,
    path = "/refresh",
    tag = "store",
    operation_id = "refresh_store",
    responses((status = 200, description = "The new app store catalog.", body = StoreResponse), ErrorResponses)
)]
pub async fn refresh(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<StoreResponse>, AppError> {
    caller.authorize(Permission::Edit)?;
    let catalog = match appstore::fetch::fetch_catalog().await {
        Ok(catalog) => catalog,
        Err(err) => {
            error!(%err, "failed to refresh the app store catalog");
            appstore::set_last_error(err.to_string());
            return Err(Error::FailedToRefreshAppStore.into());
        }
    };

    state
        .sender
        .send(ServerCommand::SetAppStore { catalog })
        .await
        .map_err(|_| Error::FailedToInvokeRpc)?;
    let record = AuditRecord::new(AuditAction::RefreshAppStore);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(appstore::response()))
}
