use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::traffic::{
    AccessLogEntry, AccessLogQuery, CountryTrafficStat, TrafficGeoDistribution,
};
use std::collections::{BTreeMap, VecDeque};
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

/// Returns the aggregated geographical distribution and top countries for entries matching a predicate.
pub fn geo_distribution_matching<F: Fn(Option<&ShortId>) -> bool>(
    filter: F,
) -> TrafficGeoDistribution {
    let Ok(buf) = STORE.buffer.read() else {
        return TrafficGeoDistribution::default();
    };
    let mut country_counts = BTreeMap::new();
    let mut total_resolved = 0u64;

    for entry in buf.iter().filter(|e| filter(e.resource_id.as_ref())) {
        if let Some(country) = &entry.country {
            let code = country.to_ascii_uppercase();
            *country_counts.entry(code).or_insert(0) += 1;
            total_resolved += 1;
        }
    }

    let mut ranked: Vec<(String, u64)> = country_counts
        .iter()
        .map(|(c, count)| (c.clone(), *count))
        .collect();
    ranked.sort_by_key(|a| std::cmp::Reverse(a.1));

    let top_countries = ranked
        .into_iter()
        .take(10)
        .map(|(country, count)| {
            let percentage = if total_resolved > 0 {
                (count as f64 / total_resolved as f64) * 100.0
            } else {
                0.0
            };
            CountryTrafficStat {
                country,
                count,
                percentage: (percentage * 10.0).round() / 10.0,
            }
        })
        .collect();

    TrafficGeoDistribution {
        total_resolved,
        top_countries,
        country_counts,
    }
}

/// Returns the aggregated geographical distribution for all or a specific resource.
pub fn geo_distribution(resource_id: Option<&ShortId>) -> TrafficGeoDistribution {
    geo_distribution_matching(|id| resource_id.is_none_or(|r| id == Some(r)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
            country: None,
        }
    }

    #[test]
    fn record_and_retrieve_recent() {
        let _guard = TEST_LOCK.lock().unwrap();
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

    #[test]
    fn geo_distribution_aggregation() {
        let _guard = TEST_LOCK.lock().unwrap();
        clear();
        let mut e1 = make_test_entry(200, "/1");
        e1.country = Some("TR".to_string());
        let mut e2 = make_test_entry(200, "/2");
        e2.country = Some("TR".to_string());
        let mut e3 = make_test_entry(200, "/3");
        e3.country = Some("US".to_string());
        let mut e4 = make_test_entry(200, "/4");
        e4.country = None;

        record(e1);
        record(e2);
        record(e3);
        record(e4);

        let geo = geo_distribution(None);
        assert_eq!(geo.total_resolved, 3);
        assert_eq!(geo.top_countries.len(), 2);
        assert_eq!(geo.top_countries[0].country, "TR");
        assert_eq!(geo.top_countries[0].count, 2);
        assert_eq!(geo.top_countries[0].percentage, 66.7);
        assert_eq!(geo.top_countries[1].country, "US");
        assert_eq!(geo.top_countries[1].count, 1);
        assert_eq!(geo.top_countries[1].percentage, 33.3);
    }
}
