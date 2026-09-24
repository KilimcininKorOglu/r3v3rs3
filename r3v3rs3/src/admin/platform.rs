//! The apps and the targets of the deployment platform. The handlers call the platform directly,
//! outside the server loop, so they check the permission and write the audit log themselves.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::clock::unix_ms;
use crate::platform::{DEFAULT_LOG_TAIL, Platform};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
};
use r3v3rs3_api::{
    audit::AuditAction,
    error::Error,
    id::ShortId,
    platform::{
        AppEntry, AppLog, AppLogQuery, AppRequest, DeploymentEntry, DeploymentTrigger, EnvEntry,
        GitTokenRequest, TargetEntry, TargetRequest, TargetToken, WebhookSecret,
    },
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
pub(super) fn platform_error(err: anyhow::Error) -> AppError {
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

/// Adds an agent target and returns its one-time enrollment token.
#[utoipa::path(
    post,
    path = "/",
    tag = "platform",
    operation_id = "add_target",
    request_body = TargetRequest,
    responses((status = 200, description = "The target with its enrollment token.", body = TargetToken), ErrorResponses)
)]
pub async fn add_target(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<TargetRequest>,
) -> Result<Json<TargetToken>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let created = platform
        .add_target(request, unix_ms())
        .await
        .map_err(platform_error)?;
    // The summary names the target, never the token.
    let record = AuditRecord::new(AuditAction::AddTarget)
        .id(created.target.id)
        .summary(created.target.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(created))
}

/// Replaces the enrollment token of an agent target. An agent that enrolls with the new token
/// replaces the enrolled agent.
#[utoipa::path(
    post,
    path = "/{id}/token",
    tag = "platform",
    operation_id = "new_target_token",
    params(("id" = ShortId, Path, description = "Target id.")),
    responses((status = 200, description = "The target with its new enrollment token.", body = TargetToken), NotFoundResponse, ErrorResponses)
)]
pub async fn new_target_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<TargetToken>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let created = platform
        .new_target_token(id)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::NewTargetToken)
        .id(id)
        .summary(created.target.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(created))
}

/// Deletes an agent target without apps and closes the connection of its agent.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "platform",
    operation_id = "delete_target",
    params(("id" = ShortId, Path, description = "Target id.")),
    responses((status = 200, description = "The target is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete_target(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let target = platform.delete_target(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteTarget)
        .id(id)
        .summary(target.name);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(()))
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

/// Sets the token that clones the private repository of a Git app. The admin API never returns
/// the token.
#[utoipa::path(
    put,
    path = "/{id}/git_token",
    tag = "platform",
    operation_id = "set_app_git_token",
    params(("id" = ShortId, Path, description = "App id.")),
    request_body = GitTokenRequest,
    responses((status = 200, description = "The app.", body = AppEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn put_git_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<GitTokenRequest>,
) -> Result<Json<AppEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let app = platform
        .set_git_token(id, &request.token)
        .await
        .map_err(platform_error)?;
    // The summary names the app, never the token.
    let record = AuditRecord::new(AuditAction::SetAppGitToken)
        .id(id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(app))
}

/// Deletes the Git token of an app.
#[utoipa::path(
    delete,
    path = "/{id}/git_token",
    tag = "platform",
    operation_id = "delete_app_git_token",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The app.", body = AppEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn delete_git_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<AppEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let app = platform
        .delete_git_token(id)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteAppGitToken)
        .id(id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(app))
}

/// Creates a new webhook secret of an app, so that the signed requests of `POST
/// /hooks/apps/{id}` deploy it. The response is the only one that holds the secret, and the old
/// secret stops working.
#[utoipa::path(
    post,
    path = "/{id}/webhook_secret",
    tag = "platform",
    operation_id = "new_app_webhook_secret",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The new secret.", body = WebhookSecret), NotFoundResponse, ErrorResponses)
)]
pub async fn new_webhook_secret(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<WebhookSecret>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let (app, secret) = platform
        .new_webhook_secret(id)
        .await
        .map_err(platform_error)?;
    // The summary names the app, never the secret.
    let record = AuditRecord::new(AuditAction::NewAppWebhookSecret)
        .id(id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(secret))
}

/// Deletes the webhook secret of an app, so that no webhook request deploys it.
#[utoipa::path(
    delete,
    path = "/{id}/webhook_secret",
    tag = "platform",
    operation_id = "delete_app_webhook_secret",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The app.", body = AppEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn delete_webhook_secret(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<AppEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let app = platform
        .delete_webhook_secret(id)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteAppWebhookSecret)
        .id(id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(app))
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

/// Returns the last lines of the container log of the running deployment of an app.
#[utoipa::path(
    get,
    path = "/{id}/logs",
    tag = "platform",
    operation_id = "get_app_logs",
    params(("id" = ShortId, Path, description = "App id."), AppLogQuery),
    responses((status = 200, description = "The container log.", body = AppLog), NotFoundResponse, ErrorResponses)
)]
pub async fn get_logs(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<AppLogQuery>,
) -> Result<Json<AppLog>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let tail = query.tail.unwrap_or(DEFAULT_LOG_TAIL);
    Ok(Json(
        platform.app_log(id, tail).await.map_err(platform_error)?,
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
