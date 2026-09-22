//! The apps and the targets of the deployment platform. The handlers call the platform directly,
//! outside the server loop, so they check the permission and write the audit log themselves.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::clock::unix_ms;
use crate::platform::Platform;
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{
    audit::AuditAction,
    error::Error,
    id::ShortId,
    platform::{AppEntry, AppRequest, DeploymentEntry, DeploymentTrigger, EnvEntry, TargetEntry},
};
use std::sync::Arc;
use tracing::error;

impl AppState {
    /// The platform, after the permission check of the caller.
    async fn platform(
        &self,
        caller: &Caller,
        permission: Permission,
    ) -> Result<Arc<Platform>, AppError> {
        caller.authorize_platform(permission)?;
        Ok(self.data.lock().await.platform.get()?.clone())
    }
}

/// An API error keeps its status code. Any other failure is logged, because its text can hold
/// file paths of the server.
fn platform_error(err: anyhow::Error) -> AppError {
    match err.downcast::<Error>() {
        Ok(err) => AppError::R3v3rs3(err),
        Err(err) => {
            error!("the deployment platform failed: {err:#}");
            AppError::Anyhow(anyhow::anyhow!(
                "the deployment platform failed, see the server log"
            ))
        }
    }
}

/// Lists the Docker hosts that run apps.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "list_targets",
    responses((status = 200, description = "The targets.", body = Vec<TargetEntry>), ErrorResponses)
)]
pub async fn list_targets(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<TargetEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(platform.targets().await.map_err(platform_error)?))
}

/// Lists the apps.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "list_apps",
    responses((status = 200, description = "The apps.", body = Vec<AppEntry>), ErrorResponses)
)]
pub async fn list_apps(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<AppEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(platform.apps().await.map_err(platform_error)?))
}

/// Adds an app. The app runs after its first deployment.
#[utoipa::path(
    post,
    path = "/",
    tag = "platform",
    operation_id = "add_app",
    request_body = AppRequest,
    responses((status = 200, description = "The new app.", body = AppEntry), ErrorResponses)
)]
pub async fn add_app(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<AppRequest>,
) -> Result<Json<AppEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let app = platform
        .add_app(request, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::AddApp)
        .id(app.id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(app))
}

/// Returns an app.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "platform",
    operation_id = "get_app",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The app.", body = AppEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn get_app(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<AppEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(platform.app(id).await.map_err(platform_error)?))
}

/// Replaces the name, the target and the spec of an app. The running containers change at the
/// next deployment.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "platform",
    operation_id = "update_app",
    params(("id" = ShortId, Path, description = "App id.")),
    request_body = AppRequest,
    responses((status = 200, description = "The updated app.", body = AppEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn update_app(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<AppRequest>,
) -> Result<Json<AppEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let app = platform
        .update_app(id, request, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateApp)
        .id(app.id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(app))
}

/// Deletes an app with its environment variables and its deployments.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "platform",
    operation_id = "delete_app",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The app is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete_app(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let app = platform.delete_app(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteApp)
        .id(app.id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(()))
}

/// Lists the environment variables of an app. A secret variable has no value in the response.
#[utoipa::path(
    get,
    path = "/{id}/env",
    tag = "platform",
    operation_id = "get_app_env",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The environment variables.", body = Vec<EnvEntry>), NotFoundResponse, ErrorResponses)
)]
pub async fn get_env(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Vec<EnvEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    Ok(Json(platform.env(id).await.map_err(platform_error)?))
}

/// Replaces the environment variables of an app. A secret variable without a value keeps its
/// current value. The running containers change at the next deployment.
#[utoipa::path(
    put,
    path = "/{id}/env",
    tag = "platform",
    operation_id = "update_app_env",
    params(("id" = ShortId, Path, description = "App id.")),
    request_body = Vec<EnvEntry>,
    responses((status = 200, description = "The environment variables are updated."), NotFoundResponse, ErrorResponses)
)]
pub async fn put_env(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(entries): Json<Vec<EnvEntry>>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    // The summary names the keys only, never a value.
    let keys = entries
        .iter()
        .map(|entry| entry.key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    platform
        .set_env(id, entries)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateAppEnv)
        .id(id)
        .summary(keys);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(()))
}

/// Lists the latest deployments of an app, the newest first.
#[utoipa::path(
    get,
    path = "/{id}/deployments",
    tag = "platform",
    operation_id = "list_app_deployments",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The deployments.", body = Vec<DeploymentEntry>), NotFoundResponse, ErrorResponses)
)]
pub async fn list_deployments(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Vec<DeploymentEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(
        platform.deployments(id).await.map_err(platform_error)?,
    ))
}

/// Starts a deployment of the current spec and environment of an app. The response returns the
/// queued deployment; `GET /api/deployments/{id}` shows its progress.
#[utoipa::path(
    post,
    path = "/{id}/deploy",
    tag = "platform",
    operation_id = "deploy_app",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The queued deployment.", body = DeploymentEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn deploy_app(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<DeploymentEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let deployment = platform
        .deploy(id, &caller.username, DeploymentTrigger::Manual)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeployApp)
        .id(id)
        .summary(deployment.id.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(deployment))
}

/// Returns a deployment.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "platform",
    operation_id = "get_deployment",
    params(("id" = ShortId, Path, description = "Deployment id.")),
    responses((status = 200, description = "The deployment.", body = DeploymentEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn get_deployment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<DeploymentEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(platform.deployment(id).await.map_err(platform_error)?))
}

/// Starts a deployment that repeats the image digest, the spec and the environment of an earlier
/// deployment.
#[utoipa::path(
    post,
    path = "/{id}/rollback",
    tag = "platform",
    operation_id = "rollback_deployment",
    params(("id" = ShortId, Path, description = "The id of the deployment to repeat.")),
    responses((status = 200, description = "The queued deployment.", body = DeploymentEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn rollback_deployment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<DeploymentEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let deployment = platform
        .rollback(id, &caller.username)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::RollbackApp)
        .id(deployment.app)
        .summary(format!("{id} -> {}", deployment.id));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(deployment))
}
