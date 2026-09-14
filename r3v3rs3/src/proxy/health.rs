use crate::proxy::http::pool::ConnectionPool;
use hyper::Uri;
use once_cell::sync::Lazy;
use r3v3rs3_api::{
    id::ShortId,
    port::UpstreamServer,
    upstream::{HealthCheck, LoadBalancing, UpstreamHealth},
};
use rand::Rng;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::{Duration, Instant},
};
use tokio::{net::TcpStream, task::JoinSet, time::MissedTickBehavior};
use tracing::{info, warn};

/// Identifies an upstream group: the proxy, and the route index for an HTTP route.
pub type GroupKey = (ShortId, Option<usize>);

static REGISTRY: Lazy<Mutex<HashMap<GroupKey, Weak<UpstreamGroup>>>> = Lazy::new(Default::default);

/// An upstream server of a group: its URL or address, and its weight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupServer {
    pub addr: String,
    pub weight: u16,
}

/// The upstream servers of a proxy or an HTTP route, with the load balancing policy and the
/// health of each server.
#[derive(Debug)]
pub struct UpstreamGroup {
    members: Vec<GroupServer>,
    policy: LoadBalancing,
    health_check: HealthCheck,
    servers: Vec<Mutex<ServerHealth>>,
    /// The current weight of each server for the smooth weighted round robin.
    current_weights: Mutex<Vec<i64>>,
    probe: Mutex<Option<Arc<Probe>>>,
}

#[derive(Debug, Default)]
struct ServerHealth {
    failures: u32,
    unhealthy_until: Option<Instant>,
    /// True after a failed active check, until the next successful active check.
    check_failed: bool,
    last_error: Option<String>,
}

impl ServerHealth {
    fn is_healthy(&self, now: Instant) -> bool {
        !self.check_failed && self.unhealthy_until.is_none_or(|until| now >= until)
    }
}

/// How the active health check reaches each server of a group. The targets have the order of the
/// servers, and `None` marks a server that the check cannot reach.
#[derive(Debug)]
pub enum Probe {
    /// Opens a TCP connection to the host and the port.
    Connect(Vec<Option<(String, u16)>>),
    /// Resolves the host name.
    Resolve(Vec<Option<(String, u16)>>),
    /// Sends `GET` to the URI through the connection pool of the proxy.
    Http {
        uris: Vec<Option<Uri>>,
        pool: Arc<ConnectionPool>,
    },
}

impl Probe {
    /// Returns the host and the port of each server address.
    pub fn targets(servers: &[UpstreamServer]) -> Vec<Option<(String, u16)>> {
        servers
            .iter()
            .map(|server| Some((server.addr.host().ok()?, server.addr.port().ok()?)))
            .collect()
    }

    async fn check(&self, index: usize, timeout: Duration) -> Result<(), String> {
        tokio::time::timeout(timeout, self.run(index))
            .await
            .unwrap_or_else(|_| Err(format!("the active check timed out after {timeout:?}")))
    }

    async fn run(&self, index: usize) -> Result<(), String> {
        match self {
            Self::Connect(targets) => {
                let (host, port) = target(targets, index)?;
                TcpStream::connect((host.as_str(), *port))
                    .await
                    .map(drop)
                    .map_err(|err| err.to_string())
            }
            Self::Resolve(targets) => {
                let (host, port) = target(targets, index)?;
                let mut addrs = tokio::net::lookup_host((host.as_str(), *port))
                    .await
                    .map_err(|err| err.to_string())?;
                addrs
                    .next()
                    .map(drop)
                    .ok_or_else(|| format!("no IP address found for {host}"))
            }
            Self::Http { uris, pool } => {
                let uri = target(uris, index)?;
                let status = pool
                    .probe(uri.clone())
                    .await
                    .map_err(|err| err.to_string())?;
                if status.is_success() || status.is_redirection() {
                    Ok(())
                } else {
                    Err(format!("the active check received status {status}"))
                }
            }
        }
    }
}

/// The addresses and the weights of the servers of a TCP or UDP proxy.
pub fn members(servers: &[UpstreamServer]) -> Vec<GroupServer> {
    servers
        .iter()
        .map(|server| GroupServer {
            addr: server.addr.to_string(),
            weight: server.weight,
        })
        .collect()
}

fn target<T>(targets: &[Option<T>], index: usize) -> Result<&T, String> {
    targets
        .get(index)
        .and_then(Option::as_ref)
        .ok_or_else(|| "the server address has no host or port".to_string())
}

/// Returns the group for `key`. The existing group is reused while its servers, their weights and
/// the settings are unchanged, so a configuration reload keeps the health of the servers. A new
/// group with an active health check starts its check task.
pub fn group(
    key: GroupKey,
    members: Vec<GroupServer>,
    policy: LoadBalancing,
    health_check: HealthCheck,
    probe: Probe,
) -> Arc<UpstreamGroup> {
    let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    registry.retain(|_, group| group.strong_count() > 0);
    if let Some(existing) = registry.get(&key).and_then(Weak::upgrade) {
        if existing.members == members
            && existing.policy == policy
            && existing.health_check == health_check
        {
            existing.set_probe(probe);
            return existing;
        }
    }
    let group = Arc::new(UpstreamGroup::new(members, policy, health_check));
    group.set_probe(probe);
    registry.insert(key, Arc::downgrade(&group));
    spawn_checker(&group);
    group
}

/// Returns the health of the upstream servers of a proxy. An HTTP proxy lists the servers of each
/// route in route order.
pub fn snapshot(id: ShortId) -> Vec<UpstreamHealth> {
    let mut groups = REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .filter(|((proxy, _), _)| *proxy == id)
        .filter_map(|((_, route), group)| Some((*route, group.upgrade()?)))
        .collect::<Vec<_>>();
    groups.sort_by_key(|(route, _)| *route);
    let now = Instant::now();
    groups
        .iter()
        .flat_map(|(_, group)| group.upstream_health(now))
        .collect()
}

/// Checks the servers of the group every interval until the group is dropped.
fn spawn_checker(group: &Arc<UpstreamGroup>) {
    if !group.health_check.is_active() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let weak = Arc::downgrade(group);
    let interval = group.health_check.interval;
    handle.spawn(async move {
        let mut ticks = tokio::time::interval(interval);
        ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            let Some(group) = weak.upgrade() else {
                break;
            };
            group.check().await;
        }
    });
}

impl UpstreamGroup {
    fn new(members: Vec<GroupServer>, policy: LoadBalancing, health_check: HealthCheck) -> Self {
        let servers = members.iter().map(|_| Mutex::default()).collect();
        let current_weights = Mutex::new(vec![0; members.len()]);
        Self {
            members,
            policy,
            health_check,
            servers,
            current_weights,
            probe: Mutex::new(None),
        }
    }

    fn set_probe(&self, probe: Probe) {
        *self.probe.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(probe));
    }

    /// Returns the server indexes in the order to try: the healthy servers in the order of the
    /// policy, then the unhealthy servers. When every server is unhealthy, the unhealthy servers
    /// use the order of the policy, so the request still goes to a server. A server with weight 0
    /// is never a candidate.
    pub fn candidates(&self) -> Vec<usize> {
        let now = Instant::now();
        let (healthy, unhealthy): (Vec<_>, Vec<_>) = (0..self.members.len())
            .filter(|&index| self.weight(index) > 0)
            .partition(|&index| self.health(index).is_some_and(|h| h.is_healthy(now)));
        if healthy.is_empty() {
            return self.order(unhealthy);
        }
        let mut ordered = self.order(healthy);
        ordered.extend(unhealthy);
        ordered
    }

    fn order(&self, servers: Vec<usize>) -> Vec<usize> {
        match self.policy {
            LoadBalancing::RoundRobin => self.order_round_robin(servers),
            LoadBalancing::Random => self.order_random(servers),
            LoadBalancing::First => servers,
        }
    }

    /// The smooth weighted round robin of nginx: each server gains its weight, and the server
    /// with the highest current weight goes first and loses the total weight. The other servers
    /// follow it in list order.
    fn order_round_robin(&self, mut servers: Vec<usize>) -> Vec<usize> {
        let mut current = self
            .current_weights
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let total = servers
            .iter()
            .map(|&index| i64::from(self.weight(index)))
            .sum::<i64>();
        let mut best: Option<(usize, i64)> = None;
        for (position, &index) in servers.iter().enumerate() {
            let Some(value) = current.get_mut(index) else {
                continue;
            };
            *value += i64::from(self.weight(index));
            if best.is_none_or(|(_, top)| *value > top) {
                best = Some((position, *value));
            }
        }
        let Some((position, _)) = best else {
            return servers;
        };
        if let Some(value) = servers.get(position).and_then(|&i| current.get_mut(i)) {
            *value -= total;
        }
        servers.rotate_left(position);
        servers
    }

    /// The weighted random order of Efraimidis and Spirakis: each server gets the key
    /// `u^(1/weight)` for a random `u` in `[0, 1)`, and a higher key goes first.
    fn order_random(&self, servers: Vec<usize>) -> Vec<usize> {
        let mut rng = rand::thread_rng();
        let mut keyed = servers
            .into_iter()
            .map(|index| {
                let key = rng.gen::<f64>().powf(1.0 / f64::from(self.weight(index)));
                (key, index)
            })
            .collect::<Vec<_>>();
        keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
        keyed.into_iter().map(|(_, index)| index).collect()
    }

    /// Records a successful connection or request. A failed active check stays until the next
    /// successful active check.
    pub fn report_success(&self, index: usize) {
        if let Some(mut health) = self.health(index) {
            health.failures = 0;
            health.unhealthy_until = None;
            if !health.check_failed {
                health.last_error = None;
            }
        }
    }

    /// Records a failed connection or a request without a response. After `max_fails`
    /// consecutive failures, the server is unhealthy for `fail_timeout`.
    pub fn report_failure(&self, index: usize, error: &str) {
        let Some(mut health) = self.health(index) else {
            return;
        };
        health.last_error = Some(error.to_string());
        let max_fails = self.health_check.max_fails;
        if max_fails == 0 {
            return;
        }
        let now = Instant::now();
        health.failures = health.failures.saturating_add(1);
        if health.failures < max_fails {
            return;
        }
        if health.is_healthy(now) {
            warn!(
                server = self.addr(index),
                failures = health.failures,
                error,
                "upstream server is unhealthy"
            );
        }
        health.unhealthy_until = Some(now + self.health_check.fail_timeout);
    }

    /// Records the result of an active check. A failed check marks the server unhealthy until a
    /// check passes.
    fn record_check(&self, index: usize, result: Result<(), String>) {
        let Some(mut health) = self.health(index) else {
            return;
        };
        match result {
            Ok(()) => {
                if health.check_failed {
                    info!(
                        server = self.addr(index),
                        "upstream server passed the active health check"
                    );
                }
                *health = ServerHealth::default();
            }
            Err(error) => {
                if !health.check_failed {
                    warn!(
                        server = self.addr(index),
                        error, "upstream server failed the active health check"
                    );
                }
                health.check_failed = true;
                health.last_error = Some(error);
            }
        }
    }

    /// Checks every server of the group at the same time.
    async fn check(&self) {
        let probe = self
            .probe
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(probe) = probe else {
            return;
        };
        let timeout = self.health_check.timeout;
        let mut checks = JoinSet::new();
        for index in 0..self.servers.len() {
            let probe = probe.clone();
            checks.spawn(async move { (index, probe.check(index, timeout).await) });
        }
        while let Some(joined) = checks.join_next().await {
            match joined {
                Ok((index, result)) => self.record_check(index, result),
                Err(err) => warn!(%err, "the active health check task failed"),
            }
        }
    }

    fn upstream_health(&self, now: Instant) -> Vec<UpstreamHealth> {
        self.members
            .iter()
            .enumerate()
            .filter_map(|(index, member)| {
                let health = self.health(index)?;
                Some(UpstreamHealth {
                    addr: member.addr.clone(),
                    weight: member.weight,
                    healthy: health.is_healthy(now),
                    failures: health.failures,
                    last_error: health.last_error.clone(),
                })
            })
            .collect()
    }

    fn addr(&self, index: usize) -> &str {
        self.members
            .get(index)
            .map_or("", |member| member.addr.as_str())
    }

    fn weight(&self, index: usize) -> u16 {
        self.members.get(index).map_or(0, |member| member.weight)
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
    use tokio::net::TcpListener;

    /// Servers named a, b, c and so on with the weights.
    fn members(weights: &[u16]) -> Vec<GroupServer> {
        weights
            .iter()
            .zip('a'..)
            .map(|(&weight, name)| GroupServer {
                addr: name.to_string(),
                weight,
            })
            .collect()
    }

    fn test_group(policy: LoadBalancing, health_check: HealthCheck) -> UpstreamGroup {
        UpstreamGroup::new(members(&[1, 1, 1]), policy, health_check)
    }

    fn no_passive_check() -> HealthCheck {
        HealthCheck {
            max_fails: 0,
            ..Default::default()
        }
    }

    #[test]
    fn policies_order_every_server() {
        let group = test_group(LoadBalancing::RoundRobin, HealthCheck::default());
        for expected in [[0, 1, 2], [1, 2, 0], [2, 0, 1], [0, 1, 2]] {
            assert_eq!(group.candidates(), expected);
        }

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
    fn weighted_round_robin_is_smooth() {
        let group = UpstreamGroup::new(
            members(&[5, 1, 1]),
            LoadBalancing::RoundRobin,
            HealthCheck::default(),
        );
        let first = (0..14).map(|_| group.candidates()[0]).collect::<Vec<_>>();
        assert_eq!(first, [0, 0, 1, 0, 2, 0, 0, 0, 0, 1, 0, 2, 0, 0]);
    }

    #[test]
    fn weight_zero_is_never_selected() {
        for policy in LoadBalancing::ALL {
            let group = UpstreamGroup::new(members(&[0, 1, 2]), policy, no_passive_check());
            for _ in 0..20 {
                let candidates = group.candidates();
                assert_eq!(candidates.len(), 2, "{policy:?}");
                assert!(!candidates.contains(&0), "{policy:?}");
            }
        }

        // A drained server stays unused also when every other server is unhealthy.
        let group = UpstreamGroup::new(
            members(&[0, 1]),
            LoadBalancing::First,
            HealthCheck::default(),
        );
        group.report_failure(1, "refused");
        assert_eq!(group.candidates(), [1]);
    }

    #[test]
    fn weighted_random_follows_weights() {
        let group = UpstreamGroup::new(members(&[9, 1]), LoadBalancing::Random, no_passive_check());
        let first = (0..2000).filter(|_| group.candidates()[0] == 0).count();
        // The expected count is 1800, with a standard deviation of about 13.
        assert!((1700..=1900).contains(&first), "{first}");
    }

    #[test]
    fn unhealthy_servers_move_behind_the_healthy_servers() {
        let health_check = HealthCheck {
            max_fails: 2,
            fail_timeout: Duration::from_millis(50),
            ..Default::default()
        };
        let group = test_group(LoadBalancing::First, health_check);
        group.report_failure(0, "refused");
        assert_eq!(group.candidates(), [0, 1, 2]);
        group.report_failure(0, "refused");
        assert_eq!(group.candidates(), [1, 2, 0]);

        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(group.candidates(), [0, 1, 2]);
        // The failure count stays until a success, so one more failure marks the server again.
        group.report_failure(0, "refused");
        assert_eq!(group.candidates(), [1, 2, 0]);

        group.report_success(0);
        assert_eq!(group.candidates(), [0, 1, 2]);
    }

    #[test]
    fn zero_max_fails_disables_the_passive_health_check() {
        let group = test_group(LoadBalancing::First, no_passive_check());
        group.report_failure(0, "refused");
        group.report_failure(0, "refused");
        assert_eq!(group.candidates(), [0, 1, 2]);
    }

    #[test]
    fn a_failed_active_check_stays_until_a_check_passes() {
        let group = test_group(LoadBalancing::First, no_passive_check());
        group.record_check(0, Err("status 500".into()));
        assert_eq!(group.candidates(), [1, 2, 0]);

        group.report_success(0);
        assert_eq!(group.candidates(), [1, 2, 0]);

        group.record_check(0, Ok(()));
        assert_eq!(group.candidates(), [0, 1, 2]);
    }

    #[test]
    fn registry_reuses_a_group_with_the_same_servers_and_settings() {
        let key = ("group".parse().unwrap(), Some(0));
        let probe = || Probe::Connect(Vec::new());
        let first = group(
            key,
            members(&[1, 1]),
            LoadBalancing::First,
            Default::default(),
            probe(),
        );
        let same = group(
            key,
            members(&[1, 1]),
            LoadBalancing::First,
            Default::default(),
            probe(),
        );
        assert!(Arc::ptr_eq(&first, &same));

        let changed = group(
            key,
            members(&[1, 1]),
            LoadBalancing::Random,
            Default::default(),
            probe(),
        );
        assert!(!Arc::ptr_eq(&first, &changed));

        let reweighted = group(
            key,
            members(&[1, 3]),
            LoadBalancing::Random,
            Default::default(),
            probe(),
        );
        assert!(!Arc::ptr_eq(&changed, &reweighted));
    }

    #[test]
    fn snapshot_lists_the_servers_of_each_route_in_route_order() {
        let id = "snap".parse().unwrap();
        let probe = || Probe::Connect(Vec::new());
        let policy = LoadBalancing::First;
        let second = group(
            (id, Some(1)),
            members(&[1, 2])[1..].to_vec(),
            policy,
            no_passive_check(),
            probe(),
        );
        let first = group(
            (id, Some(0)),
            members(&[1]),
            policy,
            no_passive_check(),
            probe(),
        );
        second.record_check(0, Err("status 500".into()));

        let health = snapshot(id);
        let addrs = health.iter().map(|h| h.addr.as_str()).collect::<Vec<_>>();
        assert_eq!(addrs, ["a", "b"]);
        assert_eq!(health[1].weight, 2);
        assert!(health[0].healthy);
        assert!(!health[1].healthy);
        assert_eq!(health[1].last_error.as_deref(), Some("status 500"));
        drop(first);
    }

    #[tokio::test]
    async fn probes_check_the_servers() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let open = listener.local_addr().unwrap().port();
        let closed = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_port = closed.local_addr().unwrap().port();
        drop(closed);

        let timeout = Duration::from_secs(2);
        let connect = Probe::Connect(vec![
            Some(("127.0.0.1".into(), open)),
            Some(("127.0.0.1".into(), closed_port)),
            None,
        ]);
        assert_eq!(connect.check(0, timeout).await, Ok(()));
        assert!(connect.check(1, timeout).await.is_err());
        assert!(connect.check(2, timeout).await.is_err());

        let resolve = Probe::Resolve(vec![Some(("localhost".into(), 53))]);
        assert_eq!(resolve.check(0, timeout).await, Ok(()));
    }
}
