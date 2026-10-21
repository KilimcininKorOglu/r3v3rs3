//! The files of an app: the volumes of its running container, over the archive API. Every
//! handler is admin-only, because the files hold the whole data of the app.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::platform::platform_error;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use axum::{
    Extension, Json,
    body::Bytes,
    extract::{Path, Query, State},
    http::header::CONTENT_DISPOSITION,
    response::IntoResponse,
};
use r3v3rs3_api::{
    audit::AuditAction,
    container::VolumeMount,
    error::Error,
    id::ShortId,
    platform::{AppEntry, FileEntry, FilePathQuery, VolumeBackupEntry},
};
use std::sync::Arc;

/// Lists the entries of a directory inside a volume of the app.
#[utoipa::path(
    get,
    path = "/{id}/files",
    tag = "platform",
    operation_id = "list_app_files",
    params(("id" = ShortId, Path, description = "App id."), FilePathQuery),
    responses((status = 200, description = "The entries of the directory.", body = [FileEntry]), NotFoundResponse, ErrorResponses)
)]
pub async fn list(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<FilePathQuery>,
) -> Result<Json<Vec<FileEntry>>, AppError> {
    let context = context(&state, &caller, id).await?;
    check_path(&context.app, &query.path, false)?;
    Ok(Json(
        context
            .platform
            .list_files(id, &query.path)
            .await
            .map_err(platform_error)?,
    ))
}

/// Downloads a file inside a volume of the app.
#[utoipa::path(
    get,
    path = "/{id}/files/download",
    tag = "platform",
    operation_id = "download_app_file",
    params(("id" = ShortId, Path, description = "App id."), FilePathQuery),
    responses((status = 200, description = "The content of the file.", content_type = "application/octet-stream", body = Vec<u8>), NotFoundResponse, ErrorResponses)
)]
pub async fn download(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<FilePathQuery>,
) -> Result<impl IntoResponse, AppError> {
    let context = context(&state, &caller, id).await?;
    check_path(&context.app, &query.path, false)?;
    let content = context
        .platform
        .download_file(id, &query.path)
        .await
        .map_err(platform_error)?;
    let name = query.path.rsplit('/').next().unwrap_or("file");
    let headers = [
        ("content-type", "application/octet-stream".to_string()),
        (
            CONTENT_DISPOSITION.as_str(),
            format!("attachment; filename=\"{name}\""),
        ),
    ];
    Ok((headers, content))
}

/// Writes `body` into a file inside a writable volume of the app.
#[utoipa::path(
    put,
    path = "/{id}/files",
    tag = "platform",
    operation_id = "upload_app_file",
    params(("id" = ShortId, Path, description = "App id."), FilePathQuery),
    request_body(content = Vec<u8>, content_type = "application/octet-stream", description = "The content of the file."),
    responses((status = 200, description = "The file is written."), NotFoundResponse, ErrorResponses)
)]
pub async fn upload(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<FilePathQuery>,
    body: Bytes,
) -> Result<Json<()>, AppError> {
    let context = context(&state, &caller, id).await?;
    check_path(&context.app, &query.path, true)?;
    context
        .platform
        .upload_file(id, &query.path, body.to_vec())
        .await
        .map_err(platform_error)?;
    audit(
        &state,
        &caller,
        id,
        format!("upload {} ({} bytes)", query.path, body.len()),
    )
    .await;
    Ok(Json(()))
}

/// Deletes a file or a directory inside a writable volume of the app.
#[utoipa::path(
    delete,
    path = "/{id}/files",
    tag = "platform",
    operation_id = "delete_app_file",
    params(("id" = ShortId, Path, description = "App id."), FilePathQuery),
    responses((status = 200, description = "The path is removed."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Query(query): Query<FilePathQuery>,
) -> Result<Json<()>, AppError> {
    let context = context(&state, &caller, id).await?;
    check_path(&context.app, &query.path, true)?;
    context
        .platform
        .delete_path(id, &query.path)
        .await
        .map_err(platform_error)?;
    audit(&state, &caller, id, format!("delete {}", query.path)).await;
    Ok(Json(()))
}

/// Lists stored snapshots and backups of an application volume.
#[utoipa::path(
    get,
    path = "/{id}/volumes/{volume}/backups",
    tag = "platform",
    operation_id = "list_volume_backups",
    params(
        ("id" = ShortId, Path, description = "App id."),
        ("volume" = String, Path, description = "Volume name.")
    ),
    responses((status = 200, description = "The volume backups.", body = [VolumeBackupEntry]), NotFoundResponse, ErrorResponses)
)]
pub async fn list_volume_backups(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, volume)): Path<(ShortId, String)>,
) -> Result<Json<Vec<VolumeBackupEntry>>, AppError> {
    let context = context(&state, &caller, id).await?;
    let entries = context
        .platform
        .list_volume_backups(id, &volume)
        .await
        .map_err(platform_error)?;
    Ok(Json(entries))
}

/// Creates a snapshot backup of an application volume.
#[utoipa::path(
    post,
    path = "/{id}/volumes/{volume}/backups",
    tag = "platform",
    operation_id = "create_volume_backup",
    params(
        ("id" = ShortId, Path, description = "App id."),
        ("volume" = String, Path, description = "Volume name.")
    ),
    responses((status = 200, description = "The created volume backup.", body = VolumeBackupEntry), NotFoundResponse, ErrorResponses)
)]
pub async fn create_volume_backup(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, volume)): Path<(ShortId, String)>,
) -> Result<Json<VolumeBackupEntry>, AppError> {
    let context = context(&state, &caller, id).await?;
    let entry = context
        .platform
        .create_volume_backup(id, &volume)
        .await
        .map_err(platform_error)?;
    audit(
        &state,
        &caller,
        id,
        format!("backup volume {volume} ({})", entry.id),
    )
    .await;
    Ok(Json(entry))
}

/// Downloads a compressed volume snapshot archive.
#[utoipa::path(
    get,
    path = "/{id}/volumes/{volume}/backups/{backup_id}/download",
    tag = "platform",
    operation_id = "download_volume_backup",
    params(
        ("id" = ShortId, Path, description = "App id."),
        ("volume" = String, Path, description = "Volume name."),
        ("backup_id" = String, Path, description = "Backup id.")
    ),
    responses((status = 200, description = "The volume backup archive.", content_type = "application/gzip", body = Vec<u8>), NotFoundResponse, ErrorResponses)
)]
pub async fn download_volume_backup(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, volume, backup_id)): Path<(ShortId, String, String)>,
) -> Result<impl IntoResponse, AppError> {
    let context = context(&state, &caller, id).await?;
    let content = context
        .platform
        .download_volume_backup(id, &volume, &backup_id)
        .await
        .map_err(platform_error)?;
    let filename = format!("{volume}-{backup_id}.tar.gz");
    let headers = [
        ("content-type", "application/gzip".to_string()),
        (
            CONTENT_DISPOSITION.as_str(),
            format!("attachment; filename=\"{filename}\""),
        ),
    ];
    Ok((headers, content))
}

/// Restores a volume snapshot into the container.
#[utoipa::path(
    post,
    path = "/{id}/volumes/{volume}/backups/{backup_id}/restore",
    tag = "platform",
    operation_id = "restore_volume_backup",
    params(
        ("id" = ShortId, Path, description = "App id."),
        ("volume" = String, Path, description = "Volume name."),
        ("backup_id" = String, Path, description = "Backup id.")
    ),
    responses((status = 200, description = "The volume is restored."), NotFoundResponse, ErrorResponses)
)]
pub async fn restore_volume_backup(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, volume, backup_id)): Path<(ShortId, String, String)>,
) -> Result<Json<()>, AppError> {
    let context = context(&state, &caller, id).await?;
    context
        .platform
        .restore_volume_backup(id, &volume, &backup_id)
        .await
        .map_err(platform_error)?;
    audit(
        &state,
        &caller,
        id,
        format!("restore volume {volume} from {backup_id}"),
    )
    .await;
    Ok(Json(()))
}

/// Deletes a stored volume snapshot.
#[utoipa::path(
    delete,
    path = "/{id}/volumes/{volume}/backups/{backup_id}",
    tag = "platform",
    operation_id = "delete_volume_backup",
    params(
        ("id" = ShortId, Path, description = "App id."),
        ("volume" = String, Path, description = "Volume name."),
        ("backup_id" = String, Path, description = "Backup id.")
    ),
    responses((status = 200, description = "The backup is deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete_volume_backup(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, volume, backup_id)): Path<(ShortId, String, String)>,
) -> Result<Json<()>, AppError> {
    let context = context(&state, &caller, id).await?;
    context
        .platform
        .delete_volume_backup(id, &volume, &backup_id)
        .await
        .map_err(platform_error)?;
    audit(
        &state,
        &caller,
        id,
        format!("delete volume backup {volume}/{backup_id}"),
    )
    .await;
    Ok(Json(()))
}

/// Exports the entire volume content as a `.tar.gz` archive.
#[utoipa::path(
    get,
    path = "/{id}/volumes/{volume}/archive",
    tag = "platform",
    operation_id = "export_volume_archive",
    params(
        ("id" = ShortId, Path, description = "App id."),
        ("volume" = String, Path, description = "Volume name.")
    ),
    responses((status = 200, description = "The volume tar.gz archive.", content_type = "application/gzip", body = Vec<u8>), NotFoundResponse, ErrorResponses)
)]
pub async fn export_volume_archive(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, volume)): Path<(ShortId, String)>,
) -> Result<impl IntoResponse, AppError> {
    let context = context(&state, &caller, id).await?;
    let content = context
        .platform
        .export_volume_archive(id, &volume)
        .await
        .map_err(platform_error)?;
    let filename = format!("{volume}-export.tar.gz");
    let headers = [
        ("content-type", "application/gzip".to_string()),
        (
            CONTENT_DISPOSITION.as_str(),
            format!("attachment; filename=\"{filename}\""),
        ),
    ];
    Ok((headers, content))
}

/// Imports and extracts a `.tar.gz` or `.tar` archive into the volume.
#[utoipa::path(
    post,
    path = "/{id}/volumes/{volume}/archive",
    tag = "platform",
    operation_id = "import_volume_archive",
    params(
        ("id" = ShortId, Path, description = "App id."),
        ("volume" = String, Path, description = "Volume name.")
    ),
    request_body(content = Vec<u8>, content_type = "application/gzip", description = "The volume tar or tar.gz archive."),
    responses((status = 200, description = "The archive is extracted into the volume."), NotFoundResponse, ErrorResponses)
)]
pub async fn import_volume_archive(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, volume)): Path<(ShortId, String)>,
    body: Bytes,
) -> Result<Json<()>, AppError> {
    let context = context(&state, &caller, id).await?;
    context
        .platform
        .import_volume_archive(id, &volume, body.to_vec())
        .await
        .map_err(platform_error)?;
    audit(
        &state,
        &caller,
        id,
        format!("import archive into volume {volume} ({} bytes)", body.len()),
    )
    .await;
    Ok(Json(()))
}

/// The pieces that every handler needs: the checked platform, the app and its container.
struct FileContext {
    platform: Arc<crate::platform::Platform>,
    app: AppEntry,
}

async fn context(state: &AppState, caller: &Caller, id: ShortId) -> Result<FileContext, AppError> {
    let platform = state.platform(caller, Permission::Admin).await?;
    let app = platform.app(id).await.map_err(platform_error)?;
    Ok(FileContext { platform, app })
}

/// The volume that holds `path`, rejecting a path outside every mount. A write also needs a
/// mount that is not read-only.
fn check_path(app: &AppEntry, path: &str, write: bool) -> Result<VolumeMount, AppError> {
    let invalid = || {
        AppError::R3v3rs3(Error::InvalidFilePath {
            path: path.to_string(),
        })
    };
    let normalized = path.strip_prefix('/').ok_or_else(invalid)?;
    if normalized.split('/').any(|segment| segment == "..") {
        return Err(invalid());
    }
    let mount = app
        .spec
        .volumes
        .iter()
        .find(|mount| {
            let target = mount.target.as_str().trim_end_matches('/');
            normalized == target.trim_start_matches('/')
                || normalized.starts_with(&format!("{}/", target.trim_start_matches('/')))
        })
        .ok_or_else(invalid)?;
    if write && mount.read_only {
        return Err(AppError::R3v3rs3(Error::VolumeReadOnly {
            path: path.to_string(),
        }));
    }
    Ok(mount.clone())
}

async fn audit(state: &AppState, caller: &Caller, id: ShortId, summary: String) {
    let record = AuditRecord::new(AuditAction::AppFile)
        .id(id)
        .summary(summary);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
}
