use crate::proxy::http::pool::ConnectionPool;
use crate::proxy::proxy_protocol;
use fnv::FnvHasher;
use hyper::Uri;
use once_cell::sync::Lazy;
use r3v3rs3_api::{
    id::ShortId,
    port::UpstreamServer,
    upstream::{CircuitBreaker, CircuitState, HealthCheck, LoadBalancing, UpstreamHealth},
};
use rand::Rng;
use std::{
    collections::HashMap,
    hash::Hasher,
    net::IpAddr,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, net::TcpStream, task::JoinSet, time::MissedTickBehavior};
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
    circuit_breaker: CircuitBreaker,
    /// The circuit of each server. It is separate from the health, because a passed active check
    /// resets the health.
    breakers: Vec<Mutex<Breaker>>,
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

#[derive(Debug, Default)]
enum Circuit {
    #[default]
    Closed,
    /// No traffic goes to the server until the time.
    Open { until: Instant },
    /// One trial request tests the server. `trial` is true while the trial request runs.
    HalfOpen { trial: bool },
}

/// The circuit of a server, and the requests of the current window of a closed circuit.
#[derive(Debug, Default)]
struct Breaker {
    circuit: Circuit,
    window_start: Option<Instant>,
    requests: u32,
    failures: u32,
}

impl Breaker {
    /// True when the circuit lets no traffic through.
    fn blocks(&self, now: Instant) -> bool {
        match self.circuit {
            Circuit::Closed | Circuit::HalfOpen { trial: false } => false,
            Circuit::Open { until } => now < until,
            Circuit::HalfOpen { trial: true } => true,
        }
    }

    /// Lets a request through. `None` when the circuit blocks it, `Some(true)` for the trial
    /// request of a half-open circuit.
    fn admit(&mut self, now: Instant) -> Option<bool> {
        if self.blocks(now) {
            return None;
        }
        if matches!(self.circuit, Circuit::Closed) {
            return Some(false);
        }
        self.circuit = Circuit::HalfOpen { trial: true };
        Some(true)
    }

    /// Records the result of a request, and returns the new state when the circuit changes.
    fn record(
        &mut self,
        config: &CircuitBreaker,
        trial: bool,
        failed: bool,
        now: Instant,
    ) -> Option<CircuitState> {
        if trial {
            return self.finish_trial(config, failed, now);
        }
        // A request that started before the circuit opened does not count.
        if !matches!(self.circuit, Circuit::Closed) {
            return None;
        }
        self.count(config.window, failed, now);
        let ratio = u64::from(self.failures) * 100;
        let limit = u64::from(config.failure_ratio) * u64::from(self.requests);
        if self.requests < config.min_requests || ratio < limit {
            return None;
        }
        self.open(config, now);
        Some(CircuitState::Open)
    }

    fn finish_trial(
        &mut self,
        config: &CircuitBreaker,
        failed: bool,
        now: Instant,
    ) -> Option<CircuitState> {
        if !matches!(self.circuit, Circuit::HalfOpen { .. }) {
            return None;
        }
        if failed {
            self.open(config, now);
            return Some(CircuitState::Open);
        }
        *self = Self::default();
        Some(CircuitState::Closed)
    }

    fn count(&mut self, window: Duration, failed: bool, now: Instant) {
        if self
            .window_start
            .is_none_or(|start| now.duration_since(start) >= window)
        {
            self.window_start = Some(now);
            self.requests = 0;
            self.failures = 0;
        }
        self.requests = self.requests.saturating_add(1);
        if failed {
            self.failures = self.failures.saturating_add(1);
        }
    }

    fn open(&mut self, config: &CircuitBreaker, now: Instant) {
        *self = Self {
            circuit: Circuit::Open {
                until: now + config.open_duration,
            },
            ..Self::default()
        };
    }

    /// Frees the slot of a trial request that ended without a result.
    fn release_trial(&mut self) {
        if matches!(self.circuit, Circuit::HalfOpen { trial: true }) {
            self.circuit = Circuit::HalfOpen { trial: false };
        }
    }

    fn state(&self) -> CircuitState {
        match self.circuit {
            Circuit::Closed => CircuitState::Closed,
            Circuit::Open { .. } => CircuitState::Open,
            Circuit::HalfOpen { .. } => CircuitState::HalfOpen,
        }
    }
}

/// The right to send one request or to open one connection to a server of a group. Report the
/// result with [`Permit::success`], [`Permit::failure`] or [`Permit::error_response`]. A permit
/// that is dropped without a result frees the trial slot of a half-open circuit.
#[derive(Debug)]
pub struct Permit {
    group: Arc<UpstreamGroup>,
    index: usize,
    trial: bool,
    reported: bool,
}

impl Permit {
    pub fn index(&self) -> usize {
        self.index
    }

    /// The server answered, or accepted the connection.
    pub fn success(mut self) {
        self.group.report_success(self.index);
        self.record(None);
    }

    /// The connection failed, or the server did not answer.
    pub fn failure(mut self, error: &str) {
        self.group.report_failure(self.index, error);
        self.record(Some(error));
    }

    /// The server answered with a gateway error. The passive health check counts the answer as a
    /// success, and the circuit breaker counts it as a failure.
    pub fn error_response(mut self, error: &str) {
        self.group.report_success(self.index);
        self.record(Some(error));
    }

    fn record(&mut self, failure: Option<&str>) {
        self.reported = true;
        self.group.record_circuit(self.index, self.trial, failure);
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        if self.trial && !self.reported {
            self.group.release_trial(self.index);
        }
    }
}

/// How the active health check reaches each server of a group. The targets have the order of the
/// servers, and `None` marks a server that the check cannot reach.
#[derive(Debug)]
pub enum Probe {
    /// Opens a TCP connection to the host and the port.
    Connect(Vec<Option<(String, u16)>>),
    /// Opens a TCP connection and sends a PROXY protocol v2 `LOCAL` header, for servers that
    /// expect a header on each connection.
    ConnectLocal(Vec<Option<(String, u16)>>),
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
            Self::ConnectLocal(targets) => {
                let (host, port) = target(targets, index)?;
                connect_local(host, *port)
                    .await
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

async fn connect_local(host: &str, port: u16) -> std::io::Result<()> {
    let mut stream = TcpStream::connect((host, port)).await?;
    stream.write_all(&proxy_protocol::local_header()?).await
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
    circuit_breaker: CircuitBreaker,
    probe: Probe,
) -> Arc<UpstreamGroup> {
    let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    registry.retain(|_, group| group.strong_count() > 0);
    if let Some(existing) = registry.get(&key).and_then(Weak::upgrade) {
        if existing.members == members
            && existing.policy == policy
            && existing.health_check == health_check
            && existing.circuit_breaker == circuit_breaker
        {
            existing.set_probe(probe);
            return existing;
        }
    }
    let group = Arc::new(
        UpstreamGroup::new(members, policy, health_check).with_circuit_breaker(circuit_breaker),
    );
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
        let breakers = members.iter().map(|_| Mutex::default()).collect();
        Self {
            members,
            policy,
            health_check,
            servers,
            current_weights,
            circuit_breaker: CircuitBreaker::default(),
            breakers,
            probe: Mutex::new(None),
        }
    }

    fn with_circuit_breaker(mut self, circuit_breaker: CircuitBreaker) -> Self {
        self.circuit_breaker = circuit_breaker;
        self
    }

    fn set_probe(&self, probe: Probe) {
        *self.probe.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(probe));
    }

    /// Returns the server indexes in the order to try: the healthy servers in the order of the
    /// policy, then the unhealthy servers. When every server is unhealthy, the unhealthy servers
    /// use the order of the policy, so the request still goes to a server. A server with weight 0
    /// and a server whose circuit blocks the traffic are never candidates. `client` is the IP
    /// address that the client IP hash uses.
    pub fn candidates(&self, client: IpAddr) -> Vec<usize> {
        let now = Instant::now();
        let (healthy, unhealthy): (Vec<_>, Vec<_>) = (0..self.members.len())
            .filter(|&index| self.weight(index) > 0 && !self.blocked(index, now))
            .partition(|&index| self.health(index).is_some_and(|h| h.is_healthy(now)));
        if healthy.is_empty() {
            return self.order(unhealthy, client);
        }
        let mut ordered = self.order(healthy, client);
        ordered.extend(unhealthy);
        ordered
    }

    /// True when a sticky session can stay on the server: the server is healthy and its circuit
    /// lets the traffic through. A server with weight 0 keeps its sticky sessions.
    pub fn is_pinnable(&self, index: usize) -> bool {
        let now = Instant::now();
        !self.blocked(index, now) && self.health(index).is_some_and(|h| h.is_healthy(now))
    }

    fn order(&self, servers: Vec<usize>, client: IpAddr) -> Vec<usize> {
        match self.policy {
            LoadBalancing::RoundRobin => self.order_round_robin(servers),
            LoadBalancing::Random => self.order_random(servers),
            LoadBalancing::First => servers,
            LoadBalancing::ClientIpHash => self.order_hash(servers, client),
        }
    }

    /// The weighted rendezvous hash: each server gets the score `-ln(u) / weight` for a `u` in
    /// `(0, 1)` from the hash of the client IP address and the server address, and a lower score
    /// goes first. When a server leaves the order, only its own clients move.
    fn order_hash(&self, servers: Vec<usize>, client: IpAddr) -> Vec<usize> {
        let mut keyed = servers
            .into_iter()
            .map(|index| (self.hash_score(index, client), index))
            .collect::<Vec<_>>();
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
        keyed.into_iter().map(|(_, index)| index).collect()
    }

    fn hash_score(&self, index: usize, client: IpAddr) -> f64 {
        let mut hasher = FnvHasher::default();
        match client.to_canonical() {
            IpAddr::V4(ip) => hasher.write(&ip.octets()),
            IpAddr::V6(ip) => hasher.write(&ip.octets()),
        }
        hasher.write(self.addr(index).as_bytes());
        // The upper 53 bits of the hash fit an f64 exactly.
        let unit = ((hasher.finish() >> 11) as f64 + 1.0) / ((1u64 << 53) as f64 + 1.0);
        -unit.ln() / f64::from(self.weight(index))
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

    /// Lets a request or a connection go to the server. `None` when the circuit of the server
    /// blocks it. A half-open circuit lets one trial request through.
    pub fn acquire(self: &Arc<Self>, index: usize) -> Option<Permit> {
        let trial = if self.circuit_breaker.enabled {
            self.breaker(index)?.admit(Instant::now())?
        } else {
            false
        };
        Some(Permit {
            group: self.clone(),
            index,
            trial,
            reported: false,
        })
    }

    fn record_circuit(&self, index: usize, trial: bool, failure: Option<&str>) {
        if !self.circuit_breaker.enabled {
            return;
        }
        let Some(mut breaker) = self.breaker(index) else {
            return;
        };
        let now = Instant::now();
        match breaker.record(&self.circuit_breaker, trial, failure.is_some(), now) {
            Some(CircuitState::Open) => warn!(
                server = self.addr(index),
                error = failure,
                "the circuit of the upstream server is open"
            ),
            Some(CircuitState::Closed) => info!(
                server = self.addr(index),
                "the circuit of the upstream server is closed"
            ),
            _ => {}
        }
    }

    fn release_trial(&self, index: usize) {
        if let Some(mut breaker) = self.breaker(index) {
            breaker.release_trial();
        }
    }

    fn blocked(&self, index: usize, now: Instant) -> bool {
        self.circuit_breaker.enabled && self.breaker(index).is_some_and(|b| b.blocks(now))
    }

    fn circuit_state(&self, index: usize) -> CircuitState {
        self.breaker(index)
            .map_or(CircuitState::Closed, |breaker| breaker.state())
    }

    fn breaker(&self, index: usize) -> Option<MutexGuard<'_, Breaker>> {
        self.breakers
            .get(index)
            .map(|breaker| breaker.lock().unwrap_or_else(PoisonError::into_inner))
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
                let circuit = self.circuit_state(index);
                let health = self.health(index)?;
                Some(UpstreamHealth {
                    addr: member.addr.clone(),
                    weight: member.weight,
                    circuit,
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
    use std::net::Ipv4Addr;
    use tokio::net::TcpListener;

    const CLIENT: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

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
            assert_eq!(group.candidates(CLIENT), expected);
        }

        let group = test_group(LoadBalancing::First, HealthCheck::default());
        assert_eq!(group.candidates(CLIENT), [0, 1, 2]);
        assert_eq!(group.candidates(CLIENT), [0, 1, 2]);

        let group = test_group(LoadBalancing::Random, HealthCheck::default());
        for _ in 0..20 {
            let mut candidates = group.candidates(CLIENT);
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
        let first = (0..14)
            .map(|_| group.candidates(CLIENT)[0])
            .collect::<Vec<_>>();
        assert_eq!(first, [0, 0, 1, 0, 2, 0, 0, 0, 0, 1, 0, 2, 0, 0]);
    }

    #[test]
    fn weight_zero_is_never_selected() {
        for policy in LoadBalancing::ALL {
            let group = UpstreamGroup::new(members(&[0, 1, 2]), policy, no_passive_check());
            for _ in 0..20 {
                let candidates = group.candidates(CLIENT);
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
        assert_eq!(group.candidates(CLIENT), [1]);
    }

    #[test]
    fn weighted_random_follows_weights() {
        let group = UpstreamGroup::new(members(&[9, 1]), LoadBalancing::Random, no_passive_check());
        let first = (0..2000)
            .filter(|_| group.candidates(CLIENT)[0] == 0)
            .count();
        // The expected count is 1800, with a standard deviation of about 13.
        assert!((1700..=1900).contains(&first), "{first}");
    }

    #[test]
    fn client_ip_hash_keeps_each_client_on_its_server() {
        let group = test_group(LoadBalancing::ClientIpHash, HealthCheck::default());
        let clients = (1..=60)
            .map(|n| IpAddr::V4(Ipv4Addr::new(10, 0, 0, n)))
            .collect::<Vec<_>>();
        let servers = |group: &UpstreamGroup| {
            clients
                .iter()
                .map(|&client| group.candidates(client)[0])
                .collect::<Vec<_>>()
        };
        let first = servers(&group);
        assert_eq!(servers(&group), first);
        for server in 0..3 {
            assert!(first.contains(&server), "server {server} gets no client");
        }
        let mapped = "::ffff:10.0.0.1".parse().unwrap();
        assert_eq!(group.candidates(mapped), group.candidates(clients[0]));

        // An unhealthy server moves only its own clients.
        group.report_failure(0, "refused");
        for (now, before) in servers(&group).into_iter().zip(first) {
            if before == 0 {
                assert_ne!(now, 0);
            } else {
                assert_eq!(now, before);
            }
        }
        assert!(!group.is_pinnable(0));
        assert!(group.is_pinnable(1));
    }

    #[test]
    fn a_drained_server_keeps_its_sticky_sessions() {
        let group = UpstreamGroup::new(members(&[0, 1]), LoadBalancing::First, no_passive_check());
        assert_eq!(group.candidates(CLIENT), [1]);
        assert!(group.is_pinnable(0));
    }

    fn breaker(min_requests: u32, open_duration: Duration) -> CircuitBreaker {
        CircuitBreaker {
            enabled: true,
            failure_ratio: 50,
            min_requests,
            window: Duration::from_secs(10),
            open_duration,
        }
    }

    fn breaker_group(circuit_breaker: CircuitBreaker) -> Arc<UpstreamGroup> {
        let group = UpstreamGroup::new(members(&[1, 1]), LoadBalancing::First, no_passive_check());
        Arc::new(group.with_circuit_breaker(circuit_breaker))
    }

    fn circuit(group: &UpstreamGroup) -> CircuitState {
        group.upstream_health(Instant::now())[0].circuit
    }

    #[test]
    fn the_circuit_opens_at_the_failure_ratio_and_a_trial_closes_it() {
        let group = breaker_group(breaker(4, Duration::from_millis(50)));
        group.acquire(0).unwrap().failure("refused");
        group.acquire(0).unwrap().success();
        group.acquire(0).unwrap().success();
        assert_eq!(group.candidates(CLIENT), [0, 1], "1 of 3 failed");

        // The fourth request makes 2 failures in 4 requests, which is the ratio of 50 percent.
        group.acquire(0).unwrap().error_response("status 503");
        assert_eq!(group.candidates(CLIENT), [1]);
        assert!(group.acquire(0).is_none());
        assert_eq!(circuit(&group), CircuitState::Open);

        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(group.candidates(CLIENT), [0, 1]);
        let trial = group.acquire(0).unwrap();
        assert!(group.acquire(0).is_none(), "one trial at a time");
        assert_eq!(group.candidates(CLIENT), [1]);
        assert_eq!(circuit(&group), CircuitState::HalfOpen);

        trial.success();
        assert_eq!(group.candidates(CLIENT), [0, 1]);
        assert_eq!(circuit(&group), CircuitState::Closed);
    }

    #[test]
    fn a_failed_trial_opens_the_circuit_and_an_abandoned_trial_frees_it() {
        let group = breaker_group(breaker(1, Duration::from_millis(30)));
        group.acquire(0).unwrap().failure("refused");
        assert!(group.acquire(0).is_none());

        std::thread::sleep(Duration::from_millis(40));
        drop(group.acquire(0).unwrap());
        let trial = group.acquire(0).unwrap();
        trial.failure("refused");
        assert!(group.acquire(0).is_none());
        assert_eq!(circuit(&group), CircuitState::Open);
    }

    #[test]
    fn a_disabled_circuit_breaker_never_blocks() {
        let group = breaker_group(CircuitBreaker::default());
        for _ in 0..50 {
            group.acquire(0).unwrap().failure("refused");
        }
        assert_eq!(group.candidates(CLIENT), [0, 1]);
        assert!(group.acquire(0).is_some());
        assert_eq!(circuit(&group), CircuitState::Closed);
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
        assert_eq!(group.candidates(CLIENT), [0, 1, 2]);
        group.report_failure(0, "refused");
        assert_eq!(group.candidates(CLIENT), [1, 2, 0]);

        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(group.candidates(CLIENT), [0, 1, 2]);
        // The failure count stays until a success, so one more failure marks the server again.
        group.report_failure(0, "refused");
        assert_eq!(group.candidates(CLIENT), [1, 2, 0]);

        group.report_success(0);
        assert_eq!(group.candidates(CLIENT), [0, 1, 2]);
    }

    #[test]
    fn zero_max_fails_disables_the_passive_health_check() {
        let group = test_group(LoadBalancing::First, no_passive_check());
        group.report_failure(0, "refused");
        group.report_failure(0, "refused");
        assert_eq!(group.candidates(CLIENT), [0, 1, 2]);
    }

    #[test]
    fn a_failed_active_check_stays_until_a_check_passes() {
        let group = test_group(LoadBalancing::First, no_passive_check());
        group.record_check(0, Err("status 500".into()));
        assert_eq!(group.candidates(CLIENT), [1, 2, 0]);

        group.report_success(0);
        assert_eq!(group.candidates(CLIENT), [1, 2, 0]);

        group.record_check(0, Ok(()));
        assert_eq!(group.candidates(CLIENT), [0, 1, 2]);
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
            CircuitBreaker::default(),
            probe(),
        );
        let same = group(
            key,
            members(&[1, 1]),
            LoadBalancing::First,
            Default::default(),
            CircuitBreaker::default(),
            probe(),
        );
        assert!(Arc::ptr_eq(&first, &same));

        let changed = group(
            key,
            members(&[1, 1]),
            LoadBalancing::Random,
            Default::default(),
            CircuitBreaker::default(),
            probe(),
        );
        assert!(!Arc::ptr_eq(&first, &changed));

        let reweighted = group(
            key,
            members(&[1, 3]),
            LoadBalancing::Random,
            Default::default(),
            CircuitBreaker::default(),
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
            CircuitBreaker::default(),
            probe(),
        );
        let first = group(
            (id, Some(0)),
            members(&[1]),
            policy,
            no_passive_check(),
            CircuitBreaker::default(),
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

    #[tokio::test]
    async fn connect_local_probe_sends_a_local_header() {
        use tokio::io::AsyncReadExt;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let probe = Probe::ConnectLocal(vec![Some(("127.0.0.1".into(), port))]);
        let (checked, accepted) =
            tokio::join!(probe.check(0, Duration::from_secs(2)), listener.accept());
        assert_eq!(checked, Ok(()));

        let expected = proxy_protocol::local_header().unwrap();
        let mut received = vec![0; expected.len()];
        accepted.unwrap().0.read_exact(&mut received).await.unwrap();
        assert_eq!(received, expected);
    }
}
