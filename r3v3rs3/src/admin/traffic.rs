use super::AppError;
use super::openapi::ErrorResponses;
use crate::accounts::{Caller, Permission};
use axum::{
    Extension, Json,
    extract::{Path, Query},
    response::sse::{Event, KeepAlive, Sse},
};
use futures::{Stream, StreamExt};
use r3v3rs3_api::traffic::{
    AccessLogEntry, AccessLogQuery, HeaderEntry, RequestReplayRequest, RequestReplayResponse,
    TrafficGeoDistribution,
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
    caller.authorize(Permission::Read).map_err(AppError::from)?;
    let timeout =
        std::time::Duration::from_millis(request.timeout_ms.unwrap_or(10_000).clamp(500, 30_000));
    let start = std::time::Instant::now();
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .danger_accept_invalid_certs(true)
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
