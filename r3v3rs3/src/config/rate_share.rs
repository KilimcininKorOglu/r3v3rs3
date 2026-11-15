//! Rate limit counts that the nodes of a cluster share. Each node counts the requests of each
//! client in fixed windows of the limit period and publishes the largest counts at an interval.
//! A node adds the counts of the other nodes to its own counts, so a client cannot use the full
//! limit on each node. The counts of the other nodes are one interval old, so a limit with a
//! short period divides its quota between the nodes instead.

use crate::clock::unix_ms;
use serde_derive::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

/// The most client counts that a node publishes.
pub const MAX_PUBLISHED_COUNTS: usize = 2048;
/// A limit with a period shorter than this number of intervals divides its quota between the nodes.
const SHARED_PERIOD_INTERVALS: u32 = 10;
/// The shortest interval. A zero interval would publish without a pause.
const MIN_INTERVAL: Duration = Duration::from_millis(100);
/// The counts of a node that did not publish for this time do not count.
const MIN_FRESHNESS: Duration = Duration::from_secs(2);

/// The requests of a client in the current and the previous window of the limit period.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowCount {
    /// The Unix time in milliseconds divided by the period.
    pub window: u64,
    pub current: u32,
    pub previous: u32,
}

impl WindowCount {
    /// The count at `window`. The counts of older windows are zero. A count of a later window,
    /// from a node with a clock that is ahead, stays as it is.
    fn at(self, window: u64) -> Self {
        match window.checked_sub(self.window) {
            Some(0) | None => Self { window, ..self },
            Some(1) => Self {
                window,
                current: 0,
                previous: self.current,
            },
            Some(_) => Self {
                window,
                ..Self::default()
            },
        }
    }

    /// The sliding window estimate of the requests in the last period. `elapsed` is the part of
    /// the current window that passed.
    fn estimate(self, window: u64, elapsed: f64) -> f64 {
        let count = self.at(window);
        f64::from(count.previous) * (1.0 - elapsed) + f64::from(count.current)
    }

    fn total(self) -> u64 {
        u64::from(self.current) + u64::from(self.previous)
    }

    fn add(&mut self, other: Self) {
        let other = other.at(self.window);
        self.current = self.current.saturating_add(other.current);
        self.previous = self.previous.saturating_add(other.previous);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCount {
    pub ip: IpAddr,
    #[serde(flatten)]
    pub count: WindowCount,
}

/// The counts of one limiter of a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimiterCounts {
    /// The proxy and the route of the limiter.
    pub key: String,
    pub period_ms: u64,
    pub counts: Vec<ClientCount>,
}

/// The counts that a node publishes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeCounts {
    /// The Unix time in milliseconds.
    pub published_at: u64,
    pub limiters: Vec<LimiterCounts>,
}

/// The counts of the other nodes, and the number of nodes with this node.
#[derive(Debug, Clone, Default)]
pub struct RemoteCounts {
    pub nodes: usize,
    pub counts: Vec<NodeCounts>,
}

#[async_trait::async_trait]
pub trait RateCountExchange: Send + Sync {
    /// Publishes the counts of this node and reads the counts of the other nodes.
    async fn exchange(&self, local: &NodeCounts) -> anyhow::Result<RemoteCounts>;
}

/// The state of the exchange that the limiters of a server share.
#[derive(Debug)]
pub struct RateShare {
    interval: Duration,
    nodes: AtomicUsize,
    connected: AtomicBool,
}

impl RateShare {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval: interval.max(MIN_INTERVAL),
            nodes: AtomicUsize::new(1),
            connected: AtomicBool::new(false),
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Records the result of an exchange. A failed exchange keeps the last number of nodes.
    pub fn set_state(&self, nodes: Option<usize>, connected: bool) {
        if let Some(nodes) = nodes {
            self.nodes.store(nodes.max(1), Ordering::Relaxed);
        }
        self.connected.store(connected, Ordering::Relaxed);
    }

    /// The oldest publication time in milliseconds that still counts.
    pub fn oldest_fresh(&self, now_ms: u64) -> u64 {
        let freshness = (self.interval * 3).max(MIN_FRESHNESS);
        now_ms.saturating_sub(freshness.as_millis() as u64)
    }

    /// The requests that this node allows in one period, and whether the counts of the other
    /// nodes count toward them. Without the store, the node divides the quota.
    fn quota(&self, limit: u32, period: Duration) -> (u32, bool) {
        let connected = self.connected.load(Ordering::Relaxed);
        if connected && period >= self.interval * SHARED_PERIOD_INTERVALS {
            return (limit, true);
        }
        let nodes = u32::try_from(self.nodes.load(Ordering::Relaxed)).unwrap_or(u32::MAX);
        (limit.div_ceil(nodes.max(1)), false)
    }
}

/// The counts of one limiter on this node and on the other nodes.
#[derive(Debug)]
pub struct SharedWindow {
    share: Arc<RateShare>,
    limit: u32,
    period_ms: u64,
    local: Mutex<HashMap<IpAddr, WindowCount>>,
    remote: Mutex<HashMap<IpAddr, WindowCount>>,
}

impl SharedWindow {
    pub fn new(share: Arc<RateShare>, limit: u32, period: Duration) -> Self {
        Self {
            share,
            limit,
            period_ms: (period.as_millis() as u64).max(1),
            local: Mutex::default(),
            remote: Mutex::default(),
        }
    }

    /// Counts a request of the client. Returns the time to wait when the client used its quota.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        let (window, elapsed) = self.position(unix_ms());
        self.check_at(ip, window, elapsed)
    }

    fn check_at(&self, ip: IpAddr, window: u64, elapsed: f64) -> Result<(), Duration> {
        let (quota, shared) = self.share.quota(self.limit, self.period());
        let remote = if shared {
            lock(&self.remote)
                .get(&ip)
                .map_or(0.0, |count| count.estimate(window, elapsed))
        } else {
            0.0
        };
        let mut local = lock(&self.local);
        let count = local.entry(ip).or_default();
        *count = count.at(window);
        if count.estimate(window, elapsed) + remote >= f64::from(quota) {
            return Err(self.period().mul_f64(1.0 - elapsed));
        }
        count.current = count.current.saturating_add(1);
        Ok(())
    }

    fn period(&self) -> Duration {
        Duration::from_millis(self.period_ms)
    }

    /// The window index and the part of the window that passed at `now_ms`.
    fn position(&self, now_ms: u64) -> (u64, f64) {
        let elapsed = (now_ms % self.period_ms) as f64 / self.period_ms as f64;
        (now_ms / self.period_ms, elapsed)
    }

    /// The counts of this node. Counts older than the previous window are removed.
    fn local_counts(&self, window: u64) -> Vec<ClientCount> {
        let mut local = lock(&self.local);
        local.retain(|_, count| {
            *count = count.at(window);
            count.total() > 0
        });
        local
            .iter()
            .map(|(ip, count)| ClientCount {
                ip: *ip,
                count: *count,
            })
            .collect()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The counts that this node publishes: the largest counts of all limiters.
pub fn collect(windows: &[(String, &SharedWindow)], now_ms: u64) -> NodeCounts {
    let mut all: Vec<(usize, ClientCount)> = windows
        .iter()
        .enumerate()
        .flat_map(|(index, (_, window))| {
            let (current, _) = window.position(now_ms);
            let counts = window.local_counts(current);
            counts.into_iter().map(move |count| (index, count))
        })
        .collect();
    all.sort_unstable_by_key(|(_, client)| Reverse(client.count.total()));
    all.truncate(MAX_PUBLISHED_COUNTS);

    let mut limiters: Vec<LimiterCounts> = windows
        .iter()
        .map(|(key, window)| LimiterCounts {
            key: key.clone(),
            period_ms: window.period_ms,
            counts: Vec::new(),
        })
        .collect();
    for (index, client) in all {
        if let Some(limiter) = limiters.get_mut(index) {
            limiter.counts.push(client);
        }
    }
    limiters.retain(|limiter| !limiter.counts.is_empty());
    NodeCounts {
        published_at: now_ms,
        limiters,
    }
}

/// Replaces the remote counts of each limiter with the sum of the fresh counts of the other nodes.
pub fn apply(windows: &[(String, &SharedWindow)], remote: &[NodeCounts], oldest: u64, now_ms: u64) {
    for (key, window) in windows {
        let (current, _) = window.position(now_ms);
        let mut merged: HashMap<IpAddr, WindowCount> = HashMap::new();
        let limiters = remote
            .iter()
            .filter(|node| node.published_at >= oldest)
            .flat_map(|node| &node.limiters)
            .filter(|limiter| limiter.key == *key && limiter.period_ms == window.period_ms);
        for client in limiters.flat_map(|limiter| &limiter.counts) {
            let empty = WindowCount {
                window: current,
                ..WindowCount::default()
            };
            merged.entry(client.ip).or_insert(empty).add(client.count);
        }
        *lock(&window.remote) = merged;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    fn connected_share(nodes: usize) -> Arc<RateShare> {
        let share = Arc::new(RateShare::new(Duration::from_secs(1)));
        share.set_state(Some(nodes), true);
        share
    }

    #[test]
    fn a_count_moves_to_later_windows() {
        let count = WindowCount {
            window: 10,
            current: 4,
            previous: 2,
        };
        assert_eq!(count.at(10), count);
        assert_eq!(count.at(9), WindowCount { window: 9, ..count });
        let next = count.at(11);
        assert_eq!((next.current, next.previous), (0, 4));
        assert_eq!(count.at(12).total(), 0);
        assert_eq!(count.estimate(11, 0.25), 3.0);
    }

    #[test]
    fn the_counts_of_other_nodes_use_the_same_quota() {
        let window = SharedWindow::new(connected_share(2), 3, MINUTE);
        let client = ip("203.0.113.1");
        assert!(window.check_at(client, 5, 0.5).is_ok());
        let remote = NodeCounts {
            published_at: 1000,
            limiters: vec![LimiterCounts {
                key: "web/-".into(),
                period_ms: 60_000,
                counts: vec![ClientCount {
                    ip: client,
                    count: WindowCount {
                        window: 5,
                        current: 2,
                        previous: 0,
                    },
                }],
            }],
        };
        let now = 5 * 60_000 + 30_000;
        apply(
            &[("web/-".into(), &window)],
            std::slice::from_ref(&remote),
            1000,
            now,
        );

        let wait = window.check_at(client, 5, 0.5).unwrap_err();
        assert_eq!(wait, Duration::from_secs(30));
        assert!(window.check_at(ip("203.0.113.2"), 5, 0.5).is_ok());

        // Stale counts of another node do not count.
        apply(&[("web/-".into(), &window)], &[remote], 1001, now);
        assert!(window.check_at(client, 5, 0.5).is_ok());
    }

    #[test]
    fn a_short_period_divides_the_quota_between_the_nodes() {
        let window = SharedWindow::new(connected_share(2), 4, Duration::from_secs(1));
        let client = ip("203.0.113.1");
        assert!(window.check_at(client, 7, 0.0).is_ok());
        assert!(window.check_at(client, 7, 0.0).is_ok());
        assert!(window.check_at(client, 7, 0.0).is_err());
    }

    #[test]
    fn a_node_without_the_store_divides_the_quota() {
        let share = connected_share(3);
        share.set_state(None, false);
        let window = SharedWindow::new(share, 3, MINUTE);
        let client = ip("203.0.113.1");
        assert!(window.check_at(client, 1, 0.0).is_ok());
        assert!(window.check_at(client, 1, 0.0).is_err());
    }

    #[test]
    fn a_node_publishes_its_largest_counts() {
        let share = connected_share(1);
        let first = SharedWindow::new(share.clone(), u32::MAX, MINUTE);
        let second = SharedWindow::new(share, u32::MAX, MINUTE);
        for index in 0..MAX_PUBLISHED_COUNTS {
            let client = IpAddr::from([10, 0, (index >> 8) as u8, index as u8]);
            first.check_at(client, 0, 0.0).unwrap();
        }
        for _ in 0..2 {
            second.check_at(ip("203.0.113.9"), 0, 0.0).unwrap();
        }
        second.check_at(ip("203.0.113.8"), 0, 0.0).unwrap();
        let windows = [("a".to_string(), &first), ("b".to_string(), &second)];

        let counts = collect(&windows, 30_000);
        let published: usize = counts.limiters.iter().map(|l| l.counts.len()).sum();
        assert_eq!(published, MAX_PUBLISHED_COUNTS);
        let second_counts = &counts.limiters[1].counts;
        assert_eq!(second_counts[0].ip, ip("203.0.113.9"));
        assert_eq!(counts.limiters[1].key, "b");

        // The counts of a window that ended two periods ago are not published.
        let later = collect(&windows, 3 * 60_000);
        assert!(later.limiters.is_empty());
    }
}
