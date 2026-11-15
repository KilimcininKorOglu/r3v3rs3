use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use axum::{
    Extension, Json,
    extract::{Path, Query, State},
    response::sse::{Event, KeepAlive, Sse},
};
use futures::{Stream, StreamExt};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::traffic::{
    AccessLogEntry, AccessLogQuery, ActiveConnectionInfo, HeaderEntry, RequestReplayRequest,
    RequestReplayResponse, TrafficGeoDistribution,
};
use tokio_stream::wrappers::BroadcastStream;

/// Returns recent HTTP access logs matching the query filters, newest first.
#[utoipa::path(
    get,
    path = "/",
    tag = "traffic",
    operation_id = "get_traffic",
    params(AccessLogQuery),
    responses((status = 200, description = "Recent access log entries.", body = Vec<AccessLogEntry>), ErrorResponses)
)]
pub async fn get(
    Extension(caller): Extension<Caller>,
    Query(query): Query<AccessLogQuery>,
) -> Result<Json<Vec<AccessLogEntry>>, AppError> {
    let entries = crate::proxy::access_log::recent(&query);
    let filtered = entries
        .into_iter()
        .filter(|e| can_see_entry(&caller, e))
        .collect();
    Ok(Json(filtered))
}

/// Streams live HTTP access logs as Server-Sent Events.
#[utoipa::path(
    get,
    path = "/live",
    tag = "traffic",
    operation_id = "stream_traffic",
    params(AccessLogQuery),
    responses((status = 200, description = "Live access log stream.", content_type = "text/event-stream"), ErrorResponses)
)]
pub async fn live(
    Extension(caller): Extension<Caller>,
    Query(query): Query<AccessLogQuery>,
) -> Sse<impl Stream<Item = Result<Event, axum::Error>>> {
    let rx = crate::proxy::access_log::subscribe();
    let stream = BroadcastStream::new(rx).filter_map(move |item| {
        let caller = caller.clone();
        let query = query.clone();
        async move {
            match item {
                Ok(entry) if can_see_entry(&caller, &entry) && entry.matches(&query) => {
                    serde_json::to_string(&entry)
                        .ok()
                        .map(|data| Ok(Event::default().data(data)))
                }
                _ => None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn can_see_entry(caller: &Caller, entry: &AccessLogEntry) -> bool {
    match entry.resource_id {
        Some(id) => caller.can_see(id),
        None => caller.proxies.is_none(),
    }
}

/// Returns the aggregated geographic distribution and top countries of recent access traffic.
#[utoipa::path(
    get,
    path = "/geo",
    tag = "traffic",
    operation_id = "get_traffic_geo",
    params(AccessLogQuery),
    responses((status = 200, description = "Geographical traffic distribution.", body = TrafficGeoDistribution), ErrorResponses)
)]
pub async fn geo(
    Extension(caller): Extension<Caller>,
    Query(query): Query<AccessLogQuery>,
) -> Result<Json<TrafficGeoDistribution>, AppError> {
    if let Some(id) = query.resource_id
        && !caller.can_see(id)
    {
        return Err(AppError::from(r3v3rs3_api::error::Error::IdNotFound {
            id: id.to_string(),
        }));
    }
    let res = if let Some(id) = query.resource_id {
        crate::proxy::access_log::geo_distribution(Some(&id))
    } else if let Some(proxies) = &caller.proxies {
        crate::proxy::access_log::geo_distribution_matching(|id| {
            id.is_some_and(|p| proxies.contains(p))
        })
    } else {
        crate::proxy::access_log::geo_distribution(None)
    };
    Ok(Json(res))
}

/// Returns the detailed access log entry matching the given ID.
#[utoipa::path(
    get,
    path = "/{id}",
    tag = "traffic",
    operation_id = "get_traffic_entry",
    params(("id" = u64, Path, description = "Access log entry ID.")),
    responses((status = 200, description = "The access log entry.", body = AccessLogEntry), ErrorResponses)
)]
pub async fn get_entry(
    Extension(caller): Extension<Caller>,
    Path(id): Path<u64>,
) -> Result<Json<AccessLogEntry>, AppError> {
    let entry = crate::proxy::access_log::get_by_id(id)
        .filter(|e| can_see_entry(&caller, e))
        .ok_or_else(|| {
            AppError::from(r3v3rs3_api::error::Error::IdNotFound { id: id.to_string() })
        })?;
    Ok(Json(entry))
}

fn is_hop_by_hop_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailers"
            | "transfer-encoding"
            | "content-length"
    )
}

fn is_cloud_metadata(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            a == 169 && b == 254
        }
        std::net::IpAddr::V6(v6) => {
            let octets = v6.octets();
            octets[0] == 0xfe && (octets[1] & 0xc0) == 0x80
        }
    }
}

fn is_restricted_ipv4(v4: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = v4.octets();
    matches!(a, 0 | 10)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
}

fn is_restricted_ip(ip: std::net::IpAddr) -> bool {
    if is_cloud_metadata(ip) || ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    match ip {
        std::net::IpAddr::V4(v4) => is_restricted_ipv4(v4),
        std::net::IpAddr::V6(v6) => (v6.octets()[0] & 0xfe) == 0xfc,
    }
}

fn is_cloud_metadata_domain(domain: &str) -> bool {
    let lower = domain.to_ascii_lowercase();
    let trimmed = lower.trim_end_matches('.');
    trimmed == "metadata.google.internal" || trimmed == "instance-data"
}

fn is_blocked_domain(domain: &str) -> bool {
    let lower = domain.to_ascii_lowercase();
    let trimmed = lower.trim_end_matches('.');
    if trimmed == "localhost" {
        return true;
    }
    const SUFFIXES: &[&str] = &[
        ".localhost",
        ".local",
        ".internal",
        ".lan",
        ".home.arpa",
        ".corp",
    ];
    SUFFIXES.iter().any(|suffix| trimmed.ends_with(suffix))
}

async fn validate_resolved_addrs(
    host: &str,
    port: u16,
    allow_internal: bool,
) -> Result<(), AppError> {
    if let Ok(addrs) = tokio::net::lookup_host((host, port)).await {
        for socket_addr in addrs {
            validate_ip_target(socket_addr.ip(), allow_internal)?;
        }
    }
    Ok(())
}

async fn validate_replay_target(parsed: &url::Url, allow_internal: bool) -> Result<(), AppError> {
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(AppError::Anyhow(anyhow::anyhow!(
            "only http and https schemes are supported for replay"
        )));
    }
    let host_str = parsed
        .host_str()
        .ok_or_else(|| AppError::Anyhow(anyhow::anyhow!("missing host in replay URL")))?;

    if let Ok(ip) = host_str.parse::<std::net::IpAddr>() {
        return validate_ip_target(ip, allow_internal);
    }

    if is_cloud_metadata_domain(host_str) {
        return Err(AppError::Anyhow(anyhow::anyhow!(
            "access to cloud metadata services is forbidden"
        )));
    }

    if is_blocked_domain(host_str) && !allow_internal {
        return Err(AppError::Anyhow(anyhow::anyhow!(
            "replay destination targets an internal domain"
        )));
    }

    let port = parsed.port_or_known_default().unwrap_or(80);
    validate_resolved_addrs(host_str, port, allow_internal).await
}

fn validate_ip_target(ip: std::net::IpAddr, allow_internal: bool) -> Result<(), AppError> {
    if is_cloud_metadata(ip) {
        return Err(AppError::Anyhow(anyhow::anyhow!(
            "access to cloud metadata services is forbidden"
        )));
    }
    if !allow_internal && is_restricted_ip(ip) {
        return Err(AppError::Anyhow(anyhow::anyhow!(
            "replay destination targets a private or loopback address"
        )));
    }
    Ok(())
}

/// Replays a captured or custom HTTP/WebSocket request to its target destination.
#[utoipa::path(
    post,
    path = "/replay",
    tag = "traffic",
    operation_id = "replay_traffic_request",
    request_body = RequestReplayRequest,
    responses((status = 200, description = "Result of replaying the request.", body = RequestReplayResponse), ErrorResponses)
)]
pub async fn replay(
    Extension(caller): Extension<Caller>,
    Json(request): Json<RequestReplayRequest>,
) -> Result<Json<RequestReplayResponse>, AppError> {
    caller
        .authorize(Permission::Admin)
        .map_err(AppError::from)?;
    let parsed_url = url::Url::parse(&request.url)
        .map_err(|e| AppError::Anyhow(anyhow::anyhow!("invalid replay URL: {e}")))?;
    validate_replay_target(&parsed_url, request.allow_internal).await?;

    let timeout =
        std::time::Duration::from_millis(request.timeout_ms.unwrap_or(10_000).clamp(500, 30_000));
    let start = std::time::Instant::now();
    let mut client_builder = reqwest::Client::builder().timeout(timeout);
    if request.insecure_skip_verify {
        client_builder = client_builder.danger_accept_invalid_certs(true);
    }
    let client = client_builder
        .build()
        .map_err(|e| AppError::Anyhow(e.into()))?;

    let method =
        reqwest::Method::from_bytes(request.method.as_bytes()).unwrap_or(reqwest::Method::GET);

    let mut builder = client.request(method, &request.url);
    for h in &request.headers {
        if !is_hop_by_hop_header(&h.name) {
            builder = builder.header(&h.name, &h.value);
        }
    }
    if let Some(body) = request.body {
        builder = builder.body(body);
    }

    match builder.send().await {
        Ok(res) => {
            let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
            let status = res.status().as_u16();
            let headers = res
                .headers()
                .iter()
                .map(|(k, v)| HeaderEntry::new(k.as_str(), v.to_str().unwrap_or_default()))
                .collect();
            let body = res.text().await.unwrap_or_default();
            Ok(Json(RequestReplayResponse {
                status,
                latency_ms,
                headers,
                body,
                error: None,
            }))
        }
        Err(err) => {
            let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
            Ok(Json(RequestReplayResponse {
                status: 502,
                latency_ms,
                headers: Vec::new(),
                body: String::new(),
                error: Some(err.to_string()),
            }))
        }
    }
}

#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct ConnectionListQuery {
    pub proxy_id: Option<ShortId>,
}

/// Lists active long-lived connections and streams.
#[utoipa::path(
    get,
    path = "/connections",
    tag = "traffic",
    operation_id = "list_traffic_connections",
    params(
        ConnectionListQuery
    ),
    responses((status = 200, description = "List of active long-lived connections.", body = Vec<ActiveConnectionInfo>), ErrorResponses)
)]
pub async fn list_connections(
    Extension(caller): Extension<Caller>,
    Query(query): Query<ConnectionListQuery>,
) -> Result<Json<Vec<ActiveConnectionInfo>>, AppError> {
    caller.authorize(Permission::Read)?;
    let conns = crate::proxy::connection_tracker::list(query.proxy_id.as_ref());
    let filtered = conns
        .into_iter()
        .filter(|c| match c.proxy_id {
            Some(id) => caller.can_see(id),
            None => caller.proxies.is_none(),
        })
        .collect();
    Ok(Json(filtered))
}

/// Terminates an active connection by its unique ID.
#[utoipa::path(
    delete,
    path = "/connections/{id}",
    tag = "traffic",
    operation_id = "terminate_traffic_connection",
    params(("id" = ShortId, Path, description = "Connection ID.")),
    responses((status = 200, description = "The connection was terminated."), ErrorResponses)
)]
pub async fn terminate_connection(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ShortId>,
) -> Result<Json<()>, AppError> {
    caller.authorize(Permission::Edit)?;
    crate::proxy::connection_tracker::terminate(id)?;
    let record =
        crate::audit::AuditRecord::new(r3v3rs3_api::audit::AuditAction::TerminateConnection);
    state
        .record_audit(&caller.username, caller.client, record)
        .await;
    Ok(Json(()))
}

/// Returns HTTP stream priority metrics and adaptive backpressure counters.
#[utoipa::path(
    get,
    path = "/priority",
    tag = "traffic",
    operation_id = "get_traffic_priority_stats",
    responses((status = 200, description = "Stream priority statistics.", body = r3v3rs3_api::priority::StreamPriorityStats), ErrorResponses)
)]
pub async fn priority_stats(
    Extension(caller): Extension<Caller>,
) -> Result<Json<r3v3rs3_api::priority::StreamPriorityStats>, AppError> {
    caller.authorize(Permission::Read)?;
    Ok(Json(
        r3v3rs3_api::priority::GLOBAL_STREAM_CONTROLLER.stats(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cloud_metadata_is_always_forbidden() {
        let meta_v4 = url::Url::parse("http://169.254.169.254/latest/meta-data").unwrap();
        assert!(validate_replay_target(&meta_v4, false).await.is_err());
        assert!(validate_replay_target(&meta_v4, true).await.is_err());

        let meta_domain =
            url::Url::parse("http://metadata.google.internal/computeMetadata/v1").unwrap();
        assert!(validate_replay_target(&meta_domain, false).await.is_err());
        assert!(validate_replay_target(&meta_domain, true).await.is_err());
    }

    #[tokio::test]
    async fn internal_and_loopback_destinations_require_allow_internal() {
        let loopback = url::Url::parse("http://127.0.0.1:8080/metrics").unwrap();
        assert!(validate_replay_target(&loopback, false).await.is_err());
        assert!(validate_replay_target(&loopback, true).await.is_ok());

        let private_ip = url::Url::parse("http://10.0.1.5:8080/status").unwrap();
        assert!(validate_replay_target(&private_ip, false).await.is_err());
        assert!(validate_replay_target(&private_ip, true).await.is_ok());

        let local_domain = url::Url::parse("http://localhost:3000/").unwrap();
        assert!(validate_replay_target(&local_domain, false).await.is_err());
        assert!(validate_replay_target(&local_domain, true).await.is_ok());
    }

    #[tokio::test]
    async fn public_destinations_are_allowed() {
        let public_url = url::Url::parse("https://93.184.216.34/test").unwrap();
        assert!(validate_replay_target(&public_url, false).await.is_ok());
    }
}
