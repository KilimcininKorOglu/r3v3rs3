use self::{
    auth::{AuthContext, AuthRejection},
    cache::CacheRequest,
    compression::ResponseCompression,
    error::ProxyError,
    header_rules::{new_request_id, HeaderVariables},
    page::PagePreferences,
    pool::{Upstream, UpstreamClients, UpstreamH2c},
    route::{FilteredRoute, Router},
};
use super::{
    spawn_connection,
    tls::{port_acceptor, server_cert_resolver, ClientCertInfo, TlsTermination},
    PortContextEvent,
};
use crate::server::cert_list::CertList;
use arc_swap::{ArcSwap, Cache};
use bytes::{Buf, Bytes};
use futures::{Stream, StreamExt};
use h3::{quic::BidiStream, server::RequestStream};
use http_body_util::{combinators::BoxBody, BodyExt, BodyStream, Full, StreamBody};
use hyper::{
    body::{Body, Frame, Incoming},
    header::{AUTHORIZATION, HOST, LOCATION},
    http::{
        uri::{Parts, Scheme},
        HeaderValue,
    },
    service::service_fn,
    Request, Response, StatusCode, Uri,
};
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto,
};
use quinn::{
    crypto::rustls::QuicServerConfig,
    rustls::{pki_types::CertificateDer, server::WebPkiClientVerifier, ServerConfig},
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::port::{PortStatus, SocketState};
use r3v3rs3_api::{port::PortEntry, proxy::ProxyEntry};
use rewriter::{RequestRewriter, ResponseRewriter, ResponseRewriterBuilder};
use std::{net::SocketAddr, ops::ControlFlow, sync::Arc, time::SystemTime};
use tokio::{
    io::{AsyncRead, AsyncWrite, BufStream},
    sync::Notify,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{rustls::pki_types::ServerName, TlsAcceptor};
use tracing::{debug, error, info, span, Instrument, Level, Span};

mod affinity;
mod auth;
mod body_limit;
pub(crate) mod cache;
pub(crate) mod client_ip;
mod compression;
mod cookie;
mod error;
mod filter;
mod header_rules;
pub(crate) mod hyper_tls;
mod page;
pub(crate) mod pool;
mod rate_limit;
mod redirect;
mod rewrite;
mod rewriter;
mod route;

pub use auth::SessionService;

const MAX_BUFFER_SIZE: usize = 4096;
const HTTP2_MAX_FRAME_SIZE: usize = 16384;

#[derive(Debug)]
pub struct HttpPortContext {
    pub listen: SocketAddr,
    status: PortStatus,
    span: Span,
    tls_termination: Option<TlsTermination>,
    h3_server_config: Option<Arc<quinn::ServerConfig>>,
    shared: Arc<ArcSwap<SharedContext>>,
    stop_notifier: Arc<Notify>,
}

impl HttpPortContext {
    pub fn new(entry: &PortEntry) -> Result<Self, Error> {
        let span = span!(
            Level::INFO,
            "proxy",
            resource_id = entry.id.to_string(),
            listen = %entry.port.listen
        );
        let enter = span.clone();
        let _enter = enter.enter();

        info!("initializing http proxy");
        let listen = entry.port.listen.socket_addr()?;

        let tls_termination = if let Some(tls) = &entry.port.opts.tls_termination {
            let alpn = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            Some(TlsTermination::new(tls, alpn)?)
        } else if entry.port.listen.is_tls() {
            return Err(Error::TlsTerminationConfigMissing);
        } else {
            None
        };

        Ok(Self {
            listen,
            status: Default::default(),
            span,
            tls_termination,
            h3_server_config: None,
            shared: Arc::new(ArcSwap::from_pointee(SharedContext {
                router: Default::default(),
                header_rewriter: Default::default(),
            })),
            stop_notifier: Arc::new(Notify::new()),
        })
    }

    pub async fn setup(
        &mut self,
        ports: &[PortEntry],
        certs: &CertList,
        proxies: Vec<ProxyEntry>,
        sessions: &Arc<SessionService>,
    ) -> Result<(), Error> {
        let https_ports = ports
            .iter()
            .filter(|entry| entry.port.listen.is_http() && entry.port.listen.is_tls())
            .filter(|entry| {
                proxies
                    .iter()
                    .any(|proxy| proxy.proxy.ports.contains(&entry.id))
            })
            .collect::<Vec<_>>();
        let https_port = if self.listen.is_ipv4() {
            https_ports.iter().find(|entry| {
                entry
                    .port
                    .listen
                    .ip_addr()
                    .map(|ip| ip.is_ipv4())
                    .unwrap_or_default()
            })
        } else {
            https_ports.iter().find(|entry| {
                entry
                    .port
                    .listen
                    .ip_addr()
                    .map(|ip| ip.is_ipv6())
                    .unwrap_or_default()
            })
        }
        .or(https_ports.first())
        .and_then(|entry| entry.port.listen.port().ok());

        let quic_ports = ports
            .iter()
            .filter(|entry| entry.port.listen.is_http() && entry.port.listen.is_udp())
            .filter(|entry| {
                proxies
                    .iter()
                    .any(|proxy| proxy.proxy.ports.contains(&entry.id))
            })
            .collect::<Vec<_>>();

        let quic_port = if self.listen.is_ipv4() {
            quic_ports.iter().find(|entry| {
                entry
                    .port
                    .listen
                    .ip_addr()
                    .map(|ip| ip.is_ipv4())
                    .unwrap_or_default()
            })
        } else {
            quic_ports.iter().find(|entry| {
                entry
                    .port
                    .listen
                    .ip_addr()
                    .map(|ip| ip.is_ipv6())
                    .unwrap_or_default()
            })
        }
        .or(quic_ports.first())
        .and_then(|entry| entry.port.listen.port().ok());

        let mut upstream = UpstreamClients::new(certs)?;
        self.shared.store(Arc::new(SharedContext {
            router: Router::new(proxies, https_port, quic_port, &mut upstream, sessions),
            header_rewriter: RequestRewriter::builder()
                .set_via(HeaderValue::from_static("r3v3rs3"))
                .build(),
        }));

        if let Some(tls) = &mut self.tls_termination {
            self.status.state.tls = Some(tls.setup(certs).await);
        }

        self.h3_server_config = h3_server_config(certs, self.tls_termination.as_ref());
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

    pub fn start_proxy(&mut self, stream: BufStream<TcpStream>) {
        let Some(tls_acceptor) = port_acceptor(self.tls_termination.as_ref()) else {
            debug!("closing the connection: the TLS config is invalid");
            return;
        };
        let task = start(
            stream,
            tls_acceptor,
            Cache::new(Arc::clone(&self.shared)),
            self.stop_notifier.clone(),
            self.span.clone(),
        );
        spawn_connection(self.span.clone(), task);
    }

    async fn accept_quic(
        conn: quinn::Incoming,
        server_config: Arc<quinn::ServerConfig>,
    ) -> anyhow::Result<quinn::Connection> {
        Ok(conn.accept_with(server_config)?.await?)
    }

    pub fn start_quic_proxy(&mut self, conn: quinn::Incoming) {
        let span = self.span.clone();
        let stop_notifier = self.stop_notifier.clone();
        let span_cloned = span.clone();
        let shared_cache = Cache::new(Arc::clone(&self.shared));

        let server_config = if let Some(config) = &self.h3_server_config {
            config.clone()
        } else {
            return;
        };

        tokio::spawn(
            async move {
                match Self::accept_quic(conn, server_config).await {
                    Ok(conn) => {
                        let local = conn.local_ip();
                        let remote = conn.remote_address();
                        let client_cert = quic_client_cert(&conn);
                        let h3_conn = h3::server::Connection::<_, Bytes>::new(
                            h3_quinn::Connection::new(conn),
                        )
                        .await;
                        match h3_conn {
                            Ok(mut conn) => loop {
                                match conn.accept().await {
                                    Ok(Some((req, stream))) => {
                                        if let Err(err) = start_quic(
                                            req,
                                            stream,
                                            shared_cache.clone(),
                                            QuickContext {
                                                local,
                                                remote,
                                                client_cert: client_cert.clone(),
                                            },
                                            span_cloned.clone(),
                                            stop_notifier.clone(),
                                        )
                                        .await
                                        {
                                            error!("{err}");
                                        }
                                    }
                                    Ok(None) => break,
                                    Err(err) => {
                                        error!("{err}");
                                        break;
                                    }
                                }
                            },
                            Err(err) => {
                                error!("{err}");
                            }
                        }
                    }
                    Err(err) => {
                        error!("{err}");
                    }
                }
            }
            .instrument(span),
        );
    }
}

/// Builds the HTTP/3 server config with the client authentication of the port. `None` means that
/// the client authentication config is invalid, so the port refuses QUIC connections.
fn h3_server_config(
    certs: &CertList,
    tls: Option<&TlsTermination>,
) -> Option<Arc<quinn::ServerConfig>> {
    let verifier = match tls {
        Some(tls) => tls.client_verifier(certs).ok()?,
        None => WebPkiClientVerifier::no_client_auth(),
    };
    let mut tls_config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(server_cert_resolver(certs, vec![]));
    tls_config.max_early_data_size = u32::MAX;
    tls_config.alpn_protocols = vec!["h3".into()];

    QuicServerConfig::try_from(tls_config)
        .ok()
        .map(|config| Arc::new(quinn::ServerConfig::with_crypto(Arc::new(config))))
}

/// Reads the verified client certificate of a QUIC connection.
fn quic_client_cert(conn: &quinn::Connection) -> Option<Arc<ClientCertInfo>> {
    let chain = conn
        .peer_identity()?
        .downcast::<Vec<CertificateDer<'static>>>()
        .ok()?;
    ClientCertInfo::from_chain(Some(chain.as_slice())).map(Arc::new)
}

async fn start(
    mut stream: BufStream<TcpStream>,
    tls_acceptor: Option<TlsAcceptor>,
    shared_cache: Cache<Arc<ArcSwap<SharedContext>>, Arc<SharedContext>>,
    stop_notifier: Arc<Notify>,
    span: Span,
) -> anyhow::Result<()> {
    let local = stream.get_ref().local_addr()?;
    let remote = stream.get_ref().peer_addr()?;
    let (mut client_stream, server_stream) = tokio::io::duplex(MAX_BUFFER_SIZE);

    let first_byte = stream.read_u8().await?;
    client_stream.write_u8(first_byte).await?;

    tokio::spawn(
        async move {
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
        }
        .instrument(span.clone()),
    );

    if tls_acceptor.is_some() && local.port() != 80 && first_byte != 0x16 {
        tokio::task::spawn(
            async move {
                let server_stream = TokioIo::new(server_stream);
                if let Err(err) = auto::Builder::new(TokioExecutor::new())
                    .serve_connection(server_stream, service_fn(redirect))
                    .await
                {
                    error!("Failed to serve the connection: {:?}", err);
                }
            }
            .instrument(span.clone()),
        );
        return Ok(());
    }

    let mut stream: Box<dyn IoStream> = Box::new(server_stream);
    let mut server_http2 = false;
    let mut sni = None;
    let mut client_cert = None;

    let forwarded_proto = if tls_acceptor.is_some() {
        "https"
    } else {
        "http"
    };

    if let Some(acceptor) = tls_acceptor {
        debug!(%remote, "server: tls handshake");
        let accepted = acceptor.accept(stream).await?;
        let tls_conn = &accepted.get_ref().1;
        server_http2 = tls_conn.alpn_protocol() == Some(b"h2");
        sni = tls_conn.server_name().map(|sni| sni.to_string());
        client_cert = ClientCertInfo::from_chain(tls_conn.peer_certificates()).map(Arc::new);
        stream = Box::new(accepted);
    }

    let span_cloned = span.clone();
    let service = hyper::service::service_fn(move |req: Request<Incoming>| {
        let mut shared_cache = shared_cache.clone();
        let span = span_cloned.clone();
        let enter = span.clone();
        let _enter = enter.enter();

        let shared = shared_cache.load().clone();
        let sni = sni.clone();
        let client_cert = client_cert.clone();

        async move {
            let (req, response_rewriter) = if is_domain_fronting(&req, sni.as_deref()) {
                (
                    ProxiedRequest::Err(ProxyError::DomainFrontingDetected),
                    ResponseRewriter::builder(),
                )
            } else {
                let info = RequestInfo {
                    remote,
                    local: local.to_string(),
                    sni: sni.as_deref(),
                    proto: forwarded_proto,
                    client_cert: client_cert.as_deref(),
                };
                route_request(&shared, req, &info).await
            };
            response_rewriter.build().map_response(match req {
                ProxiedRequest::Ok(req, upstream, span, cache_request) => {
                    let req = req.map(|b| BoxBody::new(b.map_err(Into::into)));
                    cache::fetch(&upstream, req, cache_request)
                        .instrument(span)
                        .await
                }
                ProxiedRequest::Respond(resp) => {
                    Ok(resp.map(|b| BoxBody::new(b.map_err(Into::into))))
                }
                ProxiedRequest::Err(err) => Err(err.into()),
            })
        }
        .instrument(span)
    });

    tokio::task::spawn(
        async move {
            let stream = TokioIo::new(stream);
            let builder = auto::Builder::new(TokioExecutor::new());
            let builder = if server_http2 {
                builder.http2_only()
            } else {
                builder
            };
            let http = builder.serve_connection_with_upgrades(stream, service);
            if let Err(err) = http.await {
                error!("Failed to serve the connection: {:?}", err);
            }
        }
        .instrument(span.clone()),
    );

    Ok(())
}

enum ProxiedRequest<R> {
    /// A request for the upstream server with the upstream connections of the route, and its
    /// cache state when the proxy has a cache.
    Ok(R, Upstream, Span, Option<CacheRequest>),
    /// A response that r3v3rs3 sends without contacting the upstream server.
    Respond(Response<Full<Bytes>>),
    Err(ProxyError),
}

#[derive(Debug)]
struct SharedContext {
    pub router: Router,
    pub header_rewriter: RequestRewriter,
}

pub trait IoStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<S> IoStream for S where S: AsyncRead + AsyncWrite + Unpin + Send {}

#[derive(Debug, Clone)]
pub struct Connection {
    pub name: ServerName<'static>,
    pub port: u16,
    pub tls: bool,
}

async fn redirect(req: hyper::Request<Incoming>) -> Result<Response<String>, hyper::http::Error> {
    if let Ok(uri) = get_secure_uri(&req) {
        Response::builder()
            .header("Location", uri.to_string())
            .status(StatusCode::PERMANENT_REDIRECT)
            .body(String::new())
    } else {
        Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body(String::from("TLS required\r\n"))
    }
}

fn get_secure_uri(req: &hyper::Request<Incoming>) -> anyhow::Result<Uri> {
    let mut parts = req.uri().clone().into_parts();
    if let Some(host) = req.headers().get(HOST) {
        parts.authority = Some(host.to_str()?.parse()?);
    }
    parts.scheme = Some(Scheme::HTTPS);
    Ok(Uri::from_parts(parts)?)
}

struct QuickContext {
    local: Option<std::net::IpAddr>,
    remote: SocketAddr,
    client_cert: Option<Arc<ClientCertInfo>>,
}

/// Connection details that the TCP and QUIC paths pass to [`route_request`].
struct RequestInfo<'a> {
    remote: SocketAddr,
    local: String,
    sni: Option<&'a str>,
    proto: &'static str,
    /// The verified client certificate of the TLS connection.
    client_cert: Option<&'a ClientCertInfo>,
}

fn header_host<B>(req: &Request<B>) -> Option<&str> {
    req.headers()
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|host| host.split(':').next())
}

fn is_domain_fronting<B>(req: &Request<B>, sni: Option<&str>) -> bool {
    match (sni, header_host(req)) {
        (Some(sni), Some(header)) => !sni.eq_ignore_ascii_case(header),
        _ => false,
    }
}

/// Matches the request to a route, applies the route policies and prepares
/// the request for the upstream server.
async fn route_request<B>(
    shared: &SharedContext,
    req: Request<B>,
    info: &RequestInfo<'_>,
) -> (ProxiedRequest<Request<B>>, ResponseRewriterBuilder)
where
    B: Body<Data = Bytes>,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let preferences = PagePreferences::from_headers(req.headers());
    let response_rewriter = ResponseRewriter::builder().preferences(preferences);
    let header_host = header_host(&req).map(str::to_string);
    let request_host = header_host
        .clone()
        .or_else(|| info.sni.map(str::to_string))
        .or_else(|| req.uri().host().map(str::to_string));
    let Some((res, route)) = shared.router.get_route(&req, request_host.as_deref()) else {
        return (
            ProxiedRequest::Err(ProxyError::NoRouteFound),
            response_rewriter,
        );
    };
    let response_rewriter = response_rewriter
        .https_port(route.https_port)
        .quic_port(route.quic_port)
        .compression(
            route
                .compression
                .clone()
                .map(|config| ResponseCompression::new(config, req.method(), req.headers())),
        );

    let resource_id = route.resource_id;
    let client = route
        .client_ip
        .resolve(info.remote.ip(), req.headers(), &crate::cdn::table());
    let action = format!("{} {}", req.method().as_str(), req.uri());

    let upstream = match check_policies(route, client.ip) {
        Ok(upstream) => upstream,
        Err(err) => {
            info!(target: "r3v3rs3::access_log", %resource_id, remote = %info.remote, client = %client.ip, local = %info.local, action, error = %err);
            return (ProxiedRequest::Err(err), response_rewriter);
        }
    };

    let redirect = upgrade_redirect(route, &req, header_host.as_deref(), info.proto)
        .or_else(|| redirect::redirect_rule(&route.redirects, request_host.as_deref(), &req));
    if let Some(redirect) = redirect {
        return (ProxiedRequest::Respond(redirect), response_rewriter);
    }

    if body_limit::declared_too_large(req.headers(), upstream.max_body_size) {
        let err = ProxyError::PayloadTooLarge;
        info!(target: "r3v3rs3::access_log", %resource_id, remote = %info.remote, client = %client.ip, local = %info.local, action, error = %err);
        return (ProxiedRequest::Err(err), response_rewriter);
    }

    // Authentication removes the credentials, so the cache reads them before. The cache key uses
    // the client host and path, so the selected upstream server does not split the cache.
    let authorized = req.headers().contains_key(AUTHORIZATION);
    let cache_key = format!(
        "{} {}",
        request_host.as_deref().unwrap_or_default(),
        req.uri().path_and_query().map_or("/", |path| path.as_str())
    );
    let auth_ctx = AuthContext {
        client: client.ip,
        host: request_host.as_deref(),
        proto: info.proto,
        base_path: &route.base_path,
        path_segments: &res.path_segments,
        preferences,
    };
    let mut req = match authenticate(route, req, &auth_ctx).await {
        Authenticated::Pass(req) => req,
        Authenticated::Respond(response) => {
            return (ProxiedRequest::Respond(response), response_rewriter)
        }
        Authenticated::Rejected(rejection) => {
            info!(target: "r3v3rs3::access_log", %resource_id, remote = %info.remote, client = %client.ip, local = %info.local, action, error = %rejection);
            return (rejected(rejection), response_rewriter);
        }
    };

    let secure = info.proto != "http";
    let sticky_cookie = upstream.select(&mut req, res.path_segments, client.ip, secure);
    let response_rewriter = response_rewriter.sticky_cookie(sticky_cookie);
    if route.h2c {
        req.extensions_mut().insert(UpstreamH2c);
    }

    info!(target: "r3v3rs3::access_log", remote = %info.remote, client = %client.ip, local = %info.local, action, target = %req.uri());
    let span: Span = span!(Level::INFO, "http", %resource_id, remote = %info.remote, client = %client.ip, local = %info.local, action, target = %req.uri());

    shared
        .header_rewriter
        .pre_process(req.headers_mut(), &client, header_host, info.proto);
    shared.header_rewriter.post_process(req.headers_mut());
    let response_rewriter = apply_header_rules(
        route,
        req.headers_mut(),
        client.ip,
        request_host,
        info,
        response_rewriter,
    );
    let cache_request = route
        .cache
        .as_ref()
        .and_then(|cache| CacheRequest::new(cache, &mut req, cache_key, authorized));
    (
        ProxiedRequest::Ok(req, upstream, span, cache_request),
        response_rewriter,
    )
}

/// Applies the request header rules of the route and hands the response header rules to the
/// response rewriter. Request rules run after the forwarded headers, so they can replace them.
fn apply_header_rules(
    route: &FilteredRoute,
    headers: &mut hyper::HeaderMap,
    client: std::net::IpAddr,
    host: Option<String>,
    info: &RequestInfo<'_>,
    response_rewriter: ResponseRewriterBuilder,
) -> ResponseRewriterBuilder {
    if route.header_rules.is_empty() {
        return response_rewriter;
    }
    let client_cert = info.client_cert.cloned().unwrap_or_default();
    let variables = HeaderVariables {
        client_ip: client,
        host: host.unwrap_or_default(),
        scheme: if info.proto == "http" {
            "http"
        } else {
            "https"
        },
        request_id: new_request_id(),
        route: if route.base_path.is_empty() {
            "/".to_string()
        } else {
            route.base_path.clone()
        },
        client_cert_subject: client_cert.subject,
        client_cert_fingerprint: client_cert.fingerprint,
    };
    route.header_rules.apply_request(headers, &variables);
    response_rewriter.header_rules(route.header_rules.clone(), variables)
}

/// Applies the client IP filter and the rate limit of the route, and returns the upstream
/// connections of the route.
fn check_policies(route: &FilteredRoute, client: std::net::IpAddr) -> Result<Upstream, ProxyError> {
    if !route.ip_filter.allows(client) {
        return Err(ProxyError::IpNotAllowed);
    }
    if let Some(retry_after) = route
        .rate_limiter
        .as_ref()
        .and_then(|limiter| limiter.check(client).err())
    {
        return Err(ProxyError::TooManyRequests { retry_after });
    }
    route
        .upstream
        .clone()
        .ok_or(ProxyError::UpstreamClientCertInvalid)
}

enum Authenticated<B> {
    Pass(Request<B>),
    /// A response of the authenticator itself, like the sign-in page.
    Respond(Response<Full<Bytes>>),
    Rejected(AuthRejection),
}

/// Authenticates the request when the route requires authentication. The HTTPS redirect runs
/// first, so a browser sends the credentials on the secure connection.
async fn authenticate<B>(
    route: &FilteredRoute,
    req: Request<B>,
    ctx: &AuthContext<'_>,
) -> Authenticated<B>
where
    B: Body<Data = Bytes>,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    let Some(auth) = &route.auth else {
        return Authenticated::Pass(req);
    };
    let mut req = match auth.serve(req, ctx).await {
        ControlFlow::Continue(req) => req,
        ControlFlow::Break(response) => return Authenticated::Respond(response),
    };
    match auth.authorize(&mut req, ctx).await {
        Ok(()) => Authenticated::Pass(req),
        Err(rejection) => Authenticated::Rejected(rejection),
    }
}

/// Converts a failed authentication into the response for the client.
fn rejected<R>(rejection: AuthRejection) -> ProxiedRequest<R> {
    match rejection {
        AuthRejection::Error(err) => ProxiedRequest::Err(err),
        AuthRejection::Response(res) => ProxiedRequest::Respond(*res),
    }
}

/// Builds the HTTPS redirect for a plain HTTP request when the route upgrades insecure requests.
fn upgrade_redirect<B>(
    route: &FilteredRoute,
    req: &Request<B>,
    header_host: Option<&str>,
    proto: &str,
) -> Option<Response<Full<Bytes>>> {
    if proto != "http" || !route.upgrade_insecure {
        return None;
    }
    let port = route.https_port?;
    let mut parts = Parts::from(header_host?.parse::<Uri>().ok()?);
    parts.scheme = Some(Scheme::HTTPS);
    if let Some(authority) = parts.authority {
        parts.authority = format!("{}:{}", authority.host(), port).parse().ok();
    }
    parts.path_and_query = req.uri().path_and_query().cloned();
    let uri = Uri::from_parts(parts).ok()?;
    Response::builder()
        .status(301)
        .header(LOCATION, uri.to_string())
        .body(Full::new(Bytes::new()))
        .ok()
}

async fn start_quic<T>(
    req: Request<()>,
    stream: RequestStream<T, Bytes>,
    mut shared_cache: Cache<Arc<ArcSwap<SharedContext>>, Arc<SharedContext>>,
    ctx: QuickContext,
    span: Span,
    stop_notifier: Arc<Notify>,
) -> anyhow::Result<()>
where
    T: BidiStream<Bytes> + Send + 'static,
    <T as BidiStream<Bytes>>::RecvStream: Send + Sync,
{
    let enter = span.clone();
    let _enter = enter.enter();

    let shared = shared_cache.load();
    let info = RequestInfo {
        remote: ctx.remote,
        local: ctx.local.map(|ip| ip.to_string()).unwrap_or_default(),
        sni: None,
        proto: "h3",
        client_cert: ctx.client_cert.as_deref(),
    };
    // The body is attached before routing, so an authenticator can read a form submission.
    let (mut send, recv) = stream.split();
    let body = BoxBody::new(StreamBody::new(StreamWrapper::<T> { stream: recv }));
    let (req, response_rewriter) = route_request(shared, req.map(|()| body), &info).await;

    let res = match req {
        ProxiedRequest::Ok(req, upstream, span, cache_request) => {
            cache::fetch(&upstream, req, cache_request)
                .instrument(span)
                .await
        }
        ProxiedRequest::Respond(resp) => Ok(resp.map(|b| BoxBody::new(b.map_err(Into::into)))),
        ProxiedRequest::Err(err) => Err(err.into()),
    };
    let (parts, body) = response_rewriter.build().map_response(res)?.into_parts();
    let mut res = Response::from_parts(parts, ());
    res.headers_mut().remove("transfer-encoding");
    let mut res_stream = BodyStream::new(body);

    send.send_response(res).await?;

    loop {
        tokio::select! {
            frame = res_stream.next() => {
                if let Some(Ok(frame)) = frame {
                    match frame.into_data() {
                        Ok(data) => {
                            send.send_data(data).await?;
                        }
                        Err(frame) => {
                            if let Ok(trailers) = frame.into_trailers() {
                                send.send_trailers(trailers).await?;
                            }
                        }
                    }
                } else {
                    break;
                }
            },
            _ = stop_notifier.notified() => {
                debug!("stop");
            },
        }
    }

    Ok(send.finish().await?)
}

struct StreamWrapper<T: BidiStream<Bytes>> {
    stream: RequestStream<T::RecvStream, Bytes>,
}

impl<T: BidiStream<Bytes>> Stream for StreamWrapper<T> {
    type Item = Result<Frame<Bytes>, anyhow::Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context,
    ) -> std::task::Poll<Option<Self::Item>> {
        match self.stream.poll_recv_data(cx) {
            std::task::Poll::Ready(Ok(Some(mut data))) => {
                std::task::Poll::Ready(Some(Ok(Frame::data(data.copy_to_bytes(data.remaining())))))
            }
            std::task::Poll::Ready(Ok(None)) => std::task::Poll::Ready(None),
            std::task::Poll::Ready(Err(err)) => std::task::Poll::Ready(Some(Err(err.into()))),
            std::task::Poll::Pending => std::task::Poll::Pending,
        }
    }
}
