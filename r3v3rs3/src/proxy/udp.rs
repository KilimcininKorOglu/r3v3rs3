use super::{PortContextEvent, PortStatus, SocketState};
use hickory_resolver::config::LookupIpStrategy;
use hickory_resolver::name_server::{GenericConnector, TokioRuntimeProvider};
use hickory_resolver::system_conf::read_system_conf;
use hickory_resolver::AsyncResolver;
use r3v3rs3_api::upstream::DEFAULT_SESSION_IDLE_TIMEOUT;
use r3v3rs3_api::{error::Error, multiaddr::Multiaddr, proxy::ProxyKind};
use r3v3rs3_api::{port::PortEntry, proxy::ProxyEntry};
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::{net::SocketAddr, time::SystemTime};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio_rustls::rustls::pki_types::ServerName;
use tracing::{debug, error, info, span, warn, Level, Span};

type Resolver = AsyncResolver<GenericConnector<TokioRuntimeProvider>>;

const DNS_LOOKUP_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const MAX_SESSIONS: usize = 10_000;
const MAX_DATAGRAM_SIZE: usize = 65527;

#[derive(Debug)]
pub struct UdpPortContext {
    pub listen: SocketAddr,
    servers: Vec<Connection>,
    idle_timeout: Duration,
    sessions: HashMap<SocketAddr, UdpSession>,
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

        let (conf, mut opts) = read_system_conf().unwrap_or_default();
        opts.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        let resolver = AsyncResolver::tokio(conf, opts);

        let listen = entry.port.listen.socket_addr()?;
        Ok(Self {
            listen,
            servers: Default::default(),
            idle_timeout: DEFAULT_SESSION_IDLE_TIMEOUT,
            sessions: Default::default(),
            status: Default::default(),
            span,
            resolver,
        })
    }

    pub async fn setup(&mut self, proxies: Vec<ProxyEntry>) -> Result<(), Error> {
        let mut servers = Vec::new();
        let mut idle_timeout = DEFAULT_SESSION_IDLE_TIMEOUT;
        for proxy in proxies {
            if let ProxyKind::Udp(proxy) = proxy.proxy.kind {
                idle_timeout = proxy.session_idle_timeout;
                for server in proxy.upstream_servers {
                    servers.push(multiaddr_to_host(&server.addr)?);
                }
            }
        }
        // Keep the open sessions and the resolved addresses when the settings do not change.
        if !same_servers(&servers, &self.servers) || idle_timeout != self.idle_timeout {
            self.servers = servers;
            self.idle_timeout = idle_timeout;
            self.sessions.clear();
        }
        Ok(())
    }

    pub fn apply(&mut self, new: Self) {
        *self = new;
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
    /// server go back to the client through `listener`.
    pub async fn forward(&mut self, client: SocketAddr, data: &[u8], listener: &Arc<UdpSocket>) {
        let open = self.sessions.get(&client).is_some_and(UdpSession::is_open);
        if !open {
            let Some(session) = self.open_session(client, listener).await else {
                return;
            };
            self.sessions.insert(client, session);
        }
        let Some(session) = self.sessions.get(&client) else {
            return;
        };
        session.touch();
        if let Err(err) = session.socket.send(data).await {
            self.span.in_scope(|| {
                debug!(%client, %err, "failed to send the packet to the upstream server");
            });
        }
    }

    async fn open_session(
        &mut self,
        client: SocketAddr,
        listener: &Arc<UdpSocket>,
    ) -> Option<UdpSession> {
        self.sessions.retain(|_, session| session.is_open());
        if self.sessions.len() >= MAX_SESSIONS {
            self.span.in_scope(|| {
                warn!(%client, "dropping the packet: too many udp sessions");
            });
            return None;
        }
        let server = self.servers.first_mut()?;
        let upstream = resolve(&self.resolver, &self.span, server).await?;
        let activity = Arc::new(Activity::new(self.idle_timeout));
        match UdpSession::open(
            upstream,
            client,
            listener.clone(),
            activity,
            self.span.clone(),
        )
        .await
        {
            Ok(session) => Some(session),
            Err(err) => {
                self.span.in_scope(|| {
                    error!(%upstream, %err, "failed to open the upstream socket");
                });
                None
            }
        }
    }
}

fn same_servers(a: &[Connection], b: &[Connection]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.name == b.name && a.port == b.port)
}

/// Returns the address of the server. The address is resolved again when its DNS TTL expires.
async fn resolve(resolver: &Resolver, span: &Span, server: &mut Connection) -> Option<SocketAddr> {
    if !server.ttl.elapsed().is_zero() {
        let resolved = resolver.lookup_ip(server.name.to_str().as_ref()).await;
        let (resolved, ttl): (anyhow::Result<SocketAddr>, Instant) = match resolved {
            Ok(addrs) => match addrs.iter().next() {
                Some(addr) => (Ok(SocketAddr::new(addr, server.port)), addrs.valid_until()),
                None => (
                    Err(anyhow::anyhow!(
                        "no IP address found for {}",
                        server.name.to_str()
                    )),
                    addrs.valid_until(),
                ),
            },
            Err(e) => (Err(e.into()), Instant::now() + DNS_LOOKUP_RETRY_INTERVAL),
        };
        server.ttl = ttl;
        match resolved {
            Ok(addr) => server.addr = Some(addr),
            Err(e) => span.in_scope(|| {
                error!("failed to resolve {}: {}", server.name.to_str(), e);
            }),
        }
    }
    server.addr
}

/// The upstream socket of one client. Dropping the session stops its reply task.
#[derive(Debug)]
struct UdpSession {
    socket: Arc<UdpSocket>,
    activity: Arc<Activity>,
    task: JoinHandle<()>,
}

impl UdpSession {
    async fn open(
        upstream: SocketAddr,
        client: SocketAddr,
        listener: Arc<UdpSocket>,
        activity: Arc<Activity>,
        span: Span,
    ) -> io::Result<Self> {
        let bind: SocketAddr = if upstream.is_ipv6() {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        };
        let socket = UdpSocket::bind(bind).await?;
        socket.connect(upstream).await?;
        let socket = Arc::new(socket);
        let task = tokio::spawn(relay_replies(
            socket.clone(),
            listener,
            client,
            activity.clone(),
            span,
        ));
        Ok(Self {
            socket,
            activity,
            task,
        })
    }

    fn is_open(&self) -> bool {
        !self.task.is_finished()
    }

    fn touch(&self) {
        self.activity.touch();
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
/// idle timeout or the upstream socket fails.
async fn relay_replies(
    socket: Arc<UdpSocket>,
    listener: Arc<UdpSocket>,
    client: SocketAddr,
    activity: Arc<Activity>,
    span: Span,
) {
    let mut buf = vec![0; MAX_DATAGRAM_SIZE];
    loop {
        let remaining = activity.remaining();
        if remaining.is_zero() {
            break;
        }
        let size = match tokio::time::timeout(remaining, socket.recv(&mut buf)).await {
            Ok(Ok(size)) => size,
            Ok(Err(err)) => {
                span.in_scope(|| debug!(%client, %err, "closing the udp session"));
                break;
            }
            Err(_) => continue,
        };
        activity.touch();
        if let Err(err) = listener.send_to(&buf[..size], client).await {
            span.in_scope(|| debug!(%client, %err, "failed to send the reply to the client"));
        }
    }
}

fn multiaddr_to_host(addr: &Multiaddr) -> Result<Connection, Error> {
    match (addr.ip_addr(), addr.host(), addr.port()) {
        (Ok(addr), _, Ok(port)) => Ok(Connection {
            name: ServerName::IpAddress(addr.into()),
            port,
            addr: None,
            ttl: Instant::now(),
        }),
        (_, Ok(host), Ok(port)) => Ok(Connection {
            name: ServerName::try_from(host.as_str())
                .map_err(|_| Error::InvalidServerAddress { addr: addr.clone() })?
                .to_owned(),
            port,
            addr: None,
            ttl: Instant::now(),
        }),
        _ => Err(Error::InvalidServerAddress { addr: addr.clone() }),
    }
}

#[derive(Debug, Clone)]
pub struct Connection {
    pub name: ServerName<'static>,
    pub port: u16,
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
        }
    }

    fn proxy_entry(port: u16) -> ProxyEntry {
        ProxyEntry {
            id: "proxy".parse().unwrap(),
            proxy: Proxy {
                ports: vec![port_entry().id],
                kind: ProxyKind::Udp(UdpProxy {
                    upstream_servers: vec![UpstreamServer {
                        addr: format!("/ip4/127.0.0.1/udp/{port}").parse().unwrap(),
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            },
        }
    }

    #[tokio::test]
    async fn setup_rebuilds_the_server_list() {
        let mut ctx = UdpPortContext::new(&port_entry()).unwrap();
        ctx.setup(vec![proxy_entry(5353)]).await.unwrap();
        ctx.setup(vec![proxy_entry(5353)]).await.unwrap();
        assert_eq!(ctx.servers.len(), 1);
    }

    #[tokio::test]
    async fn setup_closes_the_sessions_when_the_servers_change() {
        let listener = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let client = "127.0.0.1:40000".parse().unwrap();
        let mut ctx = UdpPortContext::new(&port_entry()).unwrap();

        ctx.setup(vec![proxy_entry(5353)]).await.unwrap();
        ctx.forward(client, b"ping", &listener).await;
        assert_eq!(ctx.sessions.len(), 1);

        ctx.setup(vec![proxy_entry(5353)]).await.unwrap();
        assert_eq!(ctx.sessions.len(), 1);

        ctx.setup(vec![proxy_entry(5354)]).await.unwrap();
        assert!(ctx.sessions.is_empty());
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
}
