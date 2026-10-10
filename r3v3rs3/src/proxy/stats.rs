//! The process-wide proxy counters that the admin metrics endpoint exports. The counters count
//! one process: with worker mode, the worker serves the ports and keeps its own counters.

use std::sync::atomic::{AtomicU64, Ordering};

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

/// Counts one served HTTP/3 request on a QUIC port.
pub(crate) fn count_h3_request() {
    COUNTERS.h3_requests.fetch_add(1, Ordering::Relaxed);
}

/// Counts a request that r3v3rs3 answered without a successful upstream response, for example a
/// proxy error or a refused upstream connection.
pub(crate) fn count_upstream_error() {
    COUNTERS.upstream_errors.fetch_add(1, Ordering::Relaxed);
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
