//! The backups of the managed databases: list, take, download, delete and restore, plus the
//! backup settings. Every changing handler is `Edit`, the downloads are `Read`.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::platform::platform_error;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use axum::{
    Extension, Json,
    extract::{Path, State},
    http::header::CONTENT_DISPOSITION,
    response::IntoResponse,
};
use r3v3rs3_api::{
    audit::AuditAction,
    id::ShortId,
    platform::{BackupConfig, BackupEntry},
};

/// Lists the backups of a database, newest first.
#[utoipa::path(
    get,
    path = "/{id}/backups",
    tag = "platform",
    operation_id = "list_database_backups",
    params(("id" = ShortId, Path, description = "Database app id.")),
    responses((status = 200, description = "The backups.", body = Vec<BackupEntry>), ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Vec<BackupEntry>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    Ok(Json(
        platform.list_backups(id).await.map_err(platform_error)?,
    ))
}

/// Takes the backup of a database now.
#[utoipa::path(
    post,
    path = "/{id}/backups",
    tag = "platform",
    operation_id = "create_database_backup",
    params(("id" = ShortId, Path, description = "Database app id.")),
    responses((status = 200, description = "The new backup.", body = BackupEntry), ErrorResponses)
)]
pub async fn create(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<BackupEntry>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let entry = platform.run_backup(id).await.map_err(platform_error)?;
    let app = platform.app(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::BackupDatabase)
        .id(id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(entry))
}

/// Downloads one backup.
#[utoipa::path(
    get,
    path = "/{id}/backups/{backup_id}/download",
    tag = "platform",
    operation_id = "download_database_backup",
    params(("id" = ShortId, Path, description = "Database app id."), ("backup_id" = String, Path, description = "Backup id.")),
    responses((status = 200, description = "The compressed backup.", content_type = "application/octet-stream", body = Vec<u8>), NotFoundResponse, ErrorResponses)
)]
pub async fn download(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, backup_id)): Path<(ShortId, String)>,
) -> Result<impl IntoResponse, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let content = platform
        .download_backup(id, &backup_id)
        .await
        .map_err(platform_error)?;
    let headers = [
        ("content-type", "application/octet-stream".to_string()),
        (
            CONTENT_DISPOSITION.as_str(),
            format!("attachment; filename=\"{backup_id}.dump.gz\""),
        ),
    ];
    Ok((headers, content))
}

/// Deletes one backup.
#[utoipa::path(
    delete,
    path = "/{id}/backups/{backup_id}",
    tag = "platform",
    operation_id = "delete_database_backup",
    params(("id" = ShortId, Path, description = "Database app id."), ("backup_id" = String, Path, description = "Backup id.")),
    responses((status = 200, description = "The backup is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, backup_id)): Path<(ShortId, String)>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    platform
        .delete_backup(id, &backup_id)
        .await
        .map_err(platform_error)?;
    Ok(Json(()))
}

/// Restores one backup into the running container of the database.
#[utoipa::path(
    post,
    path = "/{id}/backups/{backup_id}/restore",
    tag = "platform",
    operation_id = "restore_database_backup",
    params(("id" = ShortId, Path, description = "Database app id."), ("backup_id" = String, Path, description = "Backup id.")),
    responses((status = 200, description = "The backup is restored."), NotFoundResponse, ErrorResponses)
)]
pub async fn restore(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, backup_id)): Path<(ShortId, String)>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    platform
        .restore_backup(id, &backup_id)
        .await
        .map_err(platform_error)?;
    let app = platform.app(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::RestoreDatabase)
        .id(id)
        .summary(app.name.to_string());
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(()))
}

/// The backup settings. The answer carries no S3 secret key.
#[utoipa::path(
    get,
    path = "/",
    tag = "platform",
    operation_id = "get_backup_config",
    responses((status = 200, description = "The backup settings.", body = BackupConfig), ErrorResponses)
)]
pub async fn get_config(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<BackupConfig>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let mut config = platform.backup_config().await.map_err(platform_error)?;
    if let Some(s3) = config.s3.as_mut() {
        s3.secret_key = String::new();
    }
    Ok(Json(config))
}

/// Replaces the backup settings. An empty S3 secret key keeps the stored one.
#[utoipa::path(
    put,
    path = "/",
    tag = "platform",
    operation_id = "put_backup_config",
    request_body = BackupConfig,
    responses((status = 200, description = "The backup settings are saved."), ErrorResponses)
)]
pub async fn put_config(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(config): Json<BackupConfig>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    platform
        .set_backup_config(config)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::UpdateBackupConfig);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(()))
}
