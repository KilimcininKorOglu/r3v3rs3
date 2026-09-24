//! The OAuth callback of the Git provider connections. The provider sends the browser of the admin
//! here with a code, after an authorization or after GitHub created a GitHub App. The session
//! cookie is `SameSite=Strict`, so this cross-site navigation carries no session: the one-time
//! `state` of the authorization proves the request instead.

use super::{AppError, AppState};
use crate::audit::AuditRecord;
use crate::platform::oauth::Authorized;
use axum::{
    extract::{Query, State},
    http::{HeaderValue, header},
    response::{Html, IntoResponse, Response},
};
use r3v3rs3_api::{audit::AuditAction, error::Error};
use serde::Deserialize;
use tracing::{error, warn};
use utoipa::IntoParams;

/// The page of the WebUI that lists the connections.
const CONNECTIONS_PAGE: &str = "/git_connections";

/// The query of the callback.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CallbackQuery {
    /// The code that the token request trades.
    code: Option<String>,
    /// The one-time state of the authorization.
    state: Option<String>,
    /// The error of a refused authorization.
    error: Option<String>,
}

/// Finishes the authorization of a Git provider connection, then sends the browser to the
/// connections page of the WebUI with the result. A new GitHub App sends it to its installation
/// page on GitHub instead.
#[utoipa::path(
    get,
    path = "/git/callback",
    tag = "platform",
    operation_id = "git_oauth_callback",
    security(()),
    params(CallbackQuery),
    responses((status = 200, description = "A page that opens the connections page of the WebUI with the result.", content_type = "text/html"))
)]
pub async fn git_callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    let platform = state.data.lock().await.platform.get()?.clone();
    let target = match finish(&platform, query).await {
        Ok(authorized) => {
            record(&state, &authorized).await;
            match authorized.install_url {
                Some(install_url) => install_url,
                None => format!("{CONNECTIONS_PAGE}?connected={}", authorized.entry.id),
            }
        }
        Err(code) => format!("{CONNECTIONS_PAGE}?error={code}"),
    };
    Ok(page(&target))
}

/// The authorized connection, or the error code of the failure.
async fn finish(
    platform: &crate::platform::Platform,
    query: CallbackQuery,
) -> Result<Authorized, &'static str> {
    let state = query.state.unwrap_or_default();
    if let Some(reason) = query.error {
        let connection = platform.cancel_git_authorization(&state);
        warn!(
            ?connection,
            reason, "the Git provider refused the authorization"
        );
        return Err("git_authorization_refused");
    }
    let code = query.code.unwrap_or_default();
    platform
        .finish_git_authorization(&state, &code)
        .await
        .map_err(|err| error_code(&err))
}

/// The code of an API error. Any other failure is logged, because its text can hold file paths of
/// the server.
fn error_code(err: &anyhow::Error) -> &'static str {
    match err.downcast_ref::<Error>() {
        Some(Error::OauthStateInvalid) => "oauth_state_invalid",
        Some(Error::GitProviderFailed { reason }) => {
            warn!(reason, "the Git provider failed the authorization");
            "git_provider_failed"
        }
        Some(Error::IdNotFound { .. }) => "id_not_found",
        _ => {
            error!("the authorization of a Git provider connection failed: {err:#}");
            "platform_failed"
        }
    }
}

/// A new GitHub App adds a connection, and an authorization connects one. The summary never
/// holds a token.
async fn record(state: &AppState, authorized: &Authorized) {
    let entry = &authorized.entry;
    let (action, summary) = if authorized.install_url.is_some() {
        let summary = format!("{} ({})", entry.name, entry.provider.as_str());
        (AuditAction::AddGitConnection, summary)
    } else {
        (
            AuditAction::ConnectGitConnection,
            super::git::connected_summary(entry),
        )
    };
    let record = AuditRecord::new(action).id(entry.id).summary(summary);
    state
        .record_audit(&authorized.username, authorized.client, record)
        .await;
}

/// A page that opens `target`. The navigation starts on this origin, so the browser sends the
/// session cookie again. The address of the callback holds the code, so no referrer leaves.
fn page(target: &str) -> Response {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta http-equiv=\"refresh\" content=\"0;url={target}\"><title>r3v3rs3</title></head>\
         <body><a href=\"{target}\">r3v3rs3</a></body></html>"
    );
    let mut response = Html(body).into_response();
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response
}
