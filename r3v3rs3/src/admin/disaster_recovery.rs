use super::openapi::ErrorResponses;
use super::platform::platform_error;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::platform::disaster_recovery::{
    create_dr_backup, delete_dr_backup, download_dr_backup, list_dr_backups, restore_dr_backup,
    restore_dr_bytes,
};
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::platform::{
    CreateDisasterRecoveryRequest, DisasterRecoveryBackupEntry, RestoreDisasterRecoveryRequest,
};

/// Lists all disaster recovery backups.
#[utoipa::path(
    get,
    path = "/backups",
    tag = "platform",
    operation_id = "list_disaster_recovery_backups",
    responses((status = 200, description = "The disaster recovery backups.", body = Vec<DisasterRecoveryBackupEntry>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<DisasterRecoveryBackupEntry>>, AppError> {
    caller.authorize_platform(Permission::Admin)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let backups = list_dr_backups(&config_dir).await.map_err(platform_error)?;
    Ok(Json(backups))
}

/// Creates a new disaster recovery backup.
#[utoipa::path(
    post,
    path = "/backups",
    tag = "platform",
    operation_id = "create_disaster_recovery_backup",
    request_body = CreateDisasterRecoveryRequest,
    responses((status = 200, description = "The new disaster recovery backup.", body = DisasterRecoveryBackupEntry), ErrorResponses)
)]
pub async fn create(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<CreateDisasterRecoveryRequest>,
) -> Result<Json<DisasterRecoveryBackupEntry>, AppError> {
    caller.authorize_platform(Permission::Admin)?;
    let platform = state.platform(&caller, Permission::Admin).await.ok();
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let entry = create_dr_backup(
        &config_dir,
        request.passphrase.as_deref(),
        request.upload_s3,
        platform.as_deref(),
    )
    .await
    .map_err(platform_error)?;

    let record = AuditRecord::new(AuditAction::CreateDisasterRecoveryBackup)
        .id(&entry.id)
        .summary(format!("{} files, {} bytes", entry.files_count, entry.size));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;

    Ok(Json(entry))
}

/// Downloads one disaster recovery backup bundle.
#[utoipa::path(
    get,
    path = "/backups/{id}/download",
    tag = "platform",
    operation_id = "download_disaster_recovery_backup",
    params(("id" = String, Path, description = "Disaster recovery backup id.")),
    responses((status = 200, description = "The encrypted bundle file.", content_type = "application/octet-stream"), ErrorResponses)
)]
pub async fn download(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    caller.authorize_platform(Permission::Admin)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let bytes = download_dr_backup(&config_dir, &id)
        .await
        .map_err(platform_error)?;

    let filename = format!("{id}.r3bundle");
    let response = (
        [
            ("content-type", "application/octet-stream"),
            (
                "content-disposition",
                &format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    )
        .into_response();
    Ok(response)
}

/// Deletes one disaster recovery backup.
#[utoipa::path(
    delete,
    path = "/backups/{id}",
    tag = "platform",
    operation_id = "delete_disaster_recovery_backup",
    params(("id" = String, Path, description = "Disaster recovery backup id.")),
    responses((status = 200, description = "The backup is deleted."), ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<()>, AppError> {
    caller.authorize_platform(Permission::Admin)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    delete_dr_backup(&config_dir, &id)
        .await
        .map_err(platform_error)?;
    Ok(Json(()))
}

/// Restores platform state from a local disaster recovery backup ID.
#[utoipa::path(
    post,
    path = "/backups/{id}/restore",
    tag = "platform",
    operation_id = "restore_disaster_recovery_backup",
    params(("id" = String, Path, description = "Disaster recovery backup id.")),
    request_body = RestoreDisasterRecoveryRequest,
    responses((status = 200, description = "The disaster recovery backup was restored."), ErrorResponses)
)]
pub async fn restore(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    Json(request): Json<RestoreDisasterRecoveryRequest>,
) -> Result<Json<usize>, AppError> {
    caller.authorize_platform(Permission::Admin)?;
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let restored = restore_dr_backup(&config_dir, &id, request.passphrase.as_deref())
        .await
        .map_err(platform_error)?;

    let record = AuditRecord::new(AuditAction::RestoreDisasterRecoveryBackup)
        .id(&id)
        .summary(format!("{restored} files restored"));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;

    Ok(Json(restored))
}

/// Restores platform state from an uploaded `.r3bundle` payload.
#[utoipa::path(
    post,
    path = "/restore-upload",
    tag = "platform",
    operation_id = "restore_disaster_recovery_upload",
    responses((status = 200, description = "The uploaded disaster recovery backup was restored."), ErrorResponses)
)]
pub async fn restore_upload(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<usize>, AppError> {
    caller.authorize_platform(Permission::Admin)?;
    let passphrase = headers.get("x-passphrase").and_then(|v| v.to_str().ok());
    let config_dir = state.data.lock().await.app_info.config_path.clone();
    let restored = restore_dr_bytes(&config_dir, &body, passphrase)
        .await
        .map_err(platform_error)?;

    let record = AuditRecord::new(AuditAction::RestoreDisasterRecoveryBackup)
        .summary(format!("{restored} files restored from upload"));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;

    Ok(Json(restored))
}
