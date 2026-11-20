use super::dns::DnsResolver;
use super::health::{self, GroupRegistry, Probe, UpstreamGroup};
use super::{PortContextEvent, PortStatus, SocketState};
use crate::security::AutoQuarantineTracker;
use dashmap::DashMap;
use r3v3rs3_api::multiaddr::PortSpan;
use r3v3rs3_api::upstream::{HealthCheck, LoadBalancing};
use r3v3rs3_api::{error::Error, multiaddr::Multiaddr, proxy::ProxyKind};
use r3v3rs3_api::{port::PortEntry, proxy::ProxyEntry};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use std::{net::SocketAddr, time::SystemTime};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::pki_types::ServerName;
use tracing::{Level, Span, debug, error, info, span, warn};

type Resolver = DnsResolver;

const MAX_SESSIONS: usize = 10_000;
const UDP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// One datagram of a client. `local` is the original destination that TPROXY keeps for a range
/// port; a single port has no use for it.
#[derive(Debug)]
pub struct UdpDatagram {
    pub client: SocketAddr,
    pub local: Option<SocketAddr>,
    pub data: Vec<u8>,
}

/// A session is one client on one port of the range.
type SessionKey = (SocketAddr, u16);

#[derive(Debug, Clone)]
pub struct UdpPortContext {
    /// The address of the socket: the first port of a range.
    pub listen: SocketAddr,
    /// The ports of the address; more than one makes it a TPROXY range port.
    pub ports: PortSpan,
    upstream: Arc<UdpUpstream>,
    sessions: Arc<DashMap<SessionKey, UdpSession>>,
    status: PortStatus,
    span: Span,
    resolver: Resolver,
}

impl UdpPortContext {
    pub fn new(entry: &PortEntry) -> Result<Self, Error> {
        let span = span!(Level::INFO, "proxy", resource_id = entry.id.to_string(), listen = %entry.port.listen);
        let enter = span.clone();
        let _enter = enter.enter();

        info!("initializing udp proxy");

        let resolver =
            DnsResolver::from_system_conf().map_err(|err| Error::FailedToBuildDnsResolver {
                reason: err.to_string(),
            })?;

        let listen = entry.port.listen.socket_addr()?;
        Ok(Self {
            listen,
            ports: entry.port.listen.port_range()?,
            upstream: Default::default(),
            sessions: Arc::new(DashMap::new()),
            status: Default::default(),
            span,
            resolver,
        })
    }

    pub async fn setup(
        &mut self,
        proxies: Vec<ProxyEntry>,
        groups: &GroupRegistry,
    ) -> Result<(), Error> {
        let upstream = UdpUpstream::from_proxies(proxies, groups)?;
        // Keep the open sessions, the resolved addresses and the server health when the settings
        // do not change.
        if !upstream.same_settings(&self.upstream) {
            self.upstream = Arc::new(upstream);
            self.sessions.clear();
        }
        Ok(())
    }

    pub fn apply(&mut self, new: Self) {
        *self = Self {
            resolver: self.resolver.clone(),
            ..new
        };
    }

    pub fn event(&mut self, event: PortContextEvent) {
        match event {
            PortContextEvent::SocketStateUpdated(state) => {
                if self.status.state.socket != state {
                    self.status.started_at = if state == SocketState::Listening {
                        Some(SystemTime::now())
                    } else {
                        None
                    };
                }
                self.status.state.socket = state;
            }
        }
    }

    pub fn status(&self) -> &PortStatus {
        &self.status
    }

    pub fn reset(&mut self) {
        self.sessions.clear();
    }

    /// Sends a packet of a client to the upstream server of its session. The replies of the
    /// server go back to the client through `listener`, or, on a range port, through a socket on
    /// the port the client sent to. A packet of a quarantined client is dropped and closes its
    /// open sessions, so the server neither gets nor answers it.
    pub async fn forward(
        &self,
        datagram: UdpDatagram,
        listener: &Arc<UdpSocket>,
        quarantine: &AutoQuarantineTracker,
    ) {
        let UdpDatagram {
            client,
            local,
            data,
        } = datagram;
        if quarantine.is_quarantined(&client.ip()) {
            self.sessions.retain(|(c, _), _| *c != client);
            self.span.in_scope(|| {
                debug!(%client, "dropping a packet of a quarantined client");
            });
            return;
        }
        let Some(local) = self.local_port(client, local, data.len()) else {
            return;
        };
        let key = (client, local.port());
        let is_open = self
            .sessions
            .get(&key)
            .map(|s| s.is_open())
            .unwrap_or(false);
        if !is_open {
            let Some(session) = self.open_session(client, local, listener).await else {
                return;
            };
            self.sessions.insert(key, session);
        }
        let (socket, context) = match self.sessions.get(&key) {
            Some(session) => {
                session.touch();
                (session.socket.clone(), session.context.clone())
            }
            None => return,
        };
        if let Err(err) = socket.send(&data).await {
            context.report_failure(&err.to_string());
            self.span.in_scope(|| {
                debug!(%client, %err, "failed to send the packet to the upstream server");
            });
        }
    }

    /// The address the client sent to: the original destination on a range port, the listening
    /// address on a single port. A datagram above `max_datagram_size` would reach the server
    /// cut, and one without its destination cannot be mapped, so both are dropped.
    fn local_port(
        &self,
        client: SocketAddr,
        local: Option<SocketAddr>,
        size: usize,
    ) -> Option<SocketAddr> {
        if size > self.upstream.max_datagram_size {
            self.span.in_scope(|| {
                debug!(%client, size, "dropping a packet above max_datagram_size");
            });
            return None;
        }
        if self.ports.port_count() == 1 {
            return Some(self.listen);
        }
        let local = local.filter(|l| self.ports.contains(l.port()));
        if local.is_none() {
            self.span.in_scope(|| {
                debug!(%client, "dropping a range packet without its original destination");
            });
        }
        local
    }

    /// The socket that sends the replies of a session: on a range port it sends from the port
    /// the client sent to, so the client sees the reply come from that port.
    fn reply_socket(&self, local: SocketAddr, listener: &Arc<UdpSocket>) -> Option<Arc<UdpSocket>> {
        if self.ports.port_count() == 1 {
            return Some(listener.clone());
        }
        match super::transparent::bind_reply_socket(local) {
            Ok(socket) => Some(Arc::new(socket)),
            Err(err) => {
                self.span.in_scope(|| {
                    error!(%local, %err, "failed to open the reply socket of the range port");
                });
                None
            }
        }
    }

    /// Opens a session to the selected server. When the server cannot be resolved or its socket
    /// fails, it opens the session once to the next server.
    async fn open_session(
        &self,
        client: SocketAddr,
        local: SocketAddr,
        listener: &Arc<UdpSocket>,
    ) -> Option<UdpSession> {
        self.sessions.retain(|_, session| session.is_open());
        if self.sessions.len() >= MAX_SESSIONS {
            self.span.in_scope(|| {
                warn!(%client, "dropping the packet: too many udp sessions");
            });
            return None;
        }
        let group = self.upstream.group.clone()?;
        let reply = self.reply_socket(local, listener)?;
        for index in group.candidates(client.ip()).into_iter().take(2) {
            let context = SessionContext {
                listener: reply.clone(),
                local_port: local.port(),
                client,
                activity: Activity::new(self.upstream.idle_timeout),
                max_datagram_size: self.upstream.max_datagram_size,
                group: group.clone(),
                index,
                span: self.span.clone(),
            };
            if let Some(session) = self.open_server_session(context).await {
                return Some(session);
            }
        }
        None
    }

    async fn open_server_session(&self, context: SessionContext) -> Option<UdpSession> {
        let server = self.upstream.servers.get(context.index)?;
        let port = match self.ports.port_count() {
            1 => server.port,
            _ => self.ports.map_to(context.local_port, &server.span)?,
        };
        let deadline = Instant::now() + UDP_CONNECT_TIMEOUT;
        let Some(upstream) = resolve(&self.resolver, &self.span, server, port, deadline).await
        else {
            context.report_failure("failed to resolve the upstream server");
            return None;
        };
        let context = Arc::new(context);
        match UdpSession::open(upstream, context.clone()).await {
            Ok(session) => Some(session),
            Err(err) => {
                context.report_failure(&err.to_string());
                self.span.in_scope(|| {
                    error!(%upstream, %err, "failed to open the upstream socket");
                });
                None
            }
        }
    }
}

/// The upstream servers and the session settings of the UDP proxy of a port.
#[derive(Debug, Clone)]
struct UdpUpstream {
    servers: Vec<Connection>,
    idle_timeout: Duration,
    max_datagram_size: usize,
    load_balancing: LoadBalancing,
    health_check: HealthCheck,
    /// `None` when no proxy of the port has a server.
    group: Option<Arc<UpstreamGroup>>,
}

impl Default for UdpUpstream {
    fn default() -> Self {
        Self {
            servers: Vec::new(),
            idle_timeout: r3v3rs3_api::upstream::DEFAULT_SESSION_IDLE_TIMEOUT,
            max_datagram_size: r3v3rs3_api::upstream::DEFAULT_MAX_DATAGRAM_SIZE,
            load_balancing: Default::default(),
            health_check: Default::default(),
            group: None,
        }
    }
}

impl UdpUpstream {
    /// Uses the upstream servers of the first proxy of the port that has a server.
    fn from_proxies(proxies: Vec<ProxyEntry>, groups: &GroupRegistry) -> Result<Self, Error> {
        let mut upstream = Self::default();
        for entry in proxies {
            let ProxyKind::Udp(proxy) = entry.proxy.kind else {
                continue;
            };
            let servers = proxy
                .upstream_servers
                .iter()
                .map(|server| multiaddr_to_host(&server.addr))
                .collect::<Result<Vec<_>, _>>()?;
            if upstream.group.is_some() || servers.is_empty() {
                continue;
            }
            let group = groups.group(
                (entry.id, None),
                health::members(&proxy.upstream_servers),
                proxy.load_balancing,
                proxy.health_check.clone(),
                Default::default(),
                Probe::Resolve(Probe::targets(&proxy.upstream_servers)),
            );
            upstream = Self {
                servers,
                idle_timeout: proxy.session_idle_timeout,
                max_datagram_size: proxy.max_datagram_size,
                load_balancing: proxy.load_balancing,
                health_check: proxy.health_check,
                group: Some(group),
            };
        }
        Ok(upstream)
    }

    fn same_settings(&self, other: &Self) -> bool {
        same_servers(&self.servers, &other.servers)
            && same_group(&self.group, &other.group)
            && self.idle_timeout == other.idle_timeout
            && self.max_datagram_size == other.max_datagram_size
            && self.load_balancing == other.load_balancing
            && self.health_check == other.health_check
    }
}

/// True when both upstreams use the same group. A change of a server weight builds a new group.
fn same_group(a: &Option<Arc<UpstreamGroup>>, b: &Option<Arc<UpstreamGroup>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (a, b) => a.is_none() && b.is_none(),
    }
}

fn same_servers(a: &[Connection], b: &[Connection]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.name == b.name && a.port == b.port)
}

/// Returns the address of the server using DNS round-robin and TTL caching.
async fn resolve(
    resolver: &Resolver,
    span: &Span,
    server: &Connection,
    port: u16,
    deadline: Instant,
) -> Option<SocketAddr> {
    let host = server.name.to_str();
    match tokio::time::timeout_at(deadline.into(), resolver.resolve_addr(&host, port)).await {
        Ok(Ok(addr)) => Some(addr),
        Ok(Err(e)) => {
            span.in_scope(|| {
                error!("failed to resolve {}: {}", host, e);
            });
            None
        }
        Err(_) => {
            span.in_scope(|| {
                error!("timed out resolving {}", host);
            });
            None
        }
    }
}

/// The client, the server and the activity of a session.
#[derive(Debug)]
struct SessionContext {
    /// The socket that sends the replies to the client.
    listener: Arc<UdpSocket>,
    /// The port of the range that the client sent to.
    local_port: u16,
    client: SocketAddr,
    activity: Activity,
    max_datagram_size: usize,
    group: Arc<UpstreamGroup>,
    /// The index of the upstream server in `group`.
    index: usize,
    span: Span,
}

impl SessionContext {
    fn report_failure(&self, error: &str) {
        self.group.report_failure(self.index, error);
    }
}

/// The upstream socket of one client. Dropping the session stops its reply task.
#[derive(Debug)]
struct UdpSession {
    socket: Arc<UdpSocket>,
    context: Arc<SessionContext>,
    task: JoinHandle<()>,
}

impl UdpSession {
    async fn open(upstream: SocketAddr, context: Arc<SessionContext>) -> io::Result<Self> {
        let bind: SocketAddr = if upstream.is_ipv6() {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        };
        let socket = UdpSocket::bind(bind).await?;
        socket.connect(upstream).await?;
        let socket = Arc::new(socket);
        let lease = crate::stats::ConnectionLease::acquire();
        let relay_socket = socket.clone();
        let relay_context = context.clone();
        let task = tokio::spawn(async move {
            let _lease = lease;
            relay_replies(relay_socket, relay_context).await;
        });
        Ok(Self {
            socket,
            context,
            task,
        })
    }

    fn is_open(&self) -> bool {
        !self.task.is_finished()
    }

    fn touch(&self) {
        self.context.activity.touch();
    }
}

impl Drop for UdpSession {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// The time of the last packet of a session, in both directions.
#[derive(Debug)]
struct Activity {
    started: Instant,
    last_active_ms: AtomicU64,
    idle_timeout: Duration,
}

impl Activity {
    fn new(idle_timeout: Duration) -> Self {
        Self {
            started: Instant::now(),
            last_active_ms: AtomicU64::new(0),
            idle_timeout,
        }
    }

    /// The time until the session is idle for `idle_timeout`.
    fn remaining(&self) -> Duration {
        self.idle_timeout.saturating_sub(self.idle_time())
    }

    fn touch(&self) {
        let elapsed = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last_active_ms.store(elapsed, Ordering::Relaxed);
    }

    fn idle_time(&self) -> Duration {
        let last_active = Duration::from_millis(self.last_active_ms.load(Ordering::Relaxed));
        self.started.elapsed().saturating_sub(last_active)
    }
}

/// Sends the replies of the upstream server to the client until the session is idle for its
/// idle timeout or the upstream socket fails. A reply marks the server healthy, and a socket
/// error, such as an ICMP port unreachable message, counts as a failure.
async fn relay_replies(socket: Arc<UdpSocket>, context: Arc<SessionContext>) {
    let client = context.client;
    // One byte more than the limit, so a longer reply shows as one and is dropped instead of
    // being cut by the receive call.
    let mut buf = vec![0; context.max_datagram_size + 1];
    loop {
        let remaining = context.activity.remaining();
        if remaining.is_zero() {
            break;
        }
        let size = match tokio::time::timeout(remaining, socket.recv(&mut buf)).await {
            Ok(Ok(size)) => size,
            Ok(Err(err)) => {
                context.report_failure(&err.to_string());
                context
                    .span
                    .in_scope(|| debug!(%client, %err, "closing the udp session"));
                break;
            }
            Err(_) => continue,
        };
        context.activity.touch();
        context.group.report_success(context.index);
        if size > context.max_datagram_size {
            context.span.in_scope(|| {
                debug!(%client, "dropping a reply above max_datagram_size");
            });
            continue;
        }
        if let Err(err) = context.listener.send_to(&buf[..size], client).await {
            context
                .span
                .in_scope(|| debug!(%client, %err, "failed to send the reply to the client"));
        }
    }
}

fn multiaddr_to_host(addr: &Multiaddr) -> Result<Connection, Error> {
    let invalid = || Error::InvalidServerAddress { addr: addr.clone() };
    let span = addr.port_range().map_err(|_| invalid())?;
    let name = match (addr.ip_addr(), addr.host()) {
        (Ok(ip), _) => ServerName::IpAddress(ip.into()),
        (_, Ok(host)) => ServerName::try_from(host.as_str())
            .map_err(|_| invalid())?
            .to_owned(),
        _ => return Err(invalid()),
    };
    Ok(Connection {
        name,
        port: span.first,
        span,
        addr: None,
        ttl: Instant::now(),
    })
}

#[derive(Debug, Clone)]
pub struct Connection {
    pub name: ServerName<'static>,
    /// The port, or the first port of a range.
    pub port: u16,
    /// The ports of the server; a range port maps each of its ports onto them.
    pub span: PortSpan,
    pub addr: Option<SocketAddr>,
    pub ttl: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::port::{Port, UpstreamServer};
    use r3v3rs3_api::proxy::{Proxy, UdpProxy};

    fn port_entry() -> PortEntry {
        PortEntry {
            id: "port".parse().unwrap(),
            port: Port {
                active: true,
                name: String::new(),
                listen: "/ip4/127.0.0.1/udp/53".parse().unwrap(),
                opts: Default::default(),
            },
            status: None,
        }
    }

    fn datagram(client: SocketAddr, data: &[u8]) -> UdpDatagram {
        UdpDatagram {
            client,
            local: None,
            data: data.to_vec(),
        }
    }

    /// A range port maps a datagram by the port the client sent to, so a datagram without
    /// that destination, or with one outside the range, cannot be mapped and is dropped. A
    /// single port needs no destination.
    #[test]
    fn a_range_datagram_needs_its_original_destination() {
        let mut entry = port_entry();
        entry.port.listen = "/ip4/0.0.0.0/udp/49160-49240".parse().unwrap();
        let range = UdpPortContext::new(&entry).unwrap();
        let client: SocketAddr = "198.51.100.6:4000".parse().unwrap();
        let inside: SocketAddr = "192.0.2.1:49200".parse().unwrap();
        assert_eq!(range.local_port(client, Some(inside), 10), Some(inside));
        assert_eq!(range.local_port(client, None, 10), None);
        let outside: SocketAddr = "192.0.2.1:49241".parse().unwrap();
        assert_eq!(range.local_port(client, Some(outside), 10), None);
        let too_big = range.upstream.max_datagram_size + 1;
        assert_eq!(range.local_port(client, Some(inside), too_big), None);

        let single = UdpPortContext::new(&port_entry()).unwrap();
        assert_eq!(single.local_port(client, None, 10), Some(single.listen));
    }

    fn proxy_entry(id: &str, ports: &[u16]) -> ProxyEntry {
        ProxyEntry {
            id: id.parse().unwrap(),
            source: None,
            status: None,
            proxy: Proxy {
                ports: vec![port_entry().id],
                kind: ProxyKind::Udp(UdpProxy {
                    upstream_servers: ports
                        .iter()
                        .map(|port| {
                            UpstreamServer::new(
                                format!("/ip4/127.0.0.1/udp/{port}").parse().unwrap(),
                            )
                        })
                        .collect(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn setup_uses_the_first_proxy_with_a_server() {
        let groups = GroupRegistry::default();
        let mut ctx = UdpPortContext::new(&port_entry()).unwrap();
        let proxies = || {
            vec![
                proxy_entry("udpempty", &[]),
                proxy_entry("udpfirst", &[5353, 5354]),
                proxy_entry("udplast", &[5355]),
            ]
        };
        ctx.setup(proxies(), &groups).await.unwrap();
        ctx.setup(proxies(), &groups).await.unwrap();
        let ports = ctx
            .upstream
            .servers
            .iter()
            .map(|server| server.port)
            .collect::<Vec<_>>();
        assert_eq!(ports, [5353, 5354]);
    }

    #[tokio::test]
    async fn setup_closes_the_sessions_when_the_servers_change() {
        let listener = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let client: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let groups = GroupRegistry::default();
        let mut ctx = UdpPortContext::new(&port_entry()).unwrap();

        ctx.setup(vec![proxy_entry("udpses", &[5353])], &groups)
            .await
            .unwrap();
        let quarantine = AutoQuarantineTracker::default();
        ctx.forward(datagram(client, b"ping"), &listener, &quarantine)
            .await;
        assert_eq!(ctx.sessions.len(), 1);

        ctx.setup(vec![proxy_entry("udpses", &[5353])], &groups)
            .await
            .unwrap();
        assert_eq!(ctx.sessions.len(), 1);

        ctx.setup(vec![proxy_entry("udpses", &[5354])], &groups)
            .await
            .unwrap();
        assert!(ctx.sessions.is_empty());
    }

    /// Reads one datagram of the upstream as text, for up to two seconds.
    async fn recv_text(upstream: &UdpSocket) -> String {
        let mut buf = [0u8; 64];
        let wait = tokio::time::timeout(Duration::from_secs(2), upstream.recv(&mut buf));
        let len = wait.await.expect("no datagram").unwrap();
        String::from_utf8_lossy(&buf[..len]).into_owned()
    }

    /// The server quarantines a client on every port, so the upstream gets no datagram of a
    /// quarantined client, even on the session that the client opened before its quarantine.
    /// Another client keeps its service. The clients use documentation addresses (RFC 5737),
    /// because loopback is never quarantined.
    #[tokio::test]
    async fn a_quarantined_client_reaches_no_upstream_server() {
        let upstream = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let listener = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let attacker: SocketAddr = "203.0.113.5:4000".parse().unwrap();
        let other: SocketAddr = "198.51.100.6:4000".parse().unwrap();
        let quarantine = AutoQuarantineTracker::default();
        let mut ctx = UdpPortContext::new(&port_entry()).unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        ctx.setup(
            vec![proxy_entry("udpquar", &[upstream_port])],
            &GroupRegistry::default(),
        )
        .await
        .unwrap();

        ctx.forward(datagram(attacker, b"open"), &listener, &quarantine)
            .await;
        assert_eq!(recv_text(&upstream).await, "open");

        quarantine.quarantine(attacker.ip(), "test".into(), 60);
        ctx.forward(datagram(attacker, b"attack"), &listener, &quarantine)
            .await;
        ctx.forward(datagram(other, b"other"), &listener, &quarantine)
            .await;
        // The datagrams leave in this order, so the first one that arrives shows whether the
        // quarantined packet went through.
        assert_eq!(recv_text(&upstream).await, "other");
        assert!(!ctx.sessions.iter().any(|s| s.key().0 == attacker));
    }

    #[test]
    fn idle_time_counts_from_the_last_activity() {
        let activity = Activity {
            started: Instant::now() - Duration::from_secs(10),
            last_active_ms: AtomicU64::new(0),
            idle_timeout: Duration::from_secs(15),
        };
        assert!(activity.idle_time() >= Duration::from_secs(10));
        assert!(activity.remaining() <= Duration::from_secs(5));

        activity.touch();
        assert!(activity.idle_time() < Duration::from_secs(1));
        assert!(activity.remaining() > Duration::from_secs(14));
    }

    #[tokio::test]
    async fn resolve_times_out_at_deadline() {
        let resolver = DnsResolver::from_system_conf().unwrap();
        let span = Span::none();
        let server = Connection {
            name: ServerName::try_from("slow.invalid").unwrap().to_owned(),
            port: 53,
            span: PortSpan::single(53),
            addr: None,
            ttl: Instant::now(),
        };
        let deadline = Instant::now();
        let res = resolve(&resolver, &span, &server, 53, deadline).await;
        assert_eq!(res, None);
    }
}
