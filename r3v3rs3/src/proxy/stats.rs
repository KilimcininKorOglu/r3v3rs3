//! The process-wide proxy counters that the admin metrics endpoint exports. The counters count
//! one process: with worker mode, the worker serves the ports and keeps its own counters.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

pub const HISTOGRAM_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

const SAMPLES_LEN: usize = 1024;

static BUCKETS: [AtomicU64; HISTOGRAM_BUCKETS.len()] =
    [const { AtomicU64::new(0) }; HISTOGRAM_BUCKETS.len()];
static DURATION_SUM_MICROS: AtomicU64 = AtomicU64::new(0);
static DURATION_COUNT: AtomicU64 = AtomicU64::new(0);
static SAMPLE_CURSOR: AtomicU64 = AtomicU64::new(0);
static SAMPLES: [AtomicU32; SAMPLES_LEN] = [const { AtomicU32::new(0) }; SAMPLES_LEN];

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
    upstream_errors: AtomicU64,
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
}

static COUNTERS: Counters = Counters::new();

impl Counters {
    const fn new() -> Self {
        Self {
            http_requests: AtomicU64::new(0),
            h3_requests: AtomicU64::new(0),
            upstream_errors: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
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

/// The counters as `name -> value` pairs, in the export order.
pub(crate) fn snapshot() -> Vec<(&'static str, u64)> {
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    vec![
        ("r3v3rs3_http_requests_total", load(&COUNTERS.http_requests)),
        ("r3v3rs3_h3_requests_total", load(&COUNTERS.h3_requests)),
        (
            "r3v3rs3_upstream_errors_total",
            load(&COUNTERS.upstream_errors),
        ),
        ("r3v3rs3_cache_hits_total", load(&COUNTERS.cache_hits)),
        ("r3v3rs3_cache_misses_total", load(&COUNTERS.cache_misses)),
        ("r3v3rs3_open_connections", super::open_connections() as u64),
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
}
