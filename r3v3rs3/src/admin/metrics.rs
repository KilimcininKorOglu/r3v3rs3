//! The Prometheus metrics of the proxy. An admin session or an MCP Bearer token reads them, so a
//! scrape job can use `bearer_token` while the WebUI uses its cookie.

use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::proxy::stats;
use axum::{
    Extension,
    extract::State,
    http::{HeaderMap, HeaderName, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;

/// The exported metrics with their Prometheus HELP and TYPE lines.
const METRICS: [(&str, &str, &str); 6] = [
    (
        "r3v3rs3_http_requests_total",
        "The proxied HTTP requests on the TCP and TLS ports of this process.",
        "counter",
    ),
    (
        "r3v3rs3_h3_requests_total",
        "The proxied HTTP/3 requests on the QUIC ports of this process.",
        "counter",
    ),
    (
        "r3v3rs3_upstream_errors_total",
        "The requests that this process answered without a successful upstream response.",
        "counter",
    ),
    (
        "r3v3rs3_cache_hits_total",
        "The requests that a stored cache response of this process answered.",
        "counter",
    ),
    (
        "r3v3rs3_cache_misses_total",
        "The cached requests of this process that went to the upstream server.",
        "counter",
    ),
    (
        "r3v3rs3_open_connections",
        "The client connection tasks that still run in this process.",
        "gauge",
    ),
];

/// Accepts a request with an active admin session or a valid MCP Bearer token, and adds the
/// `Caller` of its account to the request.
pub async fn verify_metrics(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    mut request: axum::extract::Request,
    next: Next,
) -> Response {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    {
        if let Some(caller) = super::auth::api_token_caller(&state, token) {
            request.extensions_mut().insert(caller);
            return next.run(request).await;
        }
        let username = {
            let data = state.data.lock().await;
            let enabled = data.config.mcp.enabled;
            enabled
                .then(|| data.mcp.verify(token))
                .flatten()
                .map(|grant| grant.username().to_string())
        };
        let Some(username) = username else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        let record = crate::sessions::SessionRecord::new(&username);
        let Some(caller) = state.accounts.borrow().caller(&record) else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        request.extensions_mut().insert(caller);
        return next.run(request).await;
    }
    if let Some(token) = jar.get("token") {
        match super::auth::session_caller(&state, token.value()).await {
            Ok(caller) => {
                request.extensions_mut().insert(caller);
                return next.run(request).await;
            }
            Err(_) => return StatusCode::UNAUTHORIZED.into_response(),
        }
    }
    StatusCode::UNAUTHORIZED.into_response()
}

/// Exports the proxy counters in the Prometheus text format.
#[utoipa::path(
    get,
    path = "/",
    tag = "metrics",
    operation_id = "get_metrics",
    responses(
        (status = 200, description = "The metrics in the Prometheus text format.", body = String),
        (status = 401, description = "No active admin session or Bearer token."),
    )
)]
pub async fn metrics(
    Extension(_caller): Extension<Caller>,
) -> Result<([(HeaderName, &'static str); 1], String), AppError> {
    let mut out = String::new();
    let values = stats::snapshot();
    for (name, help, kind) in METRICS {
        out.push_str("# HELP ");
        out.push_str(name);
        out.push(' ');
        out.push_str(help);
        out.push_str("\n# TYPE ");
        out.push_str(name);
        out.push(' ');
        out.push_str(kind);
        out.push('\n');
        if let Some((_, value)) = values.iter().find(|(value_name, _)| *value_name == name) {
            out.push_str(name);
            out.push(' ');
            out.push_str(&value.to_string());
            out.push('\n');
        }
    }
    Ok((
        [(
            HeaderName::from_static("content-type"),
            PROMETHEUS_CONTENT_TYPE,
        )],
        out,
    ))
}

const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";
