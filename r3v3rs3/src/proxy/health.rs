use once_cell::sync::Lazy;
use r3v3rs3_api::{
    id::ShortId,
    upstream::{HealthCheck, LoadBalancing},
};
use rand::Rng;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, PoisonError, Weak,
    },
    time::Instant,
};
use tracing::warn;

/// Identifies an upstream group: the proxy, and the route index for an HTTP route.
pub type GroupKey = (ShortId, Option<usize>);

static REGISTRY: Lazy<Mutex<HashMap<GroupKey, Weak<UpstreamGroup>>>> = Lazy::new(Default::default);

/// The upstream servers of a proxy or an HTTP route, with the load balancing policy and the
/// health of each server.
#[derive(Debug)]
pub struct UpstreamGroup {
    addrs: Vec<String>,
    policy: LoadBalancing,
    health_check: HealthCheck,
    servers: Vec<Mutex<ServerHealth>>,
    cursor: AtomicUsize,
}

#[derive(Debug, Default)]
struct ServerHealth {
    failures: u32,
    unhealthy_until: Option<Instant>,
}

impl ServerHealth {
    fn is_healthy(&self, now: Instant) -> bool {
        self.unhealthy_until.is_none_or(|until| now >= until)
    }
}

/// Returns the group for `key`. The existing group is reused while its servers and settings are
/// unchanged, so a configuration reload keeps the health of the servers.
pub fn group(
    key: GroupKey,
    addrs: Vec<String>,
    policy: LoadBalancing,
    health_check: HealthCheck,
) -> Arc<UpstreamGroup> {
    let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    registry.retain(|_, group| group.strong_count() > 0);
    if let Some(existing) = registry.get(&key).and_then(Weak::upgrade) {
        if existing.addrs == addrs
            && existing.policy == policy
            && existing.health_check == health_check
        {
            return existing;
        }
    }
    let group = Arc::new(UpstreamGroup::new(addrs, policy, health_check));
    registry.insert(key, Arc::downgrade(&group));
    group
}

impl UpstreamGroup {
    fn new(addrs: Vec<String>, policy: LoadBalancing, health_check: HealthCheck) -> Self {
        let servers = addrs.iter().map(|_| Mutex::default()).collect();
        Self {
            addrs,
            policy,
            health_check,
            servers,
            cursor: AtomicUsize::new(0),
        }
    }

    /// Returns the server indexes in the order to try: the healthy servers in the order of the
    /// policy, then the unhealthy servers. When every server is unhealthy, the request still goes
    /// to a server.
    pub fn candidates(&self) -> Vec<usize> {
        let len = self.servers.len();
        if len == 0 {
            return Vec::new();
        }
        let start = match self.policy {
            LoadBalancing::RoundRobin => self.cursor.fetch_add(1, Ordering::Relaxed) % len,
            LoadBalancing::Random => rand::thread_rng().gen_range(0..len),
            LoadBalancing::First => 0,
        };
        let now = Instant::now();
        let (mut ordered, unhealthy): (Vec<_>, Vec<_>) = (start..len)
            .chain(0..start)
            .partition(|&index| self.health(index).is_some_and(|h| h.is_healthy(now)));
        ordered.extend(unhealthy);
        ordered
    }

    pub fn report_success(&self, index: usize) {
        if let Some(mut health) = self.health(index) {
            *health = ServerHealth::default();
        }
    }

    /// Records a failed connection or a request without a response. After `max_fails`
    /// consecutive failures, the server is unhealthy for `fail_timeout`.
    pub fn report_failure(&self, index: usize) {
        let max_fails = self.health_check.max_fails;
        let Some(mut health) = self.health(index).filter(|_| max_fails > 0) else {
            return;
        };
        let now = Instant::now();
        health.failures = health.failures.saturating_add(1);
        if health.failures < max_fails {
            return;
        }
        if health.is_healthy(now) {
            let server = self.addrs.get(index).map_or("", String::as_str);
            warn!(
                server,
                failures = health.failures,
                "upstream server is unhealthy"
            );
        }
        health.unhealthy_until = Some(now + self.health_check.fail_timeout);
    }

    fn health(&self, index: usize) -> Option<MutexGuard<'_, ServerHealth>> {
        self.servers
            .get(index)
            .map(|health| health.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn test_group(policy: LoadBalancing, health_check: HealthCheck) -> UpstreamGroup {
        let addrs = ["a", "b", "c"].map(String::from).to_vec();
        UpstreamGroup::new(addrs, policy, health_check)
    }

    #[test]
    fn round_robin_starts_at_the_next_server_each_time() {
        let group = test_group(LoadBalancing::RoundRobin, HealthCheck::default());
        assert_eq!(group.candidates(), [0, 1, 2]);
        assert_eq!(group.candidates(), [1, 2, 0]);
        assert_eq!(group.candidates(), [2, 0, 1]);
        assert_eq!(group.candidates(), [0, 1, 2]);
    }

    #[test]
    fn first_and_random_policies_return_every_server() {
        let group = test_group(LoadBalancing::First, HealthCheck::default());
        assert_eq!(group.candidates(), [0, 1, 2]);
        assert_eq!(group.candidates(), [0, 1, 2]);

        let group = test_group(LoadBalancing::Random, HealthCheck::default());
        for _ in 0..20 {
            let mut candidates = group.candidates();
            candidates.sort_unstable();
            assert_eq!(candidates, [0, 1, 2]);
        }
    }

    #[test]
    fn unhealthy_servers_move_behind_the_healthy_servers() {
        let health_check = HealthCheck {
            max_fails: 2,
            fail_timeout: Duration::from_millis(50),
        };
        let group = test_group(LoadBalancing::First, health_check);
        group.report_failure(0);
        assert_eq!(group.candidates(), [0, 1, 2]);
        group.report_failure(0);
        assert_eq!(group.candidates(), [1, 2, 0]);

        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(group.candidates(), [0, 1, 2]);
        // The failure count stays until a success, so one more failure marks the server again.
        group.report_failure(0);
        assert_eq!(group.candidates(), [1, 2, 0]);

        group.report_success(0);
        assert_eq!(group.candidates(), [0, 1, 2]);
    }

    #[test]
    fn zero_max_fails_disables_the_passive_health_check() {
        let health_check = HealthCheck {
            max_fails: 0,
            ..Default::default()
        };
        let group = test_group(LoadBalancing::First, health_check);
        group.report_failure(0);
        group.report_failure(0);
        assert_eq!(group.candidates(), [0, 1, 2]);
    }

    #[test]
    fn registry_reuses_a_group_with_the_same_servers_and_settings() {
        let key = ("group".parse().unwrap(), Some(0));
        let addrs = || vec!["a".to_string(), "b".to_string()];
        let first = group(key, addrs(), LoadBalancing::First, HealthCheck::default());
        let same = group(key, addrs(), LoadBalancing::First, HealthCheck::default());
        assert!(Arc::ptr_eq(&first, &same));

        let changed = group(key, addrs(), LoadBalancing::Random, HealthCheck::default());
        assert!(!Arc::ptr_eq(&first, &changed));
    }
}
