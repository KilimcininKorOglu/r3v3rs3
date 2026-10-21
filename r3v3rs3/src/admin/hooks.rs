//! The webhook requests of the Git providers. They carry no session cookie: the signature of each
//! request proves that the provider knows the webhook secret of the app.

use super::platform::platform_error;
use super::{AppError, AppState};
use crate::audit::AuditRecord;
use axum::{
    Json,
    body::Bytes,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use r3v3rs3_api::{
    audit::AuditAction,
    error::ErrorMessage,
    id::ShortId,
    platform::{HookOutcome, HookResponse},
};
use serde_derive::Deserialize;
use std::net::SocketAddr;
use utoipa::IntoParams;

/// Query parameters for webhooks from registries (Docker Hub, Quay) or direct triggers.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
pub struct WebhookQuery {
    /// Webhook secret token for container registries (Docker Hub, Quay) or direct callers.
    #[serde(default)]
    pub secret: Option<String>,
    /// Alternative query parameter for the webhook secret token.
    #[serde(default)]
    pub token: Option<String>,
}

/// Deploys an app for a signed push event of GitHub, GitLab, Gitea or Forgejo, or for a signed
/// request of another sender. The request needs no session: the signature header proves it.
#[utoipa::path(
    post,
    path = "/apps/{id}",
    tag = "platform",
    operation_id = "app_webhook",
    security(()),
    params(
        ("id" = ShortId, Path, description = "App id."),
        WebhookQuery,
    ),
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
    Query(query): Query<WebhookQuery>,
    mut headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    if let Some(token) = query.secret.as_deref().or(query.token.as_deref())
        && !headers.contains_key("x-webhook-token")
        && let Ok(val) = HeaderValue::from_str(token)
    {
        headers.insert("x-webhook-token", val);
    }
    let platform = state
        .call_system(crate::server::rpc::auth::GetPlatform)
        .await?
        .get()?
        .clone();
    let (app, provider, response) = platform
        .hook(id, &headers, &body)
        .await
        .map_err(platform_error)?;
    let status = match response.outcome {
        HookOutcome::Queued => StatusCode::ACCEPTED,
        HookOutcome::Deployed
        | HookOutcome::PreviewDeployed
        | HookOutcome::PreviewDestroyed
        | HookOutcome::Ignored => StatusCode::OK,
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
