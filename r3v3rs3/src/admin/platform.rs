//! The apps and the targets of the deployment platform. The handlers call the platform directly,
//! outside the server loop, so they check the permission and write the audit log themselves.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::clock::unix_ms;
use crate::platform::{DEFAULT_LOG_TAIL, MAX_LOG_TAIL, Platform};
use crate::runtime::docker::ExecHandle;
use axum::{
    Extension, Json,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Path, Query, State},
    response::Response,
};
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use r3v3rs3_api::{
    audit::AuditAction,
    container::{AppLogsOverview, AppResourceDiagnostics, LogVacuumResult},
    error::Error,
    id::ShortId,
    platform::{
        AppEntry, AppLog, AppLogQuery, AppMetricsPoint, AppRequest, AppScheduledCommand,
        ComposeServiceStatus, ComposeTopology, DeploymentEntry, DeploymentTrigger, EnvEntry,
        EnvGroup, EnvGroupSpec, EnvGroupSummary, EnvironmentDiffReport, EnvironmentEnvSpec,
        EnvironmentSpec, EnvironmentSyncReport, EnvironmentSyncRequest, GitTokenRequest,
        ImageUpdateStatus, KeyRotationReport, MetricsQuery, NodeAlertRules, Project,
        ProjectEnvironment, ProjectSpec, ProjectSummary, PrunePolicy, PruneResult, RegistryEntry,
        RegistryRequest, ScheduledCommandRequest, ScheduledCommandRunResult,
        SetTargetRegionRequest, TargetEntry, TargetRequest, TargetToken, WebhookSecret,
    },
};
use serde_derive::Deserialize;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::error;
use utoipa::IntoParams;

impl AppState {
    /// The platform, after the permission check of the caller.
    pub(super) async fn platform(
        &self,
        caller: &Caller,
        permission: Permission,
    ) -> Result<Arc<Platform>, AppError> {
        caller.authorize_platform(permission)?;
        // The settings page opens and closes the platform while the server runs, so the
        // handle comes from the server loop and not from the snapshot of the start.
        let handle = self
            .call_system(crate::server::rpc::auth::GetPlatform)
            .await?;
        Ok(handle.get()?.clone())
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

/// Upgrades the agent of a target by pushing the latest server binary to it.
#[utoipa::path(
    post,
    path = "/{id}/upgrade",
    tag = "platform",
    operation_id = "upgrade_target",
    params(("id" = ShortId, Path, description = "Target id.")),
    responses((status = 200, description = "The target with its updated state.", body = TargetEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn upgrade_target(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<TargetEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let updated = platform.upgrade_target(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpgradeTarget)
        .id(id)
        .summary(format!("{} -> {}", updated.name, env!("CARGO_PKG_VERSION")));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(updated))
}

/// Drains a target for maintenance, removing it from active load balancing pools.
#[utoipa::path(
    post,
    path = "/{id}/drain",
    tag = "platform",
    operation_id = "drain_target",
    params(("id" = ShortId, Path, description = "Target id.")),
    responses((status = 200, description = "The target with its drained state.", body = TargetEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn drain_target(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<TargetEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let updated = platform
        .set_target_drained(id, true)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DrainTarget)
        .id(id)
        .summary(updated.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(updated))
}

/// Restores a drained target, returning it to active load balancing pools.
#[utoipa::path(
    post,
    path = "/{id}/undrain",
    tag = "platform",
    operation_id = "undrain_target",
    params(("id" = ShortId, Path, description = "Target id.")),
    responses((status = 200, description = "The target with its active state.", body = TargetEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn undrain_target(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<TargetEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let updated = platform
        .set_target_drained(id, false)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UndrainTarget)
        .id(id)
        .summary(updated.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(updated))
}

/// Sets or clears the geographical region of a target node.
#[utoipa::path(
    put,
    path = "/{id}/region",
    tag = "platform",
    operation_id = "set_target_region",
    params(("id" = ShortId, Path, description = "Target id.")),
    request_body = SetTargetRegionRequest,
    responses((status = 200, description = "The target with its updated region.", body = TargetEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn set_target_region(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<SetTargetRegionRequest>,
) -> Result<Json<TargetEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let updated = platform
        .set_target_region(id, request.region)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::SetTargetRegion)
        .id(id)
        .summary(updated.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(updated))
}

/// Upgrades all connected agents by pushing the latest server binary to them.
#[utoipa::path(
    post,
    path = "/upgrade",
    tag = "platform",
    operation_id = "upgrade_all_targets",
    responses((status = 200, description = "The upgraded targets.", body = Vec<TargetEntry>), ErrorResponses)
)]
pub async fn upgrade_all_targets(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<TargetEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let upgraded = platform
        .upgrade_all_targets()
        .await
        .map_err(platform_error)?;
    Ok(Json(upgraded))
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
    let mut apps = platform.apps().await.map_err(platform_error)?;
    let catalog = crate::appstore::state().catalog.clone();
    for app in &mut apps {
        app.catalog_update = check_catalog_update(app, &catalog);
    }
    Ok(Json(apps))
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
    let mut app = platform.app(id).await.map_err(platform_error)?;
    let catalog = crate::appstore::state().catalog.clone();
    app.catalog_update = check_catalog_update(&app, &catalog);
    Ok(Json(app))
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

/// Installs the webhook of an app at the Git provider of its connection again. The old webhook
/// of the app is removed first. A failed installation shows in the `hook` of the returned app.
#[utoipa::path(
    post,
    path = "/{id}/hook",
    tag = "platform",
    operation_id = "install_app_hook",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The app with its webhook.", body = AppEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn install_hook(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<AppEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let app = platform
        .reinstall_app_hook(id, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::InstallAppHook)
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
        platform
            .app_log(id, tail, query.service.as_deref())
            .await
            .map_err(platform_error)?,
    ))
}

/// Returns the container log file sizes and overview for an app.
#[utoipa::path(
    get,
    path = "/{id}/logs/size",
    tag = "platform",
    operation_id = "get_app_log_size",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The container log sizes.", body = AppLogsOverview), NotFoundResponse, ErrorResponses)
)]
pub async fn get_log_size(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<AppLogsOverview>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(
        platform.app_log_size(id).await.map_err(platform_error)?,
    ))
}

/// Truncates (vacuums) the container log files of an app.
#[utoipa::path(
    post,
    path = "/{id}/logs/vacuum",
    tag = "platform",
    operation_id = "vacuum_app_logs",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The log vacuum result.", body = LogVacuumResult), NotFoundResponse, ErrorResponses)
)]
pub async fn vacuum_app_logs(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<LogVacuumResult>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let result = platform.vacuum_app_logs(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::VacuumLogs)
        .id(id)
        .summary(format!(
            "Vacuumed container logs, freed {} bytes",
            result.freed_bytes
        ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(result))
}

/// Returns the status and health of the services of a Compose app.
#[utoipa::path(
    get,
    path = "/{id}/services",
    tag = "platform",
    operation_id = "get_app_services",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The list of Compose services.", body = Vec<ComposeServiceStatus>), NotFoundResponse, ErrorResponses)
)]
pub async fn get_services(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Vec<ComposeServiceStatus>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(
        platform.app_services(id).await.map_err(platform_error)?,
    ))
}

/// Returns the architecture dependency graph and topology of a Compose app.
#[utoipa::path(
    get,
    path = "/{id}/topology",
    tag = "platform",
    operation_id = "get_app_topology",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The Compose architecture topology.", body = ComposeTopology), NotFoundResponse, ErrorResponses)
)]
pub async fn get_topology(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<ComposeTopology>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(
        platform.app_topology(id).await.map_err(platform_error)?,
    ))
}

/// Query for live container log streaming.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AppLogLiveQuery {
    /// Number of initial lines to stream.
    pub tail: Option<u32>,
    /// Optional service name to stream logs from in a Compose app.
    pub service: Option<String>,
}

/// Opens a WebSocket stream for live logs of the running container of an app.
#[utoipa::path(
    get,
    path = "/{id}/logs/live",
    tag = "platform",
    operation_id = "app_logs_live",
    params(
        ("id" = ShortId, Path, description = "App id."),
        AppLogLiveQuery
    ),
    responses((status = 101, description = "The live container log stream."), NotFoundResponse, ErrorResponses)
)]
pub async fn logs_live(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<AppLogLiveQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let tail = query
        .tail
        .unwrap_or(DEFAULT_LOG_TAIL)
        .clamp(1, MAX_LOG_TAIL);
    platform.app(id).await.map_err(platform_error)?;
    let service = query.service;
    Ok(ws.on_upgrade(move |socket| logs_live_bridge(socket, platform, id, tail, service)))
}

async fn logs_live_bridge(
    socket: WebSocket,
    platform: Arc<Platform>,
    id: ShortId,
    tail: u32,
    service: Option<String>,
) {
    let (sink, ws_stream) = socket.split();
    match platform.app_log_stream(id, tail, service.as_deref()).await {
        Ok((raw, response)) => {
            logs_stream_bridge(sink, ws_stream, raw, response).await;
        }
        Err(_) => {
            logs_poll_bridge(sink, ws_stream, platform, id, tail, service).await;
        }
    }
}

async fn logs_stream_bridge(
    mut sink: futures::stream::SplitSink<WebSocket, Message>,
    mut ws_stream: futures::stream::SplitStream<WebSocket>,
    raw: bool,
    response: hyper::Response<hyper::body::Incoming>,
) {
    let mut body = response.into_body();
    let mut buffer = Vec::new();

    let stream_to_client = async {
        while let Some(frame_result) = body.frame().await {
            let Ok(frame) = frame_result else { break };
            let Some(data) = frame.data_ref() else {
                continue;
            };
            if raw {
                let text = String::from_utf8_lossy(data);
                if sink
                    .send(Message::Text(text.into_owned().into()))
                    .await
                    .is_err()
                {
                    break;
                }
            } else {
                buffer.extend_from_slice(data);
                if send_demuxed_chunks(&mut sink, &mut buffer).await.is_err() {
                    break;
                }
            }
        }
    };

    let client_listener = async {
        while let Some(msg) = ws_stream.next().await {
            if matches!(msg, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    };

    tokio::select! {
        _ = stream_to_client => {}
        _ = client_listener => {}
    }
}

async fn send_demuxed_chunks(
    sink: &mut futures::stream::SplitSink<WebSocket, Message>,
    buffer: &mut Vec<u8>,
) -> Result<(), ()> {
    while buffer.len() >= 8 {
        let size = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;
        if buffer.len() < 8 + size {
            break;
        }
        let text = String::from_utf8_lossy(&buffer[8..8 + size]).into_owned();
        buffer.drain(..8 + size);
        if sink.send(Message::Text(text.into())).await.is_err() {
            return Err(());
        }
    }
    Ok(())
}

async fn logs_poll_bridge(
    mut sink: futures::stream::SplitSink<WebSocket, Message>,
    mut ws_stream: futures::stream::SplitStream<WebSocket>,
    platform: Arc<Platform>,
    id: ShortId,
    tail: u32,
    service: Option<String>,
) {
    let poller = async {
        if let Ok(app_log) = platform.app_log(id, tail, service.as_deref()).await
            && !app_log.log.is_empty()
        {
            let _ = sink.send(Message::Text(app_log.log.into())).await;
        }
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(1500));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let Ok(app_log) = platform.app_log(id, 20, service.as_deref()).await else {
                break;
            };
            if !app_log.running {
                break;
            }
            if !app_log.log.is_empty()
                && sink.send(Message::Text(app_log.log.into())).await.is_err()
            {
                break;
            }
        }
    };

    let client_listener = async {
        while let Some(msg) = ws_stream.next().await {
            if matches!(msg, Ok(Message::Close(_)) | Err(_)) {
                break;
            }
        }
    };

    tokio::select! {
        _ = poller => {}
        _ = client_listener => {}
    }
}

/// The recorded metrics of an app: the samples of the last hours, oldest first.
#[utoipa::path(
    get,
    path = "/{id}/metrics",
    tag = "platform",
    operation_id = "get_app_metrics",
    params(("id" = ShortId, Path, description = "App id."), MetricsQuery),
    responses((status = 200, description = "The metrics samples.", body = Vec<AppMetricsPoint>), NotFoundResponse, ErrorResponses)
)]
pub async fn get_app_metrics(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<MetricsQuery>,
) -> Result<Json<Vec<AppMetricsPoint>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let hours = query.hours.unwrap_or(6).clamp(1, 24);
    Ok(Json(
        platform
            .app_metrics(id, hours)
            .await
            .map_err(platform_error)?,
    ))
}

/// Shows live container resource throttling, CFS quota, and OOM diagnostic status of an app.
#[utoipa::path(
    get,
    path = "/{id}/diagnostics",
    tag = "platform",
    operation_id = "get_app_resource_diagnostics",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The resource diagnostics.", body = AppResourceDiagnostics), NotFoundResponse, ErrorResponses)
)]
pub async fn get_app_resource_diagnostics(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<AppResourceDiagnostics>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(
        platform
            .app_resource_diagnostics(id)
            .await
            .map_err(platform_error)?,
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

/// Checks Docker/OCI registry for updates to the image of an app. If auto-deploy is enabled and an update is found, triggers deployment.
#[utoipa::path(
    post,
    path = "/{id}/check_update",
    tag = "platform",
    operation_id = "check_app_update",
    params(("id" = ShortId, Path, description = "App id.")),
    responses((status = 200, description = "The image update status.", body = ImageUpdateStatus), NotFoundResponse, ErrorResponses)
)]
pub async fn check_app_update(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<ImageUpdateStatus>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let status = platform
        .check_app_update(id)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::CheckAppUpdate)
        .id(id)
        .summary(format!("update_available: {}", status.update_available));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(status))
}

/// Opens a WebSocket terminal into the running container of an app. Binary frames carry the
/// terminal data both ways, and a text frame with `{"cols":N,"rows":N}` resizes the terminal.
#[utoipa::path(
    get,
    path = "/{id}/terminal",
    tag = "platform",
    operation_id = "open_app_terminal",
    params(
        ("id" = ShortId, Path, description = "App id."),
        TerminalQuery
    ),
    responses((status = 101, description = "The terminal stream of the container."), NotFoundResponse, ErrorResponses)
)]
pub async fn terminal(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<TerminalQuery>,
    ws: WebSocketUpgrade,
) -> Result<Response, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let (container, exec) = platform
        .app_terminal(id, query.cols(), query.rows(), query.shell.as_deref())
        .await
        .map_err(platform_error)?;
    let summary = match &query.shell {
        Some(shell) if !shell.trim().is_empty() => format!("{container} ({shell})"),
        _ => container,
    };
    let record = AuditRecord::new(AuditAction::ExecApp)
        .id(id)
        .summary(summary);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(ws.on_upgrade(move |socket| terminal_bridge(socket, exec)))
}

/// The size and shell configuration of the terminal that the page opens.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TerminalQuery {
    /// The columns of the terminal. The default is 80.
    pub cols: Option<u16>,
    /// The rows of the terminal. The default is 24.
    pub rows: Option<u16>,
    /// Optional shell or CLI command (e.g. `sh`, `bash`, `zsh`, `psql`, `mysql`, `mariadb`, `redis-cli`, `mongosh`, `cli`).
    pub shell: Option<String>,
}

impl TerminalQuery {
    fn cols(&self) -> u16 {
        self.cols.unwrap_or(80).clamp(2, 500)
    }

    fn rows(&self) -> u16 {
        self.rows.unwrap_or(24).clamp(2, 200)
    }
}

/// Bridges a WebSocket and the exec stream of a terminal until either side closes.
async fn terminal_bridge(socket: WebSocket, exec: ExecHandle) {
    let ExecHandle { stream, client, id } = exec;
    let (mut sink, mut ws_stream) = socket.split();
    let (mut reader, mut writer) = tokio::io::split(stream);
    let read = async {
        let mut chunk = [0u8; 8192];
        loop {
            let size = reader.read(&mut chunk).await?;
            if size == 0 {
                break;
            }
            if sink
                .send(Message::Binary(Bytes::copy_from_slice(&chunk[..size])))
                .await
                .is_err()
            {
                break;
            }
        }
        anyhow::Ok(())
    };
    let write = async {
        while let Some(message) = ws_stream.next().await {
            match message? {
                Message::Binary(data) => writer.write_all(&data).await?,
                Message::Text(text) => {
                    if let Ok(size) = serde_json::from_str::<TerminalSize>(&text) {
                        crate::runtime::docker::resize_exec(&client, &id, size.cols(), size.rows())
                            .await?;
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
        anyhow::Ok::<()>(())
    };
    tokio::select! {
        _ = read => {}
        _ = write => {}
    }
}

/// The resize message of the terminal page.
#[derive(Deserialize)]
struct TerminalSize {
    cols: u16,
    rows: u16,
}

impl TerminalSize {
    fn cols(&self) -> u16 {
        self.cols.clamp(2, 500)
    }

    fn rows(&self) -> u16 {
        self.rows.clamp(2, 200)
    }
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

/// Approves a pending deployment and starts its pipeline.
#[utoipa::path(
    post,
    path = "/{id}/approve",
    tag = "platform",
    operation_id = "approve_deployment",
    params(("id" = ShortId, Path, description = "The id of the pending deployment.")),
    responses((status = 200, description = "The approved deployment.", body = DeploymentEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn approve_deployment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<DeploymentEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let deployment = platform
        .approve_deployment(id, &caller.username)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::ApproveDeployment)
        .id(deployment.app)
        .summary(id.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(deployment))
}

/// Rejects a pending deployment.
#[utoipa::path(
    post,
    path = "/{id}/reject",
    tag = "platform",
    operation_id = "reject_deployment",
    params(("id" = ShortId, Path, description = "The id of the pending deployment.")),
    responses((status = 200, description = "The rejected deployment.", body = DeploymentEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn reject_deployment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<DeploymentEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let deployment = platform
        .reject_deployment(id, &caller.username)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::RejectDeployment)
        .id(deployment.app)
        .summary(id.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(deployment))
}

/// Lists the scheduled commands configured for an app.
#[utoipa::path(
    get,
    path = "/{id}/commands",
    tag = "platform",
    operation_id = "get_app_scheduled_commands",
    params(("id" = ShortId, Path, description = "The app id.")),
    responses((status = 200, description = "The scheduled commands.", body = Vec<AppScheduledCommand>), NotFoundResponse, ErrorResponses)
)]
pub async fn get_app_scheduled_commands(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Vec<AppScheduledCommand>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let list = platform
        .scheduled_commands(id)
        .await
        .map_err(platform_error)?;
    Ok(Json(list))
}

/// Adds a new scheduled command to an app.
#[utoipa::path(
    post,
    path = "/{id}/commands",
    tag = "platform",
    operation_id = "create_app_scheduled_command",
    params(("id" = ShortId, Path, description = "The app id.")),
    request_body = ScheduledCommandRequest,
    responses((status = 200, description = "The created scheduled command.", body = AppScheduledCommand), NotFoundResponse, ErrorResponses)
)]
pub async fn create_app_scheduled_command(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<ScheduledCommandRequest>,
) -> Result<Json<AppScheduledCommand>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let cmd = platform
        .add_scheduled_command(id, request)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::AddScheduledCommand)
        .id(id)
        .summary(&cmd.name);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(cmd))
}

/// Updates a scheduled command.
#[utoipa::path(
    put,
    path = "/{id}/commands/{cmd_id}",
    tag = "platform",
    operation_id = "update_app_scheduled_command",
    params(
        ("id" = ShortId, Path, description = "The app id."),
        ("cmd_id" = ShortId, Path, description = "The command id.")
    ),
    request_body = ScheduledCommandRequest,
    responses((status = 200, description = "The updated scheduled command.", body = AppScheduledCommand), NotFoundResponse, ErrorResponses)
)]
pub async fn update_app_scheduled_command(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, cmd_id)): Path<(ShortId, ShortId)>,
    Json(request): Json<ScheduledCommandRequest>,
) -> Result<Json<AppScheduledCommand>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let cmd = platform
        .update_scheduled_command(cmd_id, request)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateScheduledCommand)
        .id(id)
        .summary(&cmd.name);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(cmd))
}

/// Deletes a scheduled command.
#[utoipa::path(
    delete,
    path = "/{id}/commands/{cmd_id}",
    tag = "platform",
    operation_id = "delete_app_scheduled_command",
    params(
        ("id" = ShortId, Path, description = "The app id."),
        ("cmd_id" = ShortId, Path, description = "The command id.")
    ),
    responses((status = 200, description = "Command deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete_app_scheduled_command(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, cmd_id)): Path<(ShortId, ShortId)>,
) -> Result<(), AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    platform
        .delete_scheduled_command(cmd_id)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteScheduledCommand)
        .id(id)
        .summary(cmd_id.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(())
}

/// Runs a scheduled command immediately inside the running container.
#[utoipa::path(
    post,
    path = "/{id}/commands/{cmd_id}/run",
    tag = "platform",
    operation_id = "run_app_scheduled_command",
    params(
        ("id" = ShortId, Path, description = "The app id."),
        ("cmd_id" = ShortId, Path, description = "The command id.")
    ),
    responses((status = 200, description = "Execution result.", body = ScheduledCommandRunResult), NotFoundResponse, ErrorResponses)
)]
pub async fn run_app_scheduled_command(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, cmd_id)): Path<(ShortId, ShortId)>,
) -> Result<Json<ScheduledCommandRunResult>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let result = platform
        .execute_scheduled_command(cmd_id)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::RunScheduledCommand)
        .id(id)
        .summary(cmd_id.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(result))
}

/// Upgrades an app to the newest version in the app store catalog and starts a deployment.
#[utoipa::path(
    post,
    path = "/{id}/upgrade-catalog",
    tag = "platform",
    operation_id = "upgrade_app_catalog",
    params(("id" = ShortId, Path, description = "The app id.")),
    responses((status = 200, description = "The upgraded app.", body = AppEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn upgrade_app_catalog(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<AppEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let mut app = platform.app(id).await.map_err(platform_error)?;
    let catalog = crate::appstore::state().catalog.clone();
    let Some(store_app) = find_catalog_app(&app.spec, &catalog) else {
        return Err(Error::IdNotFound { id: id.to_string() }.into());
    };
    apply_catalog_upgrade(&mut app.spec, store_app)?;
    let req = AppRequest {
        name: app.name.clone(),
        target: app.target,
        spec: app.spec.clone(),
    };
    let mut updated = platform
        .update_app(id, req, unix_ms())
        .await
        .map_err(platform_error)?;
    let _ = platform
        .deploy(id, &caller.username, DeploymentTrigger::Manual)
        .await;
    let record = AuditRecord::new(AuditAction::UpgradeAppFromCatalog)
        .id(id)
        .summary(format!("{} -> {}", app.name, store_app.id));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    updated.catalog_update = None;
    Ok(Json(updated))
}

fn find_catalog_app<'a>(
    spec: &r3v3rs3_api::platform::AppSpec,
    catalog: &'a r3v3rs3_api::store::StoreCatalog,
) -> Option<&'a r3v3rs3_api::store::StoreApp> {
    if let Some(ref store_id) = spec.store_app_id {
        return catalog.apps.iter().find(|a| &a.id == store_id);
    }
    match &spec.source {
        r3v3rs3_api::platform::AppSource::Image { image } => {
            let current_repo = image.repository();
            catalog.apps.iter().find(|a| {
                if let Some(ref cat_img) = a.image
                    && let Ok(cat_parsed) = cat_img.parse::<r3v3rs3_api::container::ImageRef>()
                {
                    return cat_parsed.repository() == current_repo;
                }
                false
            })
        }
        r3v3rs3_api::platform::AppSource::Compose { file, .. } => {
            let file_str = file.as_ref().map(|f| f.as_str()).unwrap_or("");
            catalog
                .apps
                .iter()
                .find(|a| a.file.as_deref().unwrap_or("") == file_str)
        }
        _ => None,
    }
}

fn diff_catalog_version(
    source: &r3v3rs3_api::platform::AppSource,
    store_app: &r3v3rs3_api::store::StoreApp,
) -> Option<(String, String)> {
    match source {
        r3v3rs3_api::platform::AppSource::Image { image } => {
            let cat_img = store_app.image.as_ref()?;
            if image.as_str() != cat_img {
                Some((image.as_str().to_string(), cat_img.clone()))
            } else {
                None
            }
        }
        r3v3rs3_api::platform::AppSource::Compose { branch, file, .. } => {
            let cat_file = store_app.file.as_deref().unwrap_or_default();
            let cat_branch = store_app.branch.as_deref().unwrap_or("main");
            let file_str = file.as_ref().map(|f| f.as_str()).unwrap_or_default();
            if file_str != cat_file || branch.as_str() != cat_branch {
                Some((
                    format!("{}#{}", branch.as_str(), file_str),
                    format!("{}#{}", cat_branch, cat_file),
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

pub(super) fn check_catalog_update(
    app: &AppEntry,
    catalog: &r3v3rs3_api::store::StoreCatalog,
) -> Option<r3v3rs3_api::platform::CatalogUpdateStatus> {
    let store_app = find_catalog_app(&app.spec, catalog)?;
    let (current_version, catalog_version) = diff_catalog_version(&app.spec.source, store_app)?;
    Some(r3v3rs3_api::platform::CatalogUpdateStatus {
        store_app_id: store_app.id.clone(),
        current_version,
        catalog_version,
    })
}

fn apply_catalog_upgrade(
    spec: &mut r3v3rs3_api::platform::AppSpec,
    store_app: &r3v3rs3_api::store::StoreApp,
) -> Result<(), Error> {
    spec.store_app_id = Some(store_app.id.clone());
    if let Some(ref image_str) = store_app.image {
        let image: r3v3rs3_api::container::ImageRef = image_str.parse()?;
        spec.source = r3v3rs3_api::platform::AppSource::Image { image };
    } else if let Some(ref repo_str) = store_app.repository {
        let repo: r3v3rs3_api::git::RepoUrl = repo_str.parse()?;
        let branch = store_app
            .branch
            .as_deref()
            .map(str::parse::<r3v3rs3_api::git::GitRef>)
            .transpose()?
            .unwrap_or_else(r3v3rs3_api::git::GitRef::main);
        let file = store_app
            .file
            .as_deref()
            .map(str::parse::<r3v3rs3_api::git::RelPath>)
            .transpose()?;
        let service: r3v3rs3_api::container::ServiceName =
            store_app.service.as_deref().unwrap_or("app").parse()?;
        let connection = match &spec.source {
            r3v3rs3_api::platform::AppSource::Compose { connection, .. } => *connection,
            r3v3rs3_api::platform::AppSource::Git { connection, .. } => *connection,
            _ => None,
        };
        spec.source = r3v3rs3_api::platform::AppSource::Compose {
            repository: Some(repo),
            branch,
            file,
            service,
            connection,
            compose_yaml: None,
        };
    }
    Ok(())
}

/// Rotates the platform encryption key and re-encrypts all stored secrets in SQLite tables.
#[utoipa::path(
    post,
    path = "/key/rotate",
    tag = "platform",
    operation_id = "rotate_platform_key",
    responses((status = 200, description = "Key rotation report.", body = KeyRotationReport), ErrorResponses)
)]
pub async fn rotate_platform_key(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<KeyRotationReport>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let report = platform
        .rotate_platform_key()
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::RotatePlatformKey)
        .summary(format!("new key: {}", report.new_key_id));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(report))
}

/// Lists the configured container registries for image pulls.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "list_registries",
    responses((status = 200, description = "The container registries.", body = Vec<RegistryEntry>), ErrorResponses)
)]
pub async fn list_registries(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<RegistryEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let registries = platform.registries().await.map_err(platform_error)?;
    Ok(Json(registries))
}

/// Adds container registry credentials for private image pulls.
#[utoipa::path(
    post,
    path = "/",
    tag = "platform",
    operation_id = "add_registry",
    request_body = RegistryRequest,
    responses((status = 200, description = "The created registry.", body = RegistryEntry), ErrorResponses)
)]
pub async fn add_registry(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<RegistryRequest>,
) -> Result<Json<RegistryEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform
        .add_registry(request)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::AddRegistry)
        .id(entry.id)
        .summary(entry.server.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(entry))
}

/// Updates container registry credentials.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "platform",
    operation_id = "update_registry",
    params(("id" = ShortId, Path, description = "The registry id")),
    request_body = RegistryRequest,
    responses((status = 200, description = "The updated registry.", body = RegistryEntry), ErrorResponses, NotFoundResponse)
)]
pub async fn update_registry(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<RegistryRequest>,
) -> Result<Json<RegistryEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform
        .update_registry(id, request)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateRegistry)
        .id(id)
        .summary(entry.server.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(entry))
}

/// Deletes container registry credentials.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "platform",
    operation_id = "delete_registry",
    params(("id" = ShortId, Path, description = "The registry id")),
    responses((status = 200, description = "The registry was deleted."), ErrorResponses, NotFoundResponse)
)]
pub async fn delete_registry(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<(), AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    platform.delete_registry(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteRegistry).id(id);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(())
}

/// The prune policy and disk hygiene settings.
#[utoipa::path(
    get,
    path = "/prune",
    tag = "platform",
    operation_id = "get_prune_policy",
    responses(
        (status = 200, description = "The prune policy.", body = PrunePolicy),
        ErrorResponses
    )
)]
pub async fn get_prune_policy(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<PrunePolicy>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let policy = platform.prune_policy().await.map_err(platform_error)?;
    Ok(Json(policy))
}

/// Updates the prune policy and disk hygiene settings.
#[utoipa::path(
    put,
    path = "/prune",
    tag = "platform",
    operation_id = "update_prune_policy",
    request_body = PrunePolicy,
    responses(
        (status = 200, description = "The prune policy is saved.", body = PrunePolicy),
        ErrorResponses
    )
)]
pub async fn update_prune_policy(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(policy): Json<PrunePolicy>,
) -> Result<Json<PrunePolicy>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    platform
        .update_prune_policy(policy.clone())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdatePlatformSettings).summary(format!(
        "Updated prune policy (enabled: {})",
        policy.enabled
    ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(policy))
}

/// Runs a disk hygiene prune immediately.
#[utoipa::path(
    post,
    path = "/prune/run",
    tag = "platform",
    operation_id = "run_prune",
    responses(
        (status = 200, description = "The prune results with reclaimed bytes.", body = PruneResult),
        ErrorResponses
    )
)]
pub async fn run_prune(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<PruneResult>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let result = platform.run_prune().await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdatePlatformSettings).summary(format!(
        "Ran disk prune, reclaimed {} bytes",
        result.space_reclaimed_bytes
    ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(result))
}

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct VacuumQuery {
    /// Optional threshold in MB. Only log files exceeding this size are truncated.
    pub threshold_mb: Option<u64>,
}

/// Truncates (vacuums) container log files across the platform.
#[utoipa::path(
    post,
    path = "/logs/vacuum",
    tag = "platform",
    operation_id = "vacuum_platform_logs",
    params(VacuumQuery),
    responses((status = 200, description = "The platform log vacuum result.", body = LogVacuumResult), ErrorResponses)
)]
pub async fn vacuum_platform_logs(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Query(query): Query<VacuumQuery>,
) -> Result<Json<LogVacuumResult>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let result = platform
        .vacuum_all_logs(query.threshold_mb)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::VacuumLogs).summary(format!(
        "Platform vacuumed container logs, freed {} bytes across {} files",
        result.freed_bytes, result.truncated_files
    ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(result))
}

/// Reads the host node threshold alert rules.
#[utoipa::path(
    get,
    path = "/node_alerts",
    tag = "platform",
    operation_id = "get_node_alerts",
    responses(
        (status = 200, description = "The node threshold alert rules.", body = NodeAlertRules),
        ErrorResponses
    )
)]
pub async fn get_node_alerts(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<NodeAlertRules>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let rules = platform.node_alert_rules().await.map_err(platform_error)?;
    Ok(Json(rules))
}

/// Updates the host node threshold alert rules.
#[utoipa::path(
    put,
    path = "/node_alerts",
    tag = "platform",
    operation_id = "update_node_alerts",
    request_body = NodeAlertRules,
    responses(
        (status = 200, description = "The node threshold alert rules are saved.", body = NodeAlertRules),
        ErrorResponses
    )
)]
pub async fn update_node_alerts(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(rules): Json<NodeAlertRules>,
) -> Result<Json<NodeAlertRules>, AppError> {
    rules.validate().map_err(AppError::from)?;
    let platform = state.platform(&caller, Permission::Admin).await?;
    platform
        .update_node_alert_rules(rules)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateNodeAlertRules).summary(format!(
        "Updated node alert rules (enabled: {})",
        rules.enabled
    ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(rules))
}

/// Lists all environment variable groups.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "list_env_groups",
    responses((status = 200, description = "The environment variable groups.", body = Vec<EnvGroupSummary>), ErrorResponses)
)]
pub async fn list_env_groups(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<EnvGroupSummary>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let groups = platform.list_env_groups().await.map_err(platform_error)?;
    Ok(Json(groups))
}

/// Creates a new environment variable group.
#[utoipa::path(
    post,
    path = "/",
    tag = "platform",
    operation_id = "create_env_group",
    request_body = EnvGroupSpec,
    responses((status = 200, description = "The created environment variable group.", body = EnvGroup), ErrorResponses)
)]
pub async fn create_env_group(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<EnvGroupSpec>,
) -> Result<Json<EnvGroup>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let group = platform
        .create_env_group(request, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::AddEnvGroup)
        .id(group.id)
        .summary(group.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(group))
}

/// Retrieves an environment variable group and its variables.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "platform",
    operation_id = "get_env_group",
    params(("id" = ShortId, Path, description = "The environment variable group id")),
    responses((status = 200, description = "The environment variable group.", body = EnvGroup), ErrorResponses, NotFoundResponse)
)]
pub async fn get_env_group(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<EnvGroup>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let group = platform.env_group(id).await.map_err(platform_error)?;
    Ok(Json(group))
}

/// Updates an environment variable group and replaces its variables.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "platform",
    operation_id = "update_env_group",
    params(("id" = ShortId, Path, description = "The environment variable group id")),
    request_body = EnvGroupSpec,
    responses((status = 200, description = "The updated environment variable group.", body = EnvGroup), ErrorResponses, NotFoundResponse)
)]
pub async fn update_env_group(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<EnvGroupSpec>,
) -> Result<Json<EnvGroup>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let group = platform
        .update_env_group(id, request, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateEnvGroup)
        .id(id)
        .summary(group.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(group))
}

/// Deletes an environment variable group.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "platform",
    operation_id = "delete_env_group",
    params(("id" = ShortId, Path, description = "The environment variable group id")),
    responses((status = 200, description = "The deleted environment variable group.", body = EnvGroup), ErrorResponses, NotFoundResponse)
)]
pub async fn delete_env_group(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<EnvGroup>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let group = platform
        .delete_env_group(id)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteEnvGroup)
        .id(id)
        .summary(group.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(group))
}

/// Lists all projects.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "list_projects",
    responses((status = 200, description = "The projects.", body = Vec<ProjectSummary>), ErrorResponses)
)]
pub async fn list_projects(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<ProjectSummary>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let projects = platform.list_projects().await.map_err(platform_error)?;
    Ok(Json(projects))
}

/// Creates a new project.
#[utoipa::path(
    post,
    path = "/",
    tag = "platform",
    operation_id = "create_project",
    request_body = ProjectSpec,
    responses((status = 200, description = "The created project.", body = Project), ErrorResponses)
)]
pub async fn create_project(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<ProjectSpec>,
) -> Result<Json<Project>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let project = platform
        .create_project(request, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::AddProject)
        .id(project.id)
        .summary(project.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(project))
}

/// Retrieves a project and its environments.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "platform",
    operation_id = "get_project",
    params(("id" = ShortId, Path, description = "The project id")),
    responses((status = 200, description = "The project.", body = Project), ErrorResponses, NotFoundResponse)
)]
pub async fn get_project(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Project>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let project = platform.project(id).await.map_err(platform_error)?;
    Ok(Json(project))
}

/// Updates a project.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "platform",
    operation_id = "update_project",
    params(("id" = ShortId, Path, description = "The project id")),
    request_body = ProjectSpec,
    responses((status = 200, description = "The updated project.", body = Project), ErrorResponses, NotFoundResponse)
)]
pub async fn update_project(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<ProjectSpec>,
) -> Result<Json<Project>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let project = platform
        .update_project(id, request, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateProject)
        .id(id)
        .summary(project.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(project))
}

/// Deletes a project.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "platform",
    operation_id = "delete_project",
    params(("id" = ShortId, Path, description = "The project id")),
    responses((status = 200, description = "The deleted project.", body = Project), ErrorResponses, NotFoundResponse)
)]
pub async fn delete_project(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Project>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let project = platform.delete_project(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteProject)
        .id(id)
        .summary(project.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(project))
}

/// Creates a new environment in a project.
#[utoipa::path(
    post,
    path = "/{id}/environments",
    tag = "platform",
    operation_id = "create_environment",
    params(("id" = ShortId, Path, description = "The project id")),
    request_body = EnvironmentSpec,
    responses((status = 200, description = "The created environment.", body = ProjectEnvironment), ErrorResponses, NotFoundResponse)
)]
pub async fn create_environment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<EnvironmentSpec>,
) -> Result<Json<ProjectEnvironment>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let env = platform
        .create_environment(id, request, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::AddEnvironment)
        .id(env.id)
        .summary(env.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(env))
}

/// Retrieves an environment and its variables.
#[utoipa::path(
    get,
    path = "/{id}/environments/{env_id}",
    tag = "platform",
    operation_id = "get_environment",
    params(
        ("id" = ShortId, Path, description = "The project id"),
        ("env_id" = ShortId, Path, description = "The environment id")
    ),
    responses((status = 200, description = "The environment.", body = ProjectEnvironment), ErrorResponses, NotFoundResponse)
)]
pub async fn get_environment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((_id, env_id)): Path<(ShortId, ShortId)>,
) -> Result<Json<ProjectEnvironment>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let env = platform.environment(env_id).await.map_err(platform_error)?;
    Ok(Json(env))
}

/// Updates an environment.
#[utoipa::path(
    put,
    path = "/{id}/environments/{env_id}",
    tag = "platform",
    operation_id = "update_environment",
    params(
        ("id" = ShortId, Path, description = "The project id"),
        ("env_id" = ShortId, Path, description = "The environment id")
    ),
    request_body = EnvironmentSpec,
    responses((status = 200, description = "The updated environment.", body = ProjectEnvironment), ErrorResponses, NotFoundResponse)
)]
pub async fn update_environment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((_id, env_id)): Path<(ShortId, ShortId)>,
    Json(request): Json<EnvironmentSpec>,
) -> Result<Json<ProjectEnvironment>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let env = platform
        .update_environment(env_id, request, unix_ms())
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateEnvironment)
        .id(env_id)
        .summary(env.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(env))
}

/// Deletes an environment.
#[utoipa::path(
    delete,
    path = "/{id}/environments/{env_id}",
    tag = "platform",
    operation_id = "delete_environment",
    params(
        ("id" = ShortId, Path, description = "The project id"),
        ("env_id" = ShortId, Path, description = "The environment id")
    ),
    responses((status = 200, description = "The deleted environment.", body = ProjectEnvironment), ErrorResponses, NotFoundResponse)
)]
pub async fn delete_environment(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((_id, env_id)): Path<(ShortId, ShortId)>,
) -> Result<Json<ProjectEnvironment>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let env = platform
        .delete_environment(env_id)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::DeleteEnvironment)
        .id(env_id)
        .summary(env.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(env))
}

/// Retrieves the environment variables of an environment.
#[utoipa::path(
    get,
    path = "/{id}/environments/{env_id}/env",
    tag = "platform",
    operation_id = "get_environment_env",
    params(
        ("id" = ShortId, Path, description = "The project id"),
        ("env_id" = ShortId, Path, description = "The environment id")
    ),
    responses((status = 200, description = "The environment variables.", body = Vec<EnvEntry>), ErrorResponses, NotFoundResponse)
)]
pub async fn get_environment_env(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((_id, env_id)): Path<(ShortId, ShortId)>,
) -> Result<Json<Vec<EnvEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let env = platform.environment(env_id).await.map_err(platform_error)?;
    Ok(Json(env.variables))
}

/// Updates the environment variables of an environment.
#[utoipa::path(
    put,
    path = "/{id}/environments/{env_id}/env",
    tag = "platform",
    operation_id = "put_environment_env",
    params(
        ("id" = ShortId, Path, description = "The project id"),
        ("env_id" = ShortId, Path, description = "The environment id")
    ),
    request_body = EnvironmentEnvSpec,
    responses((status = 200, description = "The updated environment variables.", body = Vec<EnvEntry>), ErrorResponses, NotFoundResponse)
)]
pub async fn put_environment_env(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((_id, env_id)): Path<(ShortId, ShortId)>,
    Json(request): Json<EnvironmentEnvSpec>,
) -> Result<Json<Vec<EnvEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let env = platform
        .set_environment_env(env_id, request)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateEnvironmentEnv)
        .id(env_id)
        .summary(env.name.clone());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(env.variables))
}

/// Compares environment variables and secrets between source and target environments.
#[utoipa::path(
    get,
    path = "/{id}/environments/{source_id}/diff/{target_id}",
    tag = "platform",
    operation_id = "diff_environments",
    params(
        ("id" = ShortId, Path, description = "The project id"),
        ("source_id" = ShortId, Path, description = "The source environment id"),
        ("target_id" = ShortId, Path, description = "The target environment id")
    ),
    responses((status = 200, description = "The environment comparison report.", body = EnvironmentDiffReport), ErrorResponses, NotFoundResponse)
)]
pub async fn diff_environments(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, source_id, target_id)): Path<(ShortId, ShortId, ShortId)>,
) -> Result<Json<EnvironmentDiffReport>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let report = platform
        .diff_environments(id, source_id, target_id)
        .await
        .map_err(platform_error)?;
    Ok(Json(report))
}

/// Synchronizes environment variables and secrets from source to target environment.
#[utoipa::path(
    post,
    path = "/{id}/environments/sync",
    tag = "platform",
    operation_id = "sync_environment_env",
    params(
        ("id" = ShortId, Path, description = "The project id")
    ),
    request_body = EnvironmentSyncRequest,
    responses((status = 200, description = "The synchronization result.", body = EnvironmentSyncReport), ErrorResponses, NotFoundResponse)
)]
pub async fn sync_environment_env(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<EnvironmentSyncRequest>,
) -> Result<Json<EnvironmentSyncReport>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let report = platform
        .sync_environment_env(id, request)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::SyncEnvironmentEnv)
        .id(report.target_env_id)
        .summary(format!(
            "{} -> {} ({} synced)",
            report.source_env_id, report.target_env_id, report.synced_count
        ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(report))
}
