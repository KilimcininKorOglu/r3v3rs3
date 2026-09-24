//! The Git provider connections and the settings of the deployment platform. An account with the
//! Edit permission lists the connections for the app form, and only an admin changes them.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::platform::platform_error;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::clock::unix_ms;
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{
    audit::AuditAction,
    git_connection::{
        AuthorizeRequest, AuthorizeResponse, GitConnectionEntry, GitConnectionRequest,
        PlatformSettings,
    },
    id::ShortId,
};

/// Starts the authorization of a Git provider connection. The browser of the admin opens the
/// returned page of the provider, and the provider sends it back to `redirect_uri`.
#[utoipa::path(
    post,
    path = "/{id}/authorize",
    tag = "platform",
    operation_id = "authorize_git_connection",
    params(("id" = ShortId, Path, description = "Connection id.")),
    request_body = AuthorizeRequest,
    responses((status = 200, description = "The authorization page of the provider.", body = AuthorizeResponse), NotFoundResponse, ErrorResponses)
)]
pub async fn authorize_connection(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<AuthorizeRequest>,
) -> Result<Json<AuthorizeResponse>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let response = platform
        .authorize_git_connection(id, &request.redirect_uri, &caller.username, caller.client)
        .await
        .map_err(platform_error)?;
    Ok(Json(response))
}

/// Lists the Git provider connections.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "list_git_connections",
    responses((status = 200, description = "The connections.", body = Vec<GitConnectionEntry>), ErrorResponses)
)]
pub async fn list_connections(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<GitConnectionEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    Ok(Json(
        platform.git_connections().await.map_err(platform_error)?,
    ))
}

/// Adds a Git provider connection. An admin authorizes it afterwards.
#[utoipa::path(
    post,
    path = "/",
    tag = "platform",
    operation_id = "add_git_connection",
    request_body = GitConnectionRequest,
    responses((status = 200, description = "The new connection.", body = GitConnectionEntry), ErrorResponses)
)]
pub async fn add_connection(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<GitConnectionRequest>,
) -> Result<Json<GitConnectionEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform
        .add_git_connection(request, unix_ms())
        .await
        .map_err(platform_error)?;
    record(&state, &caller, AuditAction::AddGitConnection, &entry).await;
    Ok(Json(entry))
}

/// Returns a Git provider connection.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "platform",
    operation_id = "get_git_connection",
    params(("id" = ShortId, Path, description = "Connection id.")),
    responses((status = 200, description = "The connection.", body = GitConnectionEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn get_connection(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<GitConnectionEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    Ok(Json(
        platform.git_connection(id).await.map_err(platform_error)?,
    ))
}

/// Replaces the settings of a Git provider connection. A new provider, address or client id
/// disconnects it. Without a client secret the current secret stays.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "platform",
    operation_id = "update_git_connection",
    params(("id" = ShortId, Path, description = "Connection id.")),
    request_body = GitConnectionRequest,
    responses((status = 200, description = "The updated connection.", body = GitConnectionEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn update_connection(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<GitConnectionRequest>,
) -> Result<Json<GitConnectionEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform
        .update_git_connection(id, request, unix_ms())
        .await
        .map_err(platform_error)?;
    record(&state, &caller, AuditAction::UpdateGitConnection, &entry).await;
    Ok(Json(entry))
}

/// Deletes a Git provider connection with its tokens.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "platform",
    operation_id = "delete_git_connection",
    params(("id" = ShortId, Path, description = "Connection id.")),
    responses((status = 200, description = "The connection is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete_connection(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform
        .delete_git_connection(id)
        .await
        .map_err(platform_error)?;
    record(&state, &caller, AuditAction::DeleteGitConnection, &entry).await;
    Ok(Json(()))
}

/// The summary names the connection and its provider, never the client secret.
async fn record(
    state: &AppState,
    caller: &Caller,
    action: AuditAction,
    entry: &GitConnectionEntry,
) {
    let record = AuditRecord::new(action).id(entry.id).summary(format!(
        "{} ({})",
        entry.name,
        entry.provider.as_str()
    ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
}

/// Returns the settings of the deployment platform.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "get_platform_settings",
    responses((status = 200, description = "The settings.", body = PlatformSettings), ErrorResponses)
)]
pub async fn get_settings(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<PlatformSettings>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    Ok(Json(
        platform.platform_settings().await.map_err(platform_error)?,
    ))
}

/// Replaces the settings of the deployment platform.
#[utoipa::path(
    put,
    path = "/",
    tag = "platform",
    operation_id = "update_platform_settings",
    request_body = PlatformSettings,
    responses((status = 200, description = "The settings.", body = PlatformSettings), ErrorResponses)
)]
pub async fn put_settings(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(settings): Json<PlatformSettings>,
) -> Result<Json<PlatformSettings>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    platform
        .set_platform_settings(&settings)
        .await
        .map_err(platform_error)?;
    let summary = settings
        .public_url
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    let record = AuditRecord::new(AuditAction::UpdatePlatformSettings).summary(summary);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(settings))
}
