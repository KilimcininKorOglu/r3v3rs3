//! The process-wide proxy counters that the admin metrics endpoint exports. The counters count
//! one process: with worker mode, the worker serves the ports and keeps its own counters.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

pub const HISTOGRAM_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

pub const TLS_HISTOGRAM_BUCKETS: [f64; 10] =
    [0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5];

const SAMPLES_LEN: usize = 1024;

static BUCKETS: [AtomicU64; HISTOGRAM_BUCKETS.len()] =
    [const { AtomicU64::new(0) }; HISTOGRAM_BUCKETS.len()];
static DURATION_SUM_MICROS: AtomicU64 = AtomicU64::new(0);
static DURATION_COUNT: AtomicU64 = AtomicU64::new(0);
static SAMPLE_CURSOR: AtomicU64 = AtomicU64::new(0);
static SAMPLES: [AtomicU32; SAMPLES_LEN] = [const { AtomicU32::new(0) }; SAMPLES_LEN];

static TLS_BUCKETS: [AtomicU64; TLS_HISTOGRAM_BUCKETS.len()] =
    [const { AtomicU64::new(0) }; TLS_HISTOGRAM_BUCKETS.len()];
static TLS_DURATION_SUM_MICROS: AtomicU64 = AtomicU64::new(0);
static TLS_DURATION_COUNT: AtomicU64 = AtomicU64::new(0);
static TLS_SAMPLE_CURSOR: AtomicU64 = AtomicU64::new(0);
static TLS_SAMPLES: [AtomicU32; SAMPLES_LEN] = [const { AtomicU32::new(0) }; SAMPLES_LEN];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpstreamErrorKind {
    Timeout,
    Connect,
    CircuitOpen,
    NoRoute,
    Other,
}

static UPSTREAM_ERROR_TIMEOUT: AtomicU64 = AtomicU64::new(0);
static UPSTREAM_ERROR_CONNECT: AtomicU64 = AtomicU64::new(0);
static UPSTREAM_ERROR_CIRCUIT: AtomicU64 = AtomicU64::new(0);
static UPSTREAM_ERROR_NO_ROUTE: AtomicU64 = AtomicU64::new(0);
static UPSTREAM_ERROR_OTHER: AtomicU64 = AtomicU64::new(0);

/// The served requests of one resource, keyed by the resource id of the proxy. A platform app
/// names its proxy after its id, so the collector reads the requests of an app here.
static RESOURCE_REQUESTS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// The proxy counters of this process.
#[derive(Default)]
struct Counters {
    http_requests: AtomicU64,
    h3_requests: AtomicU64,
    rx_bytes: AtomicU64,
    tx_bytes: AtomicU64,
    upstream_errors: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    cache_bypasses: AtomicU64,
    http_status_2xx: AtomicU64,
    http_status_3xx: AtomicU64,
    http_status_4xx: AtomicU64,
    http_status_5xx: AtomicU64,
    tls_handshakes: AtomicU64,
    tls_handshake_errors: AtomicU64,
}

static COUNTERS: Counters = Counters::new();

impl Counters {
    const fn new() -> Self {
        Self {
            http_requests: AtomicU64::new(0),
            h3_requests: AtomicU64::new(0),
            rx_bytes: AtomicU64::new(0),
            tx_bytes: AtomicU64::new(0),
            upstream_errors: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
            cache_bypasses: AtomicU64::new(0),
            http_status_2xx: AtomicU64::new(0),
            http_status_3xx: AtomicU64::new(0),
            http_status_4xx: AtomicU64::new(0),
            http_status_5xx: AtomicU64::new(0),
            tls_handshakes: AtomicU64::new(0),
            tls_handshake_errors: AtomicU64::new(0),
        }
    }
}

/// Counts one served HTTP request on a TCP or TLS port.
pub(crate) fn count_http_request() {
    COUNTERS.http_requests.fetch_add(1, Ordering::Relaxed);
}

/// Counts one served request of the resource with this id, for example the app behind a
/// platform proxy.
pub(crate) fn count_resource_request(resource_id: &str) {
    if let Ok(mut map) = RESOURCE_REQUESTS.lock() {
        *map.entry(resource_id.to_string()).or_insert(0) += 1;
    }
}

/// The served requests per resource id, sorted by the id.
pub(crate) fn snapshot_resources() -> Vec<(String, u64)> {
    RESOURCE_REQUESTS
        .lock()
        .map(|map| map.iter().map(|(id, count)| (id.clone(), *count)).collect())
        .unwrap_or_default()
}

/// Counts one served HTTP/3 request on a QUIC port.
pub(crate) fn count_h3_request() {
    COUNTERS.h3_requests.fetch_add(1, Ordering::Relaxed);
}

/// Counts an upstream failure of a specific kind.
pub(crate) fn count_upstream_error_kind(kind: UpstreamErrorKind) {
    COUNTERS.upstream_errors.fetch_add(1, Ordering::Relaxed);
    match kind {
        UpstreamErrorKind::Timeout => UPSTREAM_ERROR_TIMEOUT.fetch_add(1, Ordering::Relaxed),
        UpstreamErrorKind::Connect => UPSTREAM_ERROR_CONNECT.fetch_add(1, Ordering::Relaxed),
        UpstreamErrorKind::CircuitOpen => UPSTREAM_ERROR_CIRCUIT.fetch_add(1, Ordering::Relaxed),
        UpstreamErrorKind::NoRoute => UPSTREAM_ERROR_NO_ROUTE.fetch_add(1, Ordering::Relaxed),
        UpstreamErrorKind::Other => UPSTREAM_ERROR_OTHER.fetch_add(1, Ordering::Relaxed),
    };
}

/// The upstream errors partitioned by kind.
pub(crate) fn snapshot_upstream_errors() -> Vec<(&'static str, u64)> {
    vec![
        ("timeout", UPSTREAM_ERROR_TIMEOUT.load(Ordering::Relaxed)),
        ("connect", UPSTREAM_ERROR_CONNECT.load(Ordering::Relaxed)),
        (
            "circuit_open",
            UPSTREAM_ERROR_CIRCUIT.load(Ordering::Relaxed),
        ),
        ("no_route", UPSTREAM_ERROR_NO_ROUTE.load(Ordering::Relaxed)),
        ("other", UPSTREAM_ERROR_OTHER.load(Ordering::Relaxed)),
    ]
}

/// Records the observed duration of a served HTTP request.
pub(crate) fn record_request_duration(duration: Duration) {
    let secs = duration.as_secs_f64();
    let micros = duration.as_micros().min(u32::MAX as u128) as u32;

    for (i, &limit) in HISTOGRAM_BUCKETS.iter().enumerate() {
        if secs <= limit {
            BUCKETS[i].fetch_add(1, Ordering::Relaxed);
        }
    }
    DURATION_COUNT.fetch_add(1, Ordering::Relaxed);
    DURATION_SUM_MICROS.fetch_add(micros as u64, Ordering::Relaxed);

    let idx = (SAMPLE_CURSOR.fetch_add(1, Ordering::Relaxed) as usize) % SAMPLES_LEN;
    SAMPLES[idx].store(micros, Ordering::Relaxed);
}

/// The latency histogram buckets and summary: `(buckets, +Inf count, sum seconds, count)`.
pub(crate) fn snapshot_latency_histogram() -> (Vec<(f64, u64)>, u64, f64, u64) {
    let count = DURATION_COUNT.load(Ordering::Relaxed);
    let sum = DURATION_SUM_MICROS.load(Ordering::Relaxed) as f64 / 1_000_000.0;
    let buckets = HISTOGRAM_BUCKETS
        .iter()
        .enumerate()
        .map(|(i, &le)| (le, BUCKETS[i].load(Ordering::Relaxed)))
        .collect();
    (buckets, count, sum, count)
}

/// Returns the estimated p50, p95, and p99 latencies in seconds from the recent samples reservoir.
pub(crate) fn latency_quantiles() -> (f64, f64, f64) {
    let total = DURATION_COUNT.load(Ordering::Relaxed);
    if total == 0 {
        return (0.0, 0.0, 0.0);
    }
    let count = (total as usize).min(SAMPLES_LEN);
    let mut vals: Vec<u32> = SAMPLES[..count]
        .iter()
        .map(|s| s.load(Ordering::Relaxed))
        .collect();
    vals.sort_unstable();
    let p50 = quantile_at(&vals, 50);
    let p95 = quantile_at(&vals, 95);
    let p99 = quantile_at(&vals, 99);
    (p50, p95, p99)
}

fn quantile_at(vals: &[u32], percentile: usize) -> f64 {
    if vals.is_empty() {
        return 0.0;
    }
    let idx = (vals.len() * percentile / 100).min(vals.len() - 1);
    vals[idx] as f64 / 1_000_000.0
}

/// Counts a request that a stored cache response answered.
pub(crate) fn count_cache_hit() {
    COUNTERS.cache_hits.fetch_add(1, Ordering::Relaxed);
}

/// Counts a request that went to the upstream server through the cache.
pub(crate) fn count_cache_miss() {
    COUNTERS.cache_misses.fetch_add(1, Ordering::Relaxed);
}

/// Counts a request that bypassed the cache.
pub(crate) fn count_cache_bypass() {
    COUNTERS.cache_bypasses.fetch_add(1, Ordering::Relaxed);
}

/// Records bytes received from clients.
pub(crate) fn record_rx_bytes(bytes: u64) {
    COUNTERS.rx_bytes.fetch_add(bytes, Ordering::Relaxed);
}

/// Records bytes transmitted to clients.
pub(crate) fn record_tx_bytes(bytes: u64) {
    COUNTERS.tx_bytes.fetch_add(bytes, Ordering::Relaxed);
}

/// Records an answered HTTP status code.
pub(crate) fn record_http_response(status: u16) {
    match status {
        200..=299 => COUNTERS.http_status_2xx.fetch_add(1, Ordering::Relaxed),
        300..=399 => COUNTERS.http_status_3xx.fetch_add(1, Ordering::Relaxed),
        400..=499 => COUNTERS.http_status_4xx.fetch_add(1, Ordering::Relaxed),
        500..=599 => COUNTERS.http_status_5xx.fetch_add(1, Ordering::Relaxed),
        _ => 0,
    };
}

/// Snapshot of HTTP responses partitioned by status class (2xx, 3xx, 4xx, 5xx).
pub(crate) fn snapshot_http_statuses() -> Vec<(&'static str, u64)> {
    vec![
        ("2xx", COUNTERS.http_status_2xx.load(Ordering::Relaxed)),
        ("3xx", COUNTERS.http_status_3xx.load(Ordering::Relaxed)),
        ("4xx", COUNTERS.http_status_4xx.load(Ordering::Relaxed)),
        ("5xx", COUNTERS.http_status_5xx.load(Ordering::Relaxed)),
    ]
}

/// Counts a failed TLS handshake.
pub(crate) fn count_tls_handshake_error() {
    COUNTERS
        .tls_handshake_errors
        .fetch_add(1, Ordering::Relaxed);
}

/// Records the observed duration of a successful TLS handshake.
pub(crate) fn record_tls_handshake(duration: Duration) {
    COUNTERS.tls_handshakes.fetch_add(1, Ordering::Relaxed);
    let secs = duration.as_secs_f64();
    let micros = duration.as_micros().min(u32::MAX as u128) as u32;

    for (i, &limit) in TLS_HISTOGRAM_BUCKETS.iter().enumerate() {
        if secs <= limit {
            TLS_BUCKETS[i].fetch_add(1, Ordering::Relaxed);
        }
    }
    TLS_DURATION_COUNT.fetch_add(1, Ordering::Relaxed);
    TLS_DURATION_SUM_MICROS.fetch_add(micros as u64, Ordering::Relaxed);

    let idx = (TLS_SAMPLE_CURSOR.fetch_add(1, Ordering::Relaxed) as usize) % SAMPLES_LEN;
    TLS_SAMPLES[idx].store(micros, Ordering::Relaxed);
}

/// The TLS handshake latency histogram buckets and summary: `(buckets, +Inf count, sum seconds, count)`.
pub(crate) fn snapshot_tls_handshake_histogram() -> (Vec<(f64, u64)>, u64, f64, u64) {
    let count = TLS_DURATION_COUNT.load(Ordering::Relaxed);
    let sum = TLS_DURATION_SUM_MICROS.load(Ordering::Relaxed) as f64 / 1_000_000.0;
    let buckets = TLS_HISTOGRAM_BUCKETS
        .iter()
        .enumerate()
        .map(|(i, &le)| (le, TLS_BUCKETS[i].load(Ordering::Relaxed)))
        .collect();
    (buckets, count, sum, count)
}

/// Returns the estimated p50, p95, and p99 TLS handshake latencies in seconds.
pub(crate) fn tls_latency_quantiles() -> (f64, f64, f64) {
    let total = TLS_DURATION_COUNT.load(Ordering::Relaxed);
    if total == 0 {
        return (0.0, 0.0, 0.0);
    }
    let count = (total as usize).min(SAMPLES_LEN);
    let mut vals: Vec<u32> = TLS_SAMPLES[..count]
        .iter()
        .map(|s| s.load(Ordering::Relaxed))
        .collect();
    vals.sort_unstable();
    let p50 = quantile_at(&vals, 50);
    let p95 = quantile_at(&vals, 95);
    let p99 = quantile_at(&vals, 99);
    (p50, p95, p99)
}

/// The counters as `name -> value` pairs, in the export order.
pub(crate) fn snapshot() -> Vec<(&'static str, u64)> {
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    vec![
        ("r3v3rs3_http_requests_total", load(&COUNTERS.http_requests)),
        ("r3v3rs3_h3_requests_total", load(&COUNTERS.h3_requests)),
        ("r3v3rs3_rx_bytes_total", load(&COUNTERS.rx_bytes)),
        ("r3v3rs3_tx_bytes_total", load(&COUNTERS.tx_bytes)),
        (
            "r3v3rs3_upstream_errors_total",
            load(&COUNTERS.upstream_errors),
        ),
        ("r3v3rs3_cache_hits_total", load(&COUNTERS.cache_hits)),
        ("r3v3rs3_cache_misses_total", load(&COUNTERS.cache_misses)),
        (
            "r3v3rs3_cache_bypasses_total",
            load(&COUNTERS.cache_bypasses),
        ),
        ("r3v3rs3_open_connections", super::open_connections() as u64),
        (
            "r3v3rs3_tls_handshakes_total",
            load(&COUNTERS.tls_handshakes),
        ),
        (
            "r3v3rs3_tls_handshake_errors_total",
            load(&COUNTERS.tls_handshake_errors),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_requests_accumulate_per_id() {
        let id = format!("test-{:x}", std::process::id());
        count_resource_request(&id);
        count_resource_request(&id);
        count_resource_request("");
        let count = snapshot_resources()
            .into_iter()
            .find(|(key, _)| *key == id)
            .map(|(_, count)| count);
        assert_eq!(count, Some(2));
    }

    #[test]
    fn latency_histogram_and_quantiles_record_values() {
        record_request_duration(Duration::from_millis(10));
        record_request_duration(Duration::from_millis(50));
        record_request_duration(Duration::from_millis(100));

        let (buckets, inf, sum, count) = snapshot_latency_histogram();
        assert!(count >= 3);
        assert!(sum > 0.0);
        assert_eq!(inf, count);
        assert!(!buckets.is_empty());

        let (p50, p95, p99) = latency_quantiles();
        assert!(p50 > 0.0);
        assert!(p95 >= p50);
        assert!(p99 >= p95);
    }

    #[test]
    fn upstream_error_kinds_accumulate() {
        count_upstream_error_kind(UpstreamErrorKind::Timeout);
        count_upstream_error_kind(UpstreamErrorKind::Timeout);
        count_upstream_error_kind(UpstreamErrorKind::Connect);

        let errors = snapshot_upstream_errors();
        let timeout = errors
            .iter()
            .find(|(k, _)| *k == "timeout")
            .map(|(_, v)| *v);
        let connect = errors
            .iter()
            .find(|(k, _)| *k == "connect")
            .map(|(_, v)| *v);
        assert!(timeout.unwrap_or(0) >= 2);
        assert!(connect.unwrap_or(0) >= 1);
    }

    #[test]
    fn http_statuses_and_throughput_accumulate() {
        record_rx_bytes(1024);
        record_tx_bytes(2048);
        record_http_response(200);
        record_http_response(201);
        record_http_response(301);
        record_http_response(404);
        record_http_response(500);
        count_cache_bypass();

        let statuses = snapshot_http_statuses();
        let s2xx = statuses.iter().find(|(k, _)| *k == "2xx").map(|(_, v)| *v);
        let s3xx = statuses.iter().find(|(k, _)| *k == "3xx").map(|(_, v)| *v);
        let s4xx = statuses.iter().find(|(k, _)| *k == "4xx").map(|(_, v)| *v);
        let s5xx = statuses.iter().find(|(k, _)| *k == "5xx").map(|(_, v)| *v);

        assert!(s2xx.unwrap_or(0) >= 2);
        assert!(s3xx.unwrap_or(0) >= 1);
        assert!(s4xx.unwrap_or(0) >= 1);
        assert!(s5xx.unwrap_or(0) >= 1);

        let snap = snapshot();
        let rx = snap
            .iter()
            .find(|(k, _)| *k == "r3v3rs3_rx_bytes_total")
            .map(|(_, v)| *v);
        let tx = snap
            .iter()
            .find(|(k, _)| *k == "r3v3rs3_tx_bytes_total")
            .map(|(_, v)| *v);
        assert!(rx.unwrap_or(0) >= 1024);
        assert!(tx.unwrap_or(0) >= 2048);
    }

    #[test]
    fn tls_handshake_histogram_and_quantiles_record_values() {
        record_tls_handshake(Duration::from_millis(5));
        record_tls_handshake(Duration::from_millis(20));
        record_tls_handshake(Duration::from_millis(50));
        count_tls_handshake_error();

        let (buckets, inf, sum, count) = snapshot_tls_handshake_histogram();
        assert!(count >= 3);
        assert!(sum > 0.0);
        assert_eq!(inf, count);
        assert!(!buckets.is_empty());

        let (p50, p95, p99) = tls_latency_quantiles();
        assert!(p50 > 0.0);
        assert!(p95 >= p50);
        assert!(p99 >= p95);

        let snap = snapshot();
        let handshakes = snap
            .iter()
            .find(|(k, _)| *k == "r3v3rs3_tls_handshakes_total")
            .map(|(_, v)| *v);
        let errors = snap
            .iter()
            .find(|(k, _)| *k == "r3v3rs3_tls_handshake_errors_total")
            .map(|(_, v)| *v);
        assert!(handshakes.unwrap_or(0) >= 3);
        assert!(errors.unwrap_or(0) >= 1);
    }
}
