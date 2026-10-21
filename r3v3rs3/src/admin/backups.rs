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
    platform::{
        BackupConfig, BackupEntry, BackupVerifyReport, PitrRange, PitrRestoreRequest,
        PitrRestoreResponse, WalSegment,
    },
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

/// Tests decompression and format integrity of a database backup.
#[utoipa::path(
    post,
    path = "/{id}/backups/{backup_id}/test",
    tag = "platform",
    operation_id = "test_database_backup",
    params(("id" = ShortId, Path, description = "Database app id."), ("backup_id" = String, Path, description = "Backup id.")),
    responses((status = 200, description = "The verification report.", body = BackupVerifyReport), NotFoundResponse, ErrorResponses)
)]
pub async fn test_backup(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, backup_id)): Path<(ShortId, String)>,
) -> Result<Json<BackupVerifyReport>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let report = platform
        .verify_backup(id, &backup_id)
        .await
        .map_err(platform_error)?;
    let app = platform.app(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::BackupDatabase)
        .id(id)
        .summary(format!("{}: test backup {backup_id}", app.name));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(report))
}

/// The backup settings. The answer carries no secret keys or passwords.
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
    if let Some(sftp) = config.sftp.as_mut() {
        sftp.password = String::new();
        sftp.key = String::new();
    }
    if let Some(rsync) = config.rsync.as_mut() {
        rsync.password = String::new();
    }
    Ok(Json(config))
}

/// Replaces the backup settings. An empty secret key or password keeps the stored one.
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

/// Returns the point-in-time recovery range and statistics for a database.
#[utoipa::path(
    get,
    path = "/{id}/pitr",
    tag = "platform",
    operation_id = "get_database_pitr_range",
    params(("id" = ShortId, Path, description = "Database app id.")),
    responses((status = 200, description = "The PITR recovery window and stats.", body = PitrRange), ErrorResponses)
)]
pub async fn get_pitr_range(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<PitrRange>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let range = platform.pitr_range(id).await.map_err(platform_error)?;
    Ok(Json(range))
}

/// Restores a database to a specific point in time (PITR).
#[utoipa::path(
    post,
    path = "/{id}/pitr/restore",
    tag = "platform",
    operation_id = "restore_database_pitr",
    params(("id" = ShortId, Path, description = "Database app id.")),
    request_body = PitrRestoreRequest,
    responses((status = 200, description = "The PITR restore result.", body = PitrRestoreResponse), NotFoundResponse, ErrorResponses)
)]
pub async fn restore_pitr(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<PitrRestoreRequest>,
) -> Result<Json<PitrRestoreResponse>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let resp = platform
        .restore_pitr(id, request)
        .await
        .map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::PitrRestoreDatabase)
        .id(id)
        .summary(format!(
            "{}: PITR target {} (branched: {})",
            resp.app.name, resp.target_time, resp.branched
        ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(resp))
}

/// Archives the current WAL or binary log segment from the running database container.
#[utoipa::path(
    post,
    path = "/{id}/wal/archive",
    tag = "platform",
    operation_id = "archive_database_wal",
    params(("id" = ShortId, Path, description = "Database app id.")),
    responses((status = 200, description = "The archived WAL segment.", body = WalSegment), NotFoundResponse, ErrorResponses)
)]
pub async fn archive_wal(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<WalSegment>, AppError> {
    let platform = state.platform(&caller, Permission::Edit).await?;
    let segment = platform.archive_wal(id).await.map_err(platform_error)?;
    let app = platform.app(id).await.map_err(platform_error)?;
    let record = AuditRecord::new(AuditAction::ArchiveDatabaseWal)
        .id(id)
        .summary(format!("{}: archived WAL {}", app.name, segment.filename));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(segment))
}

/// Lists archived WAL and binary log segments for a database.
#[utoipa::path(
    get,
    path = "/{id}/wal/segments",
    tag = "platform",
    operation_id = "list_database_wal_segments",
    params(("id" = ShortId, Path, description = "Database app id.")),
    responses((status = 200, description = "The archived WAL segments.", body = [WalSegment]), ErrorResponses)
)]
pub async fn list_wal_segments(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<Vec<WalSegment>>, AppError> {
    let platform = state.platform(&caller, Permission::Read).await?;
    let segments = platform
        .list_wal_segments(id)
        .await
        .map_err(platform_error)?;
    Ok(Json(segments))
}
