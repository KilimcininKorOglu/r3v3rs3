use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::notify::{self, Notification, NotificationEvent};
use crate::server::rpc::config::{
    ExportConfigBackup, GetConfig, GetNotificationWebhook, ImportConfigBackup, SetConfig,
};
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::config_backup::ConfigBackup;
use r3v3rs3_api::error::Error;

/// Returns the application settings.
#[utoipa::path(
    get,
    path = "/",
    tag = "config",
    operation_id = "get_config",
    responses((status = 200, description = "The settings.", body = AppConfig), ErrorResponses)
)]
pub async fn get(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<AppConfig>>, AppError> {
    Ok(Json(state.call(&caller, GetConfig).await?))
}

/// Replaces the application settings.
#[utoipa::path(
    put,
    path = "/",
    tag = "config",
    operation_id = "update_config",
    request_body = AppConfig,
    responses((status = 200, description = "The settings are saved."), ErrorResponses)
)]
pub async fn put(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(config): Json<AppConfig>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(state.call(&caller, SetConfig { config }).await?))
}

/// Sends a test notification to the webhook of the saved settings.
#[utoipa::path(
    post,
    path = "/notifications/test",
    tag = "config",
    operation_id = "test_notification",
    responses((status = 200, description = "The webhook accepted the test notification."), ErrorResponses)
)]
pub async fn test_notification(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<()>, AppError> {
    let webhook = *state.call(&caller, GetNotificationWebhook).await?;
    let webhook = webhook.ok_or(Error::NotificationWebhookMissing)?;
    let notification = Notification::new(NotificationEvent::Test, "");
    notify::deliver(&webhook, &notification)
        .await
        .map_err(|err| Error::NotificationFailed {
            reason: format!("{err:#}"),
        })?;
    Ok(Json(()))
}

/// Returns a configuration backup containing all storage files and settings, with secrets masked and accounts excluded.
#[utoipa::path(
    get,
    path = "/backup",
    tag = "config",
    operation_id = "backup_config",
    responses((status = 200, description = "The configuration backup.", body = ConfigBackup), ErrorResponses)
)]
pub async fn backup(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Box<ConfigBackup>>, AppError> {
    Ok(Json(state.call(&caller, ExportConfigBackup).await?))
}

/// Restores configuration from a backup file.
#[utoipa::path(
    post,
    path = "/restore",
    tag = "config",
    operation_id = "restore_config",
    request_body = ConfigBackup,
    responses((status = 200, description = "The configuration was restored successfully."), ErrorResponses)
)]
pub async fn restore(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(backup): Json<ConfigBackup>,
) -> Result<Json<Box<()>>, AppError> {
    Ok(Json(
        state
            .call(
                &caller,
                ImportConfigBackup {
                    backup: Box::new(backup),
                },
            )
            .await?,
    ))
}
