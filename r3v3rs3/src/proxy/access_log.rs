use r3v3rs3_api::traffic::{AccessLogEntry, AccessLogQuery};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, RwLock};
use tokio::sync::broadcast;

const DEFAULT_CAPACITY: usize = 1000;
const DEFAULT_LIMIT: usize = 100;
const MAX_LIMIT: usize = 1000;
const CHANNEL_CAPACITY: usize = 1024;

static NEXT_LOG_ID: AtomicU64 = AtomicU64::new(1);

struct AccessLogStore {
    buffer: RwLock<VecDeque<AccessLogEntry>>,
    sender: broadcast::Sender<AccessLogEntry>,
}

static STORE: LazyLock<AccessLogStore> = LazyLock::new(|| {
    let (sender, _) = broadcast::channel(CHANNEL_CAPACITY);
    AccessLogStore {
        buffer: RwLock::new(VecDeque::with_capacity(DEFAULT_CAPACITY)),
        sender,
    }
});

/// Records an access log entry into the in-memory ring buffer and broadcasts it to live subscribers.
pub fn record(mut entry: AccessLogEntry) {
    entry.id = NEXT_LOG_ID.fetch_add(1, Ordering::Relaxed);
    if let Ok(mut buf) = STORE.buffer.write() {
        if buf.len() >= DEFAULT_CAPACITY {
            buf.pop_front();
        }
        buf.push_back(entry.clone());
    }
    let _ = STORE.sender.send(entry);
}

/// Retrieves recent access log entries matching the given query, newest first.
pub fn recent(query: &AccessLogQuery) -> Vec<AccessLogEntry> {
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let Ok(buf) = STORE.buffer.read() else {
        return Vec::new();
    };
    buf.iter()
        .rev()
        .filter(|entry| entry.matches(query))
        .take(limit)
        .cloned()
        .collect()
}

/// Subscribes to real-time broadcast of incoming access log entries.
pub fn subscribe() -> broadcast::Receiver<AccessLogEntry> {
    STORE.sender.subscribe()
}

/// Clears the in-memory ring buffer.
pub fn clear() {
    if let Ok(mut buf) = STORE.buffer.write() {
        buf.clear();
    }
}

/// Returns the number of currently buffered entries.
pub fn count() -> usize {
    STORE.buffer.read().map(|b| b.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn make_test_entry(status: u16, uri: &str) -> AccessLogEntry {
        AccessLogEntry {
            id: 0,
            timestamp: OffsetDateTime::now_utc(),
            resource_id: Some("proxy1".parse().unwrap()),
            client_ip: "10.0.0.1".to_string(),
            method: "GET".to_string(),
            host: "localhost".to_string(),
            uri: uri.to_string(),
            status,
            latency_ms: 5.0,
            proto: "HTTP/1.1".to_string(),
            error: None,
        }
    }

    #[test]
    fn record_and_retrieve_recent() {
        clear();
        record(make_test_entry(200, "/page1"));
        record(make_test_entry(404, "/not-found"));
        record(make_test_entry(500, "/error"));

        let all = recent(&AccessLogQuery::default());
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].status, 500); // newest first

        let query_404 = AccessLogQuery {
            status: Some(404),
            ..Default::default()
        };
        let res_404 = recent(&query_404);
        assert_eq!(res_404.len(), 1);
        assert_eq!(res_404[0].uri, "/not-found");
    }
}
