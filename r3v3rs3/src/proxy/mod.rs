use self::{
    http::{HttpPortContext, HttpStarter, SessionService},
    tcp::{TcpPortContext, TcpStarter},
    udp::UdpPortContext,
};
use crate::server::cert_list::CertList;
use once_cell::sync::OnceCell;
use r3v3rs3_api::app::{Http2Limits, Http3Limits};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::multiaddr::Multiaddr;
use r3v3rs3_api::port::{PortStatus, SocketState};
use r3v3rs3_api::{
    port::{Port, PortEntry},
    proxy::ProxyEntry,
};
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{io::BufStream, net::TcpStream};
use tokio_rustls::rustls::ServerConfig;
use tracing::{Instrument, Span, error};

pub mod access_log;
pub mod dns;
pub mod geoip;
pub mod health;
pub mod http;
pub mod proxy_protocol;
pub mod registry;
pub mod srv;
pub mod stats;
pub mod tcp;
pub mod tls;
pub mod udp;

/// Builds the DNS resolver of a TCP or UDP port on the tokio runtime.
pub(crate) fn tokio_resolver(
    config: hickory_resolver::config::ResolverConfig,
    opts: hickory_resolver::config::ResolverOpts,
) -> anyhow::Result<hickory_resolver::TokioResolver> {
    let mut builder = hickory_resolver::Resolver::builder_with_config(
        config,
        hickory_resolver::net::runtime::TokioRuntimeProvider::default(),
    );
    *builder.options_mut() = opts;
    Ok(builder.build()?)
}

/// The rate limiters, HTTP caches, upstream groups and SRV targets of one server. A configuration
/// reload reuses them, and two servers in one process do not share them.
#[derive(Debug, Default)]
pub struct ProxyRegistries {
    pub limiters: http::rate_limit::LimiterRegistry,
    pub concurrency_limiters: http::concurrency_limit::ConcurrencyRegistry,
    pub caches: http::cache::CacheRegistry,
    pub groups: health::GroupRegistry,
    pub srv: srv::SrvRegistry,
    pub geoip: Arc<geoip::GeoIpResolver>,
    pub dns: dns::DnsResolver,
    pub quarantine: Arc<crate::security::AutoQuarantineTracker>,
    pub error_page_themes:
        Arc<arc_swap::ArcSwap<Vec<r3v3rs3_api::error_page_theme::ErrorPageTheme>>>,
}

/// The number of the client connection tasks that are still running. The graceful drain waits
/// for this count to reach zero.
static OPEN_CONNECTIONS: AtomicUsize = AtomicUsize::new(0);

/// Holds one open client connection. The count drops when the lease drops.
pub(crate) struct ConnectionLease;

impl ConnectionLease {
    pub(crate) fn acquire() -> Self {
        OPEN_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for ConnectionLease {
    fn drop(&mut self) {
        OPEN_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// The number of the client connections whose tasks still run.
pub(crate) fn open_connections() -> usize {
    OPEN_CONNECTIONS.load(Ordering::Relaxed)
}

/// Runs the task of a client connection in the span and logs its error.
fn spawn_connection<F>(span: Span, task: F)
where
    F: Future<Output = anyhow::Result<()>> + Send + 'static,
{
    let lease = ConnectionLease::acquire();
    tokio::spawn(
        async move {
            let _lease = lease;
            if let Err(err) = task.await {
                error!("{err}");
            }
        }
        .instrument(span),
    );
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortContextEvent {
    SocketStateUpdated(SocketState),
}

#[derive(Debug)]
pub struct PortContext {
    pub entry: PortEntry,
    pub kind: PortContextKind,
}

impl PortContext {
    /// Builds the context of a port. `global_http2` and `global_http3` are the config defaults
    /// that a port without its own limits options uses.
    pub fn new(
        entry: PortEntry,
        global_http2: &Http2Limits,
        global_http3: &Http3Limits,
    ) -> Result<Self, Error> {
        if let Some(proxy_protocol) = &entry.port.opts.proxy_protocol {
            proxy_protocol.validate(&entry.port.listen)?;
        }
        let kind = if entry.port.listen.is_quic() {
            PortContextKind::Http3(HttpPortContext::new(&entry, global_http2, global_http3)?)
        } else if entry.port.listen.is_udp() {
            PortContextKind::Udp(UdpPortContext::new(&entry)?)
        } else if entry.port.listen.is_http() {
            PortContextKind::Http(HttpPortContext::new(&entry, global_http2, global_http3)?)
        } else {
            PortContextKind::Tcp(TcpPortContext::new(&entry)?)
        };
        Ok(Self { entry, kind })
    }

    /// The effective HTTP/3 stream limits of this port.
    pub fn http3_limits(&self) -> Option<&Http3Limits> {
        match &self.kind {
            PortContextKind::Http3(ctx) => Some(ctx.http3_limits()),
            _ => None,
        }
    }

    /// The effective HTTP/2 flood limits of this port.
    pub fn http2_limits(&self) -> Option<&Http2Limits> {
        match &self.kind {
            PortContextKind::Http(ctx) => Some(ctx.http2_limits()),
            PortContextKind::Http3(ctx) => Some(ctx.http2_limits()),
            _ => None,
        }
    }

    pub fn reserved() -> Self {
        Self {
            entry: PortEntry {
                id: "reserved".parse().unwrap(),
                port: Port {
                    active: true,
                    name: String::new(),
                    listen: Multiaddr::default(),
                    opts: Default::default(),
                },
            },
            kind: PortContextKind::Reserved,
        }
    }

    pub fn entry(&self) -> &PortEntry {
        &self.entry
    }

    pub fn kind(&self) -> &PortContextKind {
        &self.kind
    }

    pub fn kind_mut(&mut self) -> &mut PortContextKind {
        &mut self.kind
    }

    /// Starts the client connections of a TCP or HTTP port. `None` for another port.
    pub fn starter(&self) -> Option<ConnectionStarter> {
        match &self.kind {
            PortContextKind::Tcp(ctx) => Some(ConnectionStarter::Tcp(Box::new(ctx.starter()))),
            PortContextKind::Http(ctx) => Some(ConnectionStarter::Http(ctx.starter())),
            _ => None,
        }
    }

    pub async fn setup(
        &mut self,
        ports: &[PortEntry],
        certs: &CertList,
        proxies: Vec<ProxyEntry>,
        sessions: &std::sync::Arc<SessionService>,
        registries: &ProxyRegistries,
    ) -> Result<(), Error> {
        match &mut self.kind {
            PortContextKind::Tcp(ctx) => ctx.setup(certs, proxies, &registries.groups).await,
            PortContextKind::Http(ctx) => {
                ctx.setup(ports, certs, proxies, sessions, registries).await
            }
            PortContextKind::Udp(ctx) => ctx.setup(proxies, &registries.groups).await,
            PortContextKind::Http3(ctx) => {
                ctx.setup(ports, certs, proxies, sessions, registries).await
            }
            PortContextKind::Reserved => Ok(()),
        }
    }

    pub fn apply(&mut self, new: Self) {
        match (&mut self.kind, new.kind) {
            (PortContextKind::Tcp(old), PortContextKind::Tcp(new)) => old.apply(new),
            (PortContextKind::Udp(old), PortContextKind::Udp(new)) => old.apply(new),
            (PortContextKind::Http(old), PortContextKind::Http(new)) => old.apply(new),
            (PortContextKind::Http3(old), PortContextKind::Http3(new)) => old.apply(new),
            (old, new) => *old = new,
        }
        self.entry = new.entry;
    }

    pub fn event(&mut self, event: PortContextEvent) {
        match &mut self.kind {
            PortContextKind::Tcp(ctx) => ctx.event(event),
            PortContextKind::Udp(ctx) => ctx.event(event),
            PortContextKind::Http(ctx) => ctx.event(event),
            PortContextKind::Http3(ctx) => ctx.event(event),
            PortContextKind::Reserved => (),
        }
    }

    pub fn status(&self) -> &PortStatus {
        match &self.kind {
            PortContextKind::Tcp(ctx) => ctx.status(),
            PortContextKind::Udp(ctx) => ctx.status(),
            PortContextKind::Http(ctx) => ctx.status(),
            PortContextKind::Http3(ctx) => ctx.status(),
            PortContextKind::Reserved => {
                static STATUS: OnceCell<PortStatus> = OnceCell::new();
                STATUS.get_or_init(PortStatus::default)
            }
        }
    }

    pub fn reset(&mut self) {
        match &mut self.kind {
            PortContextKind::Tcp(ctx) => ctx.reset(),
            PortContextKind::Udp(ctx) => ctx.reset(),
            PortContextKind::Http(ctx) => ctx.reset(),
            PortContextKind::Http3(ctx) => ctx.reset(),
            PortContextKind::Reserved => (),
        }
    }
}

/// Starts a client connection of a TCP or HTTP port. It holds a copy of the port settings, so the
/// connection can start in another task.
pub enum ConnectionStarter {
    /// Boxed, because the DNS resolver makes this variant much larger than the other.
    Tcp(Box<TcpStarter>),
    Http(HttpStarter),
}

impl ConnectionStarter {
    /// `challenge` is the TLS config of the active TLS-ALPN-01 challenges.
    pub fn start(self, stream: BufStream<TcpStream>, challenge: Option<Arc<ServerConfig>>) {
        match self {
            Self::Tcp(starter) => starter.start(stream, challenge),
            Self::Http(starter) => starter.start(stream, challenge),
        }
    }
}

#[derive(Debug)]
pub enum PortContextKind {
    Tcp(TcpPortContext),
    Udp(UdpPortContext),
    Http(HttpPortContext),
    Http3(HttpPortContext),
    Reserved,
}
