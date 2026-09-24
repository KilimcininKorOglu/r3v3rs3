//! The webhook requests of the Git providers. They carry no session cookie: the signature of each
//! request proves that the provider knows the webhook secret of the app.

use super::platform::platform_error;
use super::{AppError, AppState};
use crate::audit::AuditRecord;
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use r3v3rs3_api::{
    audit::AuditAction,
    error::ErrorMessage,
    id::ShortId,
    platform::{HookOutcome, HookResponse},
};
use std::net::SocketAddr;

/// Deploys an app for a signed push event of GitHub, GitLab, Gitea or Forgejo, or for a signed
/// request of another sender. The request needs no session: the signature header proves it.
#[utoipa::path(
    post,
    path = "/apps/{id}",
    tag = "platform",
    operation_id = "app_webhook",
    security(()),
    params(("id" = ShortId, Path, description = "App id.")),
    request_body(content = String, description = "The event of the Git provider.", content_type = "application/json"),
    responses(
        (status = 200, description = "The request started a deployment, or its event or branch does not deploy the app.", body = HookResponse),
        (status = 202, description = "A deployment of the app is running, so one more deployment starts after it.", body = HookResponse),
        (status = 400, description = "The event is not JSON.", body = ErrorMessage),
        (status = 401, description = "The signature is missing or wrong.", body = ErrorMessage),
        (status = 404, description = "No app has this id, or the app has no webhook secret.", body = ErrorMessage),
        (status = 429, description = "The client sent more than 10 requests in a burst, or more than one request per second after the burst."),
        (status = 503, description = "The deployment platform is off.", body = ErrorMessage)
    )
)]
pub async fn app_hook(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<ShortId>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let platform = state.data.lock().await.platform.get()?.clone();
    let (app, provider, response) = platform
        .hook(id, &headers, &body)
        .await
        .map_err(platform_error)?;
    let status = match response.outcome {
        HookOutcome::Queued => StatusCode::ACCEPTED,
        HookOutcome::Deployed | HookOutcome::Ignored => StatusCode::OK,
    };
    if response.outcome != HookOutcome::Ignored {
        let started = response
            .deployment
            .as_ref()
            .map_or_else(|| "queued".to_string(), |d| d.id.to_string());
        let record = AuditRecord::new(AuditAction::AppWebhook)
            .id(id)
            .summary(format!("{} ({}): {started}", app.name, provider.as_str()));
        // A webhook request has no account.
        state.record_audit("", Some(peer.ip()), record).await;
    }
    Ok((status, Json(response)).into_response())
}
