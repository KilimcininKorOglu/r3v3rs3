use super::{
    health::{self, Probe, UpstreamGroup},
    spawn_connection,
    tls::{port_acceptor, upstream_client_config, TlsTermination},
    PortContextEvent, PortStatus, SocketState,
};
use crate::server::cert_list::CertList;
use hickory_resolver::config::LookupIpStrategy;
use hickory_resolver::name_server::{GenericConnector, TokioRuntimeProvider};
use hickory_resolver::system_conf::read_system_conf;
use hickory_resolver::AsyncResolver;
use r3v3rs3_api::{error::Error, id::ShortId, multiaddr::Multiaddr};
use r3v3rs3_api::{
    port::PortEntry,
    proxy::{ProxyEntry, ProxyKind, TcpProxy},
};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime},
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpSocket, TcpStream},
    time::{timeout_at, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, BufStream},
    sync::Notify,
};
use tokio_rustls::rustls::pki_types::{IpAddr, ServerName};
use tokio_rustls::{rustls::ClientConfig, TlsAcceptor, TlsConnector};
use tracing::{debug, error, info, span, warn, Level, Span};

const MAX_BUFFER_SIZE: usize = 4096;

type Resolver = AsyncResolver<GenericConnector<TokioRuntimeProvider>>;

#[derive(Debug)]
pub struct TcpPortContext {
    pub listen: SocketAddr,
    upstream: Option<TcpUpstream>,
    status: PortStatus,
    span: Span,
    resolver: Resolver,
    tls_termination: Option<TlsTermination>,
    stop_notifier: Arc<Notify>,
}

impl TcpPortContext {
    pub fn new(entry: &PortEntry) -> Result<Self, Error> {
        let span = span!(Level::INFO, "proxy", resource_id = entry.id.to_string(), listen = %entry.port.listen);
        let enter = span.clone();
        let _enter = enter.enter();

        info!("initializing tcp proxy");

        let listen = entry.port.listen.socket_addr()?;
        let tls_termination = if let Some(tls) = &entry.port.opts.tls_termination {
            Some(TlsTermination::new(tls, vec![])?)
        } else if entry.port.listen.is_tls() {
            return Err(Error::TlsTerminationConfigMissing);
        } else {
            None
        };

        let (conf, mut opts) = read_system_conf().unwrap_or_default();
        opts.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        let resolver = AsyncResolver::tokio(conf, opts);

        Ok(Self {
            listen,
            upstream: None,
            status: Default::default(),
            span,
            resolver,
            tls_termination,
            stop_notifier: Arc::new(Notify::new()),
        })
    }

    /// Uses the upstream servers of the first proxy of the port that has a valid server.
    pub async fn setup(&mut self, certs: &CertList, proxies: Vec<ProxyEntry>) -> Result<(), Error> {
        let mut upstream = None;
        for entry in proxies {
            if let ProxyKind::Tcp(proxy) = &entry.proxy.kind {
                let servers = proxy_connections(certs, proxy)?;
                if upstream.is_none() && !servers.is_empty() {
                    upstream = Some(TcpUpstream::new(entry.id, proxy, servers));
                }
            }
        }
        self.upstream = upstream;

        if let Some(tls) = &mut self.tls_termination {
            self.status.state.tls = Some(tls.setup(certs).await);
        }
        Ok(())
    }

    pub fn apply(&mut self, new: Self) {
        *self = Self {
            stop_notifier: self.stop_notifier.clone(),
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
        self.stop_notifier.notify_waiters();
    }

    pub fn start_proxy(&mut self, mut stream: BufStream<TcpStream>) {
        let Some(upstream) = self.upstream.clone() else {
            tokio::spawn(async move { stream.get_mut().shutdown().await });
            return;
        };

        let Some(tls_acceptor) = port_acceptor(self.tls_termination.as_ref()) else {
            debug!("closing the connection: the TLS config is invalid");
            return;
        };
        let task = start(
            stream,
            upstream,
            self.resolver.clone(),
            tls_acceptor,
            self.stop_notifier.clone(),
        );
        spawn_connection(self.span.clone(), task);
    }
}

/// The upstream servers of the TCP proxy of a port.
#[derive(Debug, Clone)]
struct TcpUpstream {
    servers: Arc<[Connection]>,
    group: Arc<UpstreamGroup>,
}

impl TcpUpstream {
    fn new(id: ShortId, proxy: &TcpProxy, servers: Vec<Connection>) -> Self {
        let members = health::members(&proxy.upstream_servers);
        let group = health::group(
            (id, None),
            members,
            proxy.load_balancing,
            proxy.health_check.clone(),
            Probe::Connect(Probe::targets(&proxy.upstream_servers)),
        );
        Self {
            servers: servers.into(),
            group,
        }
    }

    /// Connects to the selected server. When the connection fails, it connects once to the next
    /// server.
    async fn connect(
        &self,
        resolver: &Resolver,
    ) -> anyhow::Result<(SocketAddr, Box<dyn IoStream>)> {
        let mut result = Err(anyhow::anyhow!("the proxy has no upstream server"));
        for index in self.group.candidates().into_iter().take(2) {
            let Some(conn) = self.servers.get(index) else {
                continue;
            };
            result = connect_server(conn, resolver).await;
            match &result {
                Ok(_) => {
                    self.group.report_success(index);
                    break;
                }
                Err(err) => {
                    self.group.report_failure(index, &err.to_string());
                    warn!(%err, "failed to connect to the upstream server");
                }
            }
        }
        result
    }
}

async fn start(
    mut stream: BufStream<TcpStream>,
    upstream: TcpUpstream,
    resolver: Resolver,
    tls_acceptor: Option<TlsAcceptor>,
    stop_notifier: Arc<Notify>,
) -> anyhow::Result<()> {
    let remote = stream.get_ref().peer_addr()?;
    let local = stream.get_ref().local_addr()?;

    let (mut client_stream, server_stream) = tokio::io::duplex(MAX_BUFFER_SIZE);
    tokio::spawn(async move {
        tokio::select! {
            result = tokio::io::copy_bidirectional(&mut stream, &mut client_stream) => {
                if let Err(err) = result {
                    error!("{err}");
                }
            },
            _ = stop_notifier.notified() => {
                debug!("stop");
            },
        }
    });

    let (target, mut out) = upstream.connect(&resolver).await?;
    info!(target: "r3v3rs3::access_log", remote = %remote, %local, %target);

    let mut stream: Box<dyn IoStream> = Box::new(server_stream);
    if let Some(acceptor) = tls_acceptor {
        debug!(%remote, "server: tls handshake");
        stream = Box::new(acceptor.accept(stream).await?);
    }

    if let Err(err) = tokio::io::copy_bidirectional(&mut stream, &mut out).await {
        error!("{err}");
    }

    stream.shutdown().await?;
    out.shutdown().await?;

    debug!(%target, "eof");
    Ok(())
}

/// Resolves and connects to the server. One deadline covers the DNS lookup, the TCP connection
/// and the TLS handshake.
async fn connect_server(
    conn: &Connection,
    resolver: &Resolver,
) -> anyhow::Result<(SocketAddr, Box<dyn IoStream>)> {
    let connect_timeout = conn.connect_timeout;
    let timed_out =
        || anyhow::anyhow!("connecting to the upstream server timed out after {connect_timeout:?}");
    let deadline = Instant::now() + connect_timeout;
    let target = timeout_at(deadline, resolve_upstream(conn, resolver))
        .await
        .map_err(|_| timed_out())??;
    let out = timeout_at(deadline, connect_upstream(conn.clone(), target))
        .await
        .map_err(|_| timed_out())??;
    Ok((target, out))
}

async fn resolve_upstream(conn: &Connection, resolver: &Resolver) -> anyhow::Result<SocketAddr> {
    let name = match &conn.name {
        ServerName::DnsName(name) => name.as_ref().to_string(),
        ServerName::IpAddress(addr) => match addr {
            IpAddr::V4(addr) => std::net::Ipv4Addr::from(*addr).to_string(),
            IpAddr::V6(addr) => std::net::Ipv6Addr::from(*addr).to_string(),
        },
        _ => unreachable!(),
    };
    let resolved = resolver
        .lookup_ip(&name)
        .await?
        .iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("No IP address found for {name}"))?;
    debug!(name, %resolved);
    Ok((resolved, conn.port).into())
}

/// Connects to the upstream server, and runs the TLS handshake of a TLS server.
async fn connect_upstream(
    conn: Connection,
    target: SocketAddr,
) -> anyhow::Result<Box<dyn IoStream>> {
    let sock = if target.is_ipv4() {
        TcpSocket::new_v4()
    } else {
        TcpSocket::new_v6()
    }?;
    let out = sock.connect(target).await?;
    debug!(%target, "connected");

    let Some(config) = conn.tls_client_config else {
        let out: Box<dyn IoStream> = Box::new(out);
        return Ok(out);
    };
    debug!(%target, "client: tls handshake");
    let tls = TlsConnector::from(config);
    let out: Box<dyn IoStream> = Box::new(tls.connect(conn.name, out).await?);
    Ok(out)
}

/// Resolves the upstream servers of the proxy. A proxy with an invalid client certificate gets no
/// servers, so it does not connect without the certificate.
fn proxy_connections(certs: &CertList, proxy: &TcpProxy) -> Result<Vec<Connection>, Error> {
    let config = match upstream_client_config(certs, proxy.client_cert) {
        Ok(config) => Arc::new(config),
        Err(err) => {
            error!(%err, "the proxy cannot connect to its upstream servers");
            return Ok(vec![]);
        }
    };
    proxy
        .upstream_servers
        .iter()
        .map(|server| multiaddr_to_host(&server.addr, &config, proxy.connect_timeout))
        .collect()
}

fn multiaddr_to_host(
    addr: &Multiaddr,
    config: &Arc<ClientConfig>,
    connect_timeout: Duration,
) -> Result<Connection, Error> {
    let tls_client_config = addr.is_tls().then(|| config.clone());
    let name = match (addr.ip_addr(), addr.host()) {
        (Ok(ip), _) => ServerName::IpAddress(ip.into()),
        (_, Ok(host)) => ServerName::try_from(host.as_str())
            .map_err(|_| Error::InvalidServerAddress { addr: addr.clone() })?
            .to_owned(),
        _ => return Err(Error::InvalidServerAddress { addr: addr.clone() }),
    };
    let port = addr
        .port()
        .map_err(|_| Error::InvalidServerAddress { addr: addr.clone() })?;
    Ok(Connection {
        name,
        port,
        tls_client_config,
        connect_timeout,
    })
}

trait IoStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<S> IoStream for S where S: AsyncRead + AsyncWrite + Unpin + Send {}

#[derive(Debug, Clone)]
pub struct Connection {
    pub name: ServerName<'static>,
    pub port: u16,
    /// The TLS client config for a TLS upstream server.
    pub tls_client_config: Option<Arc<ClientConfig>>,
    pub connect_timeout: Duration,
}
