//! Outbound webhook and event notification admin endpoints.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use axum::extract::{Path, State};
use axum::{Extension, Json};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::webhook::{
    CreateWebhookRequest, OutboundWebhook, TestWebhookRequest, TestWebhookResponse,
    UpdateWebhookRequest,
};

/// Lists all outbound webhooks.
#[utoipa::path(
    get,
    path = "/",
    tag = "webhooks",
    operation_id = "list_outbound_webhooks",
    responses((status = 200, description = "The outbound webhooks.", body = Vec<OutboundWebhook>), ErrorResponses)
)]
pub async fn list_webhooks(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<OutboundWebhook>>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let webhooks = platform.list_outbound_webhooks().await?;
    Ok(Json(webhooks))
}

/// Creates a new outbound webhook.
#[utoipa::path(
    post,
    path = "/",
    tag = "webhooks",
    operation_id = "create_outbound_webhook",
    request_body = CreateWebhookRequest,
    responses((status = 200, description = "The created outbound webhook.", body = OutboundWebhook), ErrorResponses)
)]
pub async fn create_webhook(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<CreateWebhookRequest>,
) -> Result<Json<OutboundWebhook>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform.create_outbound_webhook(request).await?;
    let record = AuditRecord::new(AuditAction::AddOutboundWebhook)
        .id(entry.id)
        .summary(format!("{} ({})", entry.name, entry.url));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(entry))
}

/// Gets a single outbound webhook by ID.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "webhooks",
    operation_id = "get_outbound_webhook",
    params(("id" = ShortId, Path, description = "The webhook id")),
    responses((status = 200, description = "The outbound webhook.", body = OutboundWebhook), NotFoundResponse, ErrorResponses)
)]
pub async fn get_webhook(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<OutboundWebhook>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform.get_outbound_webhook(id).await?;
    Ok(Json(entry))
}

/// Updates an existing outbound webhook.
#[utoipa::path(
    put,
    path = "/{id}",
    tag = "webhooks",
    operation_id = "update_outbound_webhook",
    params(("id" = ShortId, Path, description = "The webhook id")),
    request_body = UpdateWebhookRequest,
    responses((status = 200, description = "The updated outbound webhook.", body = OutboundWebhook), NotFoundResponse, ErrorResponses)
)]
pub async fn update_webhook(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
    Json(request): Json<UpdateWebhookRequest>,
) -> Result<Json<OutboundWebhook>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform.update_outbound_webhook(id, request).await?;
    let record = AuditRecord::new(AuditAction::UpdateOutboundWebhook)
        .id(entry.id)
        .summary(format!("{} ({})", entry.name, entry.url));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(entry))
}

/// Deletes an outbound webhook by ID.
#[utoipa::path(
    delete,
    path = "/{id}",
    tag = "webhooks",
    operation_id = "delete_outbound_webhook",
    params(("id" = ShortId, Path, description = "The webhook id")),
    responses((status = 200, description = "The outbound webhook was deleted."), NotFoundResponse, ErrorResponses)
)]
pub async fn delete_webhook(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<()>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let entry = platform.get_outbound_webhook(id).await.ok();
    platform.delete_outbound_webhook(id).await?;
    let summary = entry
        .map(|e| format!("{} ({})", e.name, e.url))
        .unwrap_or_else(|| id.to_string());
    let record = AuditRecord::new(AuditAction::DeleteOutboundWebhook)
        .id(id)
        .summary(summary);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(()))
}

/// Sends a test payload to an outbound webhook.
#[utoipa::path(
    post,
    path = "/test",
    tag = "webhooks",
    operation_id = "test_outbound_webhook",
    request_body = TestWebhookRequest,
    responses((status = 200, description = "The test result.", body = TestWebhookResponse), ErrorResponses)
)]
pub async fn test_webhook(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<TestWebhookRequest>,
) -> Result<Json<TestWebhookResponse>, AppError> {
    let platform = state.platform(&caller, Permission::Admin).await?;
    let target_summary = request
        .webhook_id
        .map(|id| id.to_string())
        .or_else(|| request.url.clone())
        .unwrap_or_else(|| "unknown".to_string());
    let res = platform.test_outbound_webhook(request).await?;
    let record = AuditRecord::new(AuditAction::TestOutboundWebhook)
        .summary(format!("{target_summary} -> success={}", res.success));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(res))
}
