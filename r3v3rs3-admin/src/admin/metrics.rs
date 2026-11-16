//! The Prometheus metrics of the proxy. An admin session or an MCP Bearer token reads them, so a
//! scrape job can use `bearer_token` while the WebUI uses its cookie.

use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::stats;
use axum::{
    Extension,
    extract::State,
    http::{HeaderMap, HeaderName, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::CookieJar;

/// The exported metrics with their Prometheus HELP and TYPE lines.
const METRICS: [(&str, &str, &str); 9] = [
    (
        "r3v3rs3_h3_requests_total",
        "The proxied HTTP/3 requests on the QUIC ports of this process.",
        "counter",
    ),
    (
        "r3v3rs3_rx_bytes_total",
        "The total bytes received from clients by this process.",
        "counter",
    ),
    (
        "r3v3rs3_tx_bytes_total",
        "The total bytes transmitted to clients by this process.",
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
        "r3v3rs3_cache_bypasses_total",
        "The requests that bypassed the cache.",
        "counter",
    ),
    (
        "r3v3rs3_open_connections",
        "The client connection tasks that still run in this process.",
        "gauge",
    ),
    (
        "r3v3rs3_tls_handshakes_total",
        "The completed TLS handshakes of this process.",
        "counter",
    ),
    (
        "r3v3rs3_tls_handshake_errors_total",
        "The failed TLS handshakes of this process.",
        "counter",
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
    if let Some(token) = jar.get(super::auth::ADMIN_SESSION_COOKIE) {
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

fn format_basic_metrics(out: &mut String, values: &[(&str, u64)]) {
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
}

fn format_upstream_error_metrics(out: &mut String) {
    out.push_str("# HELP r3v3rs3_upstream_errors_by_kind_total The upstream errors partitioned by failure kind.\n");
    out.push_str("# TYPE r3v3rs3_upstream_errors_by_kind_total counter\n");
    for (kind, count) in stats::snapshot_upstream_errors() {
        out.push_str("r3v3rs3_upstream_errors_by_kind_total{kind=\"");
        out.push_str(kind);
        out.push_str("\"} ");
        out.push_str(&count.to_string());
        out.push('\n');
    }
}

fn escape_label_value(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// The proxy labels that the caller may read. `None` means every label, including the empty
/// label of requests that matched no proxy. Labels are compared as strings, because parsing the
/// empty label as a `ShortId` gives the zero ID instead of an error.
fn visible_proxies(caller: &Caller) -> Option<std::collections::BTreeSet<String>> {
    caller
        .proxies
        .as_ref()
        .map(|proxies| proxies.iter().map(ToString::to_string).collect())
}

/// Keeps the labeled rows whose proxy the caller may read.
fn visible_rows<T>(
    rows: Vec<(String, String, T)>,
    visible: &Option<std::collections::BTreeSet<String>>,
) -> Vec<(String, String, T)> {
    match visible {
        None => rows,
        Some(proxies) => rows
            .into_iter()
            .filter(|(proxy, _, _)| proxies.contains(proxy))
            .collect(),
    }
}

fn format_http_request_metrics(
    out: &mut String,
    visible: &Option<std::collections::BTreeSet<String>>,
) {
    out.push_str("# HELP r3v3rs3_http_requests_total The proxied HTTP requests on the TCP and TLS ports of this process.\n");
    out.push_str("# TYPE r3v3rs3_http_requests_total counter\n");
    let items = visible_rows(stats::snapshot_http_requests(), visible);
    if items.is_empty() {
        out.push_str("r3v3rs3_http_requests_total{proxy=\"\",route=\"\"} 0\n");
    } else {
        for (proxy, route, count) in items {
            let p = escape_label_value(&proxy);
            let r = escape_label_value(&route);
            out.push_str(&format!(
                "r3v3rs3_http_requests_total{{proxy=\"{p}\",route=\"{r}\"}} {count}\n"
            ));
        }
    }
}

fn format_upstream_error_total_metrics(
    out: &mut String,
    visible: &Option<std::collections::BTreeSet<String>>,
) {
    out.push_str("# HELP r3v3rs3_upstream_errors_total The requests that this process answered without a successful upstream response.\n");
    out.push_str("# TYPE r3v3rs3_upstream_errors_total counter\n");
    let items = visible_rows(stats::snapshot_upstream_error_totals(), visible);
    if items.is_empty() {
        out.push_str("r3v3rs3_upstream_errors_total{proxy=\"\",route=\"\"} 0\n");
    } else {
        for (proxy, route, count) in items {
            let p = escape_label_value(&proxy);
            let r = escape_label_value(&route);
            out.push_str(&format!(
                "r3v3rs3_upstream_errors_total{{proxy=\"{p}\",route=\"{r}\"}} {count}\n"
            ));
        }
    }
}

fn format_single_latency_histogram(
    out: &mut String,
    proxy: &str,
    route: &str,
    buckets: &[(f64, u64)],
    inf_count: u64,
    sum: f64,
    count: u64,
) {
    let p = escape_label_value(proxy);
    let r = escape_label_value(route);
    for (le, bucket_count) in buckets {
        out.push_str(&format!(
            "r3v3rs3_http_request_duration_seconds_bucket{{proxy=\"{p}\",route=\"{r}\",le=\"{le}\"}} {bucket_count}\n"
        ));
    }
    out.push_str(&format!(
        "r3v3rs3_http_request_duration_seconds_bucket{{proxy=\"{p}\",route=\"{r}\",le=\"+Inf\"}} {inf_count}\n"
    ));
    out.push_str(&format!(
        "r3v3rs3_http_request_duration_seconds_sum{{proxy=\"{p}\",route=\"{r}\"}} {sum:.6}\n"
    ));
    out.push_str(&format!(
        "r3v3rs3_http_request_duration_seconds_count{{proxy=\"{p}\",route=\"{r}\"}} {count}\n"
    ));
}

fn format_latency_histogram_metrics(
    out: &mut String,
    visible: &Option<std::collections::BTreeSet<String>>,
) {
    out.push_str(
        "# HELP r3v3rs3_http_request_duration_seconds HTTP request latencies in seconds.\n",
    );
    out.push_str("# TYPE r3v3rs3_http_request_duration_seconds histogram\n");
    let items = visible_rows(stats::snapshot_labeled_latency_histograms(), visible);
    if items.is_empty() {
        let (buckets, inf, sum, count) = stats::snapshot_latency_histogram();
        if visible.is_some() {
            // The process histogram holds the requests of every proxy, so an account with a
            // proxy list gets an empty one.
            let empty: Vec<(f64, u64)> = buckets.iter().map(|(le, _)| (*le, 0)).collect();
            format_single_latency_histogram(out, "", "", &empty, 0, 0.0, 0);
        } else {
            format_single_latency_histogram(out, "", "", &buckets, inf, sum, count);
        }
    } else {
        for (proxy, route, (buckets, inf, sum, count)) in items {
            format_single_latency_histogram(out, &proxy, &route, &buckets, inf, sum, count);
        }
    }
}

fn format_latency_quantile_metrics(out: &mut String) {
    out.push_str("# HELP r3v3rs3_http_request_duration_quantiles_seconds Request latency quantiles (p50, p95, p99) in seconds.\n");
    out.push_str("# TYPE r3v3rs3_http_request_duration_quantiles_seconds summary\n");
    let (p50, p95, p99) = stats::latency_quantiles();
    out.push_str(&format!(
        "r3v3rs3_http_request_duration_quantiles_seconds{{quantile=\"0.5\"}} {p50:.6}\n"
    ));
    out.push_str(&format!(
        "r3v3rs3_http_request_duration_quantiles_seconds{{quantile=\"0.95\"}} {p95:.6}\n"
    ));
    out.push_str(&format!(
        "r3v3rs3_http_request_duration_quantiles_seconds{{quantile=\"0.99\"}} {p99:.6}\n"
    ));
}

fn format_http_status_metrics(out: &mut String) {
    out.push_str(
        "# HELP r3v3rs3_http_responses_by_status_total The HTTP responses partitioned by status class.\n",
    );
    out.push_str("# TYPE r3v3rs3_http_responses_by_status_total counter\n");
    for (status, count) in stats::snapshot_http_statuses() {
        out.push_str("r3v3rs3_http_responses_by_status_total{status=\"");
        out.push_str(status);
        out.push_str("\"} ");
        out.push_str(&count.to_string());
        out.push('\n');
    }
}

fn format_tls_handshake_histogram_metrics(out: &mut String) {
    out.push_str(
        "# HELP r3v3rs3_tls_handshake_duration_seconds TLS handshake latencies in seconds.\n",
    );
    out.push_str("# TYPE r3v3rs3_tls_handshake_duration_seconds histogram\n");
    let (buckets, inf_count, sum, count) = stats::snapshot_tls_handshake_histogram();
    for (le, bucket_count) in buckets {
        out.push_str("r3v3rs3_tls_handshake_duration_seconds_bucket{le=\"");
        out.push_str(&le.to_string());
        out.push_str("\"} ");
        out.push_str(&bucket_count.to_string());
        out.push('\n');
    }
    out.push_str("r3v3rs3_tls_handshake_duration_seconds_bucket{le=\"+Inf\"} ");
    out.push_str(&inf_count.to_string());
    out.push_str("\nr3v3rs3_tls_handshake_duration_seconds_sum ");
    out.push_str(&format!("{sum:.6}"));
    out.push_str("\nr3v3rs3_tls_handshake_duration_seconds_count ");
    out.push_str(&count.to_string());
    out.push('\n');
}

fn format_tls_handshake_quantile_metrics(out: &mut String) {
    out.push_str("# HELP r3v3rs3_tls_handshake_duration_quantiles_seconds TLS handshake latency quantiles (p50, p95, p99) in seconds.\n");
    out.push_str("# TYPE r3v3rs3_tls_handshake_duration_quantiles_seconds summary\n");
    let (p50, p95, p99) = stats::tls_latency_quantiles();
    out.push_str(&format!(
        "r3v3rs3_tls_handshake_duration_quantiles_seconds{{quantile=\"0.5\"}} {p50:.6}\n"
    ));
    out.push_str(&format!(
        "r3v3rs3_tls_handshake_duration_quantiles_seconds{{quantile=\"0.95\"}} {p95:.6}\n"
    ));
    out.push_str(&format!(
        "r3v3rs3_tls_handshake_duration_quantiles_seconds{{quantile=\"0.99\"}} {p99:.6}\n"
    ));
}

fn format_system_metrics(out: &mut String, sys: &crate::system_metrics::SystemMetricsSnapshot) {
    if let Some(fds) = sys.open_fds {
        out.push_str("# HELP r3v3rs3_process_open_fds The number of open file descriptors.\n");
        out.push_str("# TYPE r3v3rs3_process_open_fds gauge\n");
        out.push_str("r3v3rs3_process_open_fds ");
        out.push_str(&fds.to_string());
        out.push('\n');
    }
    if let Some(max) = sys.max_fds {
        out.push_str(
            "# HELP r3v3rs3_process_max_fds The maximum number of open file descriptors.\n",
        );
        out.push_str("# TYPE r3v3rs3_process_max_fds gauge\n");
        out.push_str("r3v3rs3_process_max_fds ");
        out.push_str(&max.to_string());
        out.push('\n');
    }
    format_system_memory_metrics(out, sys);
    format_system_socket_metrics(out, sys);
    out.push_str(
        "# HELP r3v3rs3_event_loop_lag_seconds Current Tokio event loop lag in seconds.\n",
    );
    out.push_str("# TYPE r3v3rs3_event_loop_lag_seconds gauge\n");
    out.push_str(&format!(
        "r3v3rs3_event_loop_lag_seconds {:.6}\n",
        sys.event_loop_lag_seconds
    ));
}

fn format_system_socket_metrics(
    out: &mut String,
    sys: &crate::system_metrics::SystemMetricsSnapshot,
) {
    if let Some(tw) = sys.tcp_tw {
        out.push_str("# HELP r3v3rs3_host_tcp_tw TCP sockets in TIME_WAIT state.\n");
        out.push_str("# TYPE r3v3rs3_host_tcp_tw gauge\n");
        out.push_str("r3v3rs3_host_tcp_tw ");
        out.push_str(&tw.to_string());
        out.push('\n');
    }
    if let Some(inuse) = sys.tcp_inuse {
        out.push_str("# HELP r3v3rs3_host_tcp_inuse Active TCP sockets in use.\n");
        out.push_str("# TYPE r3v3rs3_host_tcp_inuse gauge\n");
        out.push_str("r3v3rs3_host_tcp_inuse ");
        out.push_str(&inuse.to_string());
        out.push('\n');
    }
}

fn format_system_memory_metrics(
    out: &mut String,
    sys: &crate::system_metrics::SystemMetricsSnapshot,
) {
    if let Some(rss) = sys.resident_memory_bytes {
        out.push_str(
            "# HELP r3v3rs3_process_resident_memory_bytes Resident memory size in bytes.\n",
        );
        out.push_str("# TYPE r3v3rs3_process_resident_memory_bytes gauge\n");
        out.push_str("r3v3rs3_process_resident_memory_bytes ");
        out.push_str(&rss.to_string());
        out.push('\n');
    }
    if let Some(vm) = sys.virtual_memory_bytes {
        out.push_str("# HELP r3v3rs3_process_virtual_memory_bytes Virtual memory size in bytes.\n");
        out.push_str("# TYPE r3v3rs3_process_virtual_memory_bytes gauge\n");
        out.push_str("r3v3rs3_process_virtual_memory_bytes ");
        out.push_str(&vm.to_string());
        out.push('\n');
    }
}

/// Exports the proxy counters in the Prometheus text format. An account with a proxy list gets
/// the per-proxy series of its own proxies only; the process totals stay unlabeled.
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
    headers: HeaderMap,
    Extension(caller): Extension<Caller>,
) -> Result<Response, AppError> {
    let sys = crate::system_metrics::collect_system_metrics();
    if headers
        .get(header::ACCEPT)
        .and_then(|val| val.to_str().ok())
        .is_some_and(|accept| accept.contains("application/json"))
    {
        return Ok(axum::Json(sys).into_response());
    }

    let mut out = String::new();
    let values = stats::snapshot();
    let visible = visible_proxies(&caller);
    format_basic_metrics(&mut out, &values);
    format_http_request_metrics(&mut out, &visible);
    format_upstream_error_total_metrics(&mut out, &visible);
    format_http_status_metrics(&mut out);
    format_upstream_error_metrics(&mut out);
    format_latency_histogram_metrics(&mut out, &visible);
    format_latency_quantile_metrics(&mut out);
    format_tls_handshake_histogram_metrics(&mut out);
    format_tls_handshake_quantile_metrics(&mut out);
    format_system_metrics(&mut out, &sys);
    Ok((
        [(
            HeaderName::from_static("content-type"),
            PROMETHEUS_CONTENT_TYPE,
        )],
        out,
    )
        .into_response())
}

const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Exports the pre-configured Grafana dashboard JSON.
#[utoipa::path(
    get,
    path = "/grafana",
    tag = "metrics",
    operation_id = "get_grafana_dashboard",
    responses(
        (status = 200, description = "The ready-to-import Grafana dashboard JSON.", body = String),
        (status = 401, description = "No active admin session or Bearer token."),
    )
)]
pub async fn grafana_dashboard(
    Extension(_caller): Extension<Caller>,
) -> Result<Response, AppError> {
    const DASHBOARD: &str = include_str!("../../../r3v3rs3/data/r3v3rs3-grafana.json");
    Ok((
        [
            (
                HeaderName::from_static("content-type"),
                "application/json; charset=utf-8",
            ),
            (HeaderName::from_static("cache-control"), "no-store"),
        ],
        DASHBOARD,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::{auth::Role, id::ShortId};

    fn caller(proxies: Option<&[ShortId]>) -> Caller {
        Caller {
            username: "viewer".into(),
            role: Role::Viewer,
            proxies: proxies.map(|ids| ids.iter().copied().collect()),
            client: None,
        }
    }

    /// Records a request, an upstream error and a duration for the proxy.
    fn record(proxy: &str) {
        stats::count_http_request(Some(proxy), Some("/"));
        stats::count_upstream_error(Some(proxy), Some("/"));
        stats::record_request_duration(std::time::Duration::from_millis(5), Some(proxy), Some("/"));
    }

    async fn body_for(caller: Caller) -> String {
        let Ok(response) = metrics(HeaderMap::new(), Extension(caller)).await else {
            panic!("the metrics handler failed");
        };
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn an_account_with_a_proxy_list_reads_only_the_series_of_its_proxies() {
        // Fixed IDs that no other test records; `ShortId::new` is the zero ID.
        let own = ShortId::from(*b"mtrcown");
        let other = ShortId::from(*b"mtrcoth");
        record(&own.to_string());
        record(&other.to_string());

        let scoped = body_for(caller(Some(&[own]))).await;
        assert!(scoped.contains(&format!("proxy=\"{own}\"")));
        assert!(!scoped.contains(&other.to_string()));

        let unscoped = body_for(caller(None)).await;
        assert!(unscoped.contains(&format!("proxy=\"{own}\"")));
        assert!(unscoped.contains(&format!("proxy=\"{other}\"")));
    }
}
