use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use utoipa::ToSchema;

/// Header name specified in RFC 8470 for early data indication.
pub const EARLY_DATA_HEADER: &str = "early-data";

/// Header value for `Early-Data` header ("1").
pub const EARLY_DATA_VALUE: &str = "1";

/// Default lifetime for 0-RTT anti-replay cache entries.
pub const DEFAULT_ANTI_REPLAY_WINDOW: Duration = Duration::from_secs(60);

/// Maximum entries in the anti-replay buffer to prevent unbounded memory growth.
pub const MAX_ANTI_REPLAY_ENTRIES: usize = 10_000;

/// Global anti-replay filter instance.
pub static GLOBAL_ANTI_REPLAY: LazyLock<AntiReplayBuffer> =
    LazyLock::new(|| AntiReplayBuffer::new(DEFAULT_ANTI_REPLAY_WINDOW, MAX_ANTI_REPLAY_ENTRIES));

/// Checks if an HTTP method is considered safe per RFC 7231 / RFC 9110 (Section 9.2.1).
/// Safe methods are idempotent and suitable for 0-RTT early data.
#[must_use]
pub fn is_safe_http_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS" | "TRACE"
    )
}

/// Key identifying a request in the anti-replay cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ReplayKey {
    client_ip: IpAddr,
    method: String,
    path_and_query: String,
}

/// An anti-replay cache tracking seen 0-RTT request identifiers within a time window.
#[derive(Debug)]
pub struct AntiReplayBuffer {
    window: Duration,
    max_entries: usize,
    entries: Mutex<HashMap<ReplayKey, Instant>>,
}

impl AntiReplayBuffer {
    /// Creates a new `AntiReplayBuffer` with the specified window and maximum capacity.
    #[must_use]
    pub fn new(window: Duration, max_entries: usize) -> Self {
        Self {
            window,
            max_entries,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Checks if a 0-RTT request has already been seen in the current window.
    /// Returns `true` if the request is fresh (allowed), or `false` if it is a replay (rejected).
    pub fn check_and_record(&self, client_ip: IpAddr, method: &str, path_and_query: &str) -> bool {
        let key = ReplayKey {
            client_ip,
            method: method.to_ascii_uppercase(),
            path_and_query: path_and_query.to_string(),
        };

        let now = Instant::now();
        let mut map = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        self.evict_expired_or_overflow(&mut map, now);

        if map
            .get(&key)
            .is_some_and(|&seen_at| now.duration_since(seen_at) < self.window)
        {
            return false;
        }

        map.insert(key, now);
        true
    }

    /// Prunes expired entries and caps size to `max_entries`.
    fn evict_expired_or_overflow(&self, map: &mut HashMap<ReplayKey, Instant>, now: Instant) {
        if map.len() > self.max_entries / 2 {
            let window = self.window;
            map.retain(|_, &mut seen_at| now.duration_since(seen_at) < window);
        }
        if map.len() >= self.max_entries {
            map.clear();
        }
    }

    /// Returns the current number of tracked entries.
    #[must_use]
    pub fn len(&self) -> usize {
        match self.entries.lock() {
            Ok(guard) => guard.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    /// Returns `true` if the buffer is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clears all entries from the buffer.
    pub fn clear(&self) {
        let mut map = match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        map.clear();
    }
}

/// Request to configure early data settings on a port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetPortEarlyDataRequest {
    /// Whether TLS 1.3 0-RTT early data is enabled on this port.
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_is_safe_http_method() {
        assert!(is_safe_http_method("GET"));
        assert!(is_safe_http_method("get"));
        assert!(is_safe_http_method("HEAD"));
        assert!(is_safe_http_method("OPTIONS"));
        assert!(is_safe_http_method("TRACE"));

        assert!(!is_safe_http_method("POST"));
        assert!(!is_safe_http_method("PUT"));
        assert!(!is_safe_http_method("DELETE"));
        assert!(!is_safe_http_method("PATCH"));
        assert!(!is_safe_http_method("CONNECT"));
    }

    #[test]
    fn test_anti_replay_buffer_records_and_detects_replay() {
        let buffer = AntiReplayBuffer::new(Duration::from_secs(10), 100);
        let ip: IpAddr = Ipv4Addr::new(192, 168, 1, 10).into();

        // First request is fresh
        assert!(buffer.check_and_record(ip, "GET", "/api/data"));

        // Second duplicate request within window is a replay
        assert!(!buffer.check_and_record(ip, "GET", "/api/data"));

        // Different path is fresh
        assert!(buffer.check_and_record(ip, "GET", "/api/other"));

        // Different IP is fresh
        let other_ip: IpAddr = Ipv4Addr::new(192, 168, 1, 11).into();
        assert!(buffer.check_and_record(other_ip, "GET", "/api/data"));
    }

    #[test]
    fn test_anti_replay_buffer_expiration() {
        let buffer = AntiReplayBuffer::new(Duration::from_millis(50), 100);
        let ip: IpAddr = Ipv4Addr::new(10, 0, 0, 1).into();

        assert!(buffer.check_and_record(ip, "GET", "/status"));
        assert!(!buffer.check_and_record(ip, "GET", "/status"));

        std::thread::sleep(Duration::from_millis(60));

        // After window expires, request should be accepted again
        assert!(buffer.check_and_record(ip, "GET", "/status"));
    }
}
