use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::notify::{self, Notification, NotificationEvent};
use crate::server::rpc::config::{GetConfig, GetNotificationWebhook, SetConfig};
use axum::{Extension, Json, extract::State};
use r3v3rs3_api::app::AppConfig;
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
    let (webhook, node) = *state.call(&caller, GetNotificationWebhook).await?;
    let webhook = webhook.ok_or(Error::NotificationWebhookMissing)?;
    let notification = Notification::new(NotificationEvent::Test, &node);
    notify::deliver(&webhook, &notification)
        .await
        .map_err(|err| Error::NotificationFailed {
            reason: format!("{err:#}"),
        })?;
    Ok(Json(()))
}
