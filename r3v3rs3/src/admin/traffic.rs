use super::AppError;
use super::openapi::ErrorResponses;
use crate::accounts::Caller;
use axum::{
    Extension, Json,
    extract::Query,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::{Stream, StreamExt};
use r3v3rs3_api::traffic::{AccessLogEntry, AccessLogQuery};
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
