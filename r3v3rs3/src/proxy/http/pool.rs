use crate::proxy::health::{Permit, UpstreamGroup};
use crate::proxy::http::error::ProxyError;
use crate::proxy::http::{hyper_tls::client::HttpsConnector, HTTP2_MAX_FRAME_SIZE};
use crate::proxy::tls::upstream_client_config;
use crate::server::cert_list::CertList;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{
    body::Body,
    header::{HeaderValue, HOST, UPGRADE},
    http::uri::Scheme,
    Request, Response, StatusCode, Uri,
};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo},
};
use r3v3rs3_api::{error::Error, id::ShortId, proxy::Server};
use std::collections::HashMap;
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, io};
use tokio_rustls::rustls::ClientConfig;
use tracing::{error, warn};
use url::Url;

use super::rewriter::ResponseRewriter;

type ProxyBody = BoxBody<Bytes, anyhow::Error>;
type ProxyClient = Client<HttpsConnector<HttpConnector>, ProxyBody>;

/// Marks a request for a plain HTTP upstream server that accepts HTTP/2 with prior knowledge.
#[derive(Debug, Clone, Copy)]
pub struct UpstreamH2c;

/// Upstream clients shared by the proxies of a port that use the same client certificate and
/// connect timeout.
pub struct ConnectionPool {
    /// Negotiates HTTP/2 or HTTP/1.1 with ALPN on TLS, and uses HTTP/1.1 on plain HTTP.
    client: ProxyClient,
    /// Offers only HTTP/1.1 with ALPN, because an HTTP upgrade needs HTTP/1.1.
    http1_client: ProxyClient,
    /// Uses HTTP/2 with prior knowledge.
    h2c_client: ProxyClient,
}

impl fmt::Debug for ConnectionPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionPool").finish_non_exhaustive()
    }
}

impl ConnectionPool {
    pub fn new(tls_client_config: Arc<ClientConfig>, connect_timeout: Duration) -> Self {
        let negotiated = with_alpn(&tls_client_config, &[b"h2", b"http/1.1"]);
        let http1 = with_alpn(&tls_client_config, &[b"http/1.1"]);
        Self {
            client: build_client(negotiated.clone(), false, connect_timeout),
            http1_client: build_client(http1, false, connect_timeout),
            h2c_client: build_client(negotiated, true, connect_timeout),
        }
    }

    fn client_for<B>(&self, req: &Request<B>, upgrading: bool) -> &ProxyClient {
        if upgrading {
            &self.http1_client
        } else if req.extensions().get::<UpstreamH2c>().is_some()
            && req.uri().scheme() == Some(&Scheme::HTTP)
        {
            &self.h2c_client
        } else {
            &self.client
        }
    }

    /// Sends `GET` to the URI without a body, and returns the status of the response.
    pub async fn probe(&self, uri: Uri) -> anyhow::Result<StatusCode> {
        let req = Request::get(uri).body(empty_body())?;
        let res = self
            .send(req, Duration::ZERO)
            .await
            .map_err(|err| err.error)?;
        Ok(res.status())
    }

    /// Sends the request. `request_timeout` limits the time until the response headers arrive,
    /// and `Duration::ZERO` disables the limit.
    async fn send(
        &self,
        mut req: Request<ProxyBody>,
        request_timeout: Duration,
    ) -> Result<Response<ProxyBody>, SendError> {
        let upgrading_req = if req.headers().contains_key(UPGRADE) {
            let mut cloned_req = Request::builder()
                .uri(req.uri())
                .body(empty_body())
                .map_err(SendError::other)?;
            cloned_req.headers_mut().clone_from(req.headers());
            Some(std::mem::replace(&mut req, cloned_req))
        } else {
            None
        };

        // The connection decides the protocol. An HTTP/1 connection rejects an HTTP/2 request
        // version, and an HTTP/2 connection ignores the request version.
        *req.version_mut() = hyper::Version::HTTP_11;

        let sending = self.client_for(&req, upgrading_req.is_some()).request(req);
        let res = response_headers(sending, request_timeout)
            .await?
            .map(|body| BoxBody::new(body.map_err(Into::into)));

        match upgrading_req {
            Some(upgrading_req) if res.status() == StatusCode::SWITCHING_PROTOCOLS => {
                let mut cloned_res = Response::builder()
                    .status(res.status())
                    .body(empty_body())
                    .map_err(SendError::other)?;
                cloned_res.headers_mut().clone_from(res.headers());
                tokio::spawn(upgrade_connection(upgrading_req, res));
                Ok(cloned_res)
            }
            _ => Ok(res),
        }
    }
}

/// A failed upstream request.
struct SendError {
    error: anyhow::Error,
    /// True when the connection to the upstream server failed, so the server received nothing.
    connect: bool,
}

impl SendError {
    fn other(error: impl Into<anyhow::Error>) -> Self {
        Self {
            error: error.into(),
            connect: false,
        }
    }
}

/// Waits for the response headers. A request timeout or a connect timeout becomes
/// [`ProxyError::UpstreamTimeout`].
async fn response_headers<F, B>(
    sending: F,
    request_timeout: Duration,
) -> Result<Response<B>, SendError>
where
    F: Future<Output = Result<Response<B>, hyper_util::client::legacy::Error>>,
{
    let result = if request_timeout.is_zero() {
        sending.await
    } else {
        tokio::time::timeout(request_timeout, sending)
            .await
            .map_err(|_| SendError::other(ProxyError::UpstreamTimeout))?
    };
    result.map_err(|err| SendError {
        connect: err.is_connect(),
        error: if is_timeout(&err) {
            ProxyError::UpstreamTimeout.into()
        } else {
            err.into()
        },
    })
}

/// Returns true when the error or one of its sources is an `io::ErrorKind::TimedOut` error.
fn is_timeout(err: &(dyn std::error::Error + 'static)) -> bool {
    let mut source = Some(err);
    while let Some(err) = source {
        if err
            .downcast_ref::<io::Error>()
            .is_some_and(|err| err.kind() == io::ErrorKind::TimedOut)
        {
            return true;
        }
        source = err.source();
    }
    false
}

/// The upstream servers of a route, their connections and the time limit of their requests.
#[derive(Debug, Clone)]
pub struct Upstream {
    pub pool: Arc<ConnectionPool>,
    /// `Duration::ZERO` disables the limit.
    pub request_timeout: Duration,
    pub servers: Arc<[Server]>,
    pub group: Arc<UpstreamGroup>,
}

/// The servers to try for a request in order, and the part of the client URI that follows the
/// route path.
#[derive(Debug, Clone)]
struct UpstreamTarget {
    candidates: Vec<usize>,
    path_segments: Vec<String>,
    query: Option<String>,
}

impl Upstream {
    /// Selects the upstream server of the request, and sets the request URI and the `Host` header
    /// for that server. A route without a server leaves the request unchanged.
    pub fn select<B>(&self, req: &mut Request<B>, path_segments: Vec<String>) {
        if self.servers.is_empty() {
            return;
        }
        let target = UpstreamTarget {
            candidates: self.group.candidates(),
            path_segments,
            query: req.uri().query().map(str::to_string),
        };
        if let Some(&first) = target.candidates.first() {
            self.apply(req, first, &target);
        }
        req.extensions_mut().insert(target);
    }

    fn apply<B>(&self, req: &mut Request<B>, index: usize, target: &UpstreamTarget) {
        let Some(server) = self.servers.get(index) else {
            return;
        };
        let url = upstream_url(
            &server.url.0,
            &target.path_segments,
            target.query.as_deref(),
        );
        let Ok(uri) = Uri::from_str(url.as_str()) else {
            return;
        };
        if let Some(host) = uri
            .authority()
            .and_then(|host| HeaderValue::from_str(host.as_str()).ok())
        {
            req.headers_mut().insert(HOST, host);
        }
        *req.uri_mut() = uri;
    }

    /// Sends the request to the selected server. When the connection to the server fails and the
    /// request has no body, the request goes once to the next server. When the circuit of every
    /// server blocks the request, the client receives 503.
    pub async fn request(
        &self,
        mut req: Request<ProxyBody>,
    ) -> Result<Response<ProxyBody>, anyhow::Error> {
        let Some(target) = req.extensions().get::<UpstreamTarget>().cloned() else {
            return finish(self.pool.send(req, self.request_timeout).await);
        };
        let copy = replayable_copy(&req);
        let mut candidates = target.candidates.iter().copied();
        let Some(permit) = self.claim(&mut req, &target, &mut candidates) else {
            return finish(Err(SendError::other(ProxyError::NoUpstreamAvailable)));
        };
        let mut result = self.attempt(req, permit).await;
        let connect_failed = matches!(&result, Err(err) if err.connect);
        if let (true, Some(mut req)) = (connect_failed, copy) {
            if let Some(permit) = self.claim(&mut req, &target, &mut candidates) {
                warn!(uri = %req.uri(), "retrying the request on the next upstream server");
                result = self.attempt(req, permit).await;
            }
        }
        finish(result)
    }

    /// Takes the next candidate whose circuit lets the request through, and points the request at
    /// that server when `select` chose another server.
    fn claim<B>(
        &self,
        req: &mut Request<B>,
        target: &UpstreamTarget,
        candidates: &mut impl Iterator<Item = usize>,
    ) -> Option<Permit> {
        let permit = candidates.find_map(|index| self.group.acquire(index))?;
        if target.candidates.first() != Some(&permit.index()) {
            self.apply(req, permit.index(), target);
        }
        Some(permit)
    }

    /// Sends the request and records the result in the health and the circuit of the server.
    async fn attempt(
        &self,
        req: Request<ProxyBody>,
        permit: Permit,
    ) -> Result<Response<ProxyBody>, SendError> {
        let result = self.pool.send(req, self.request_timeout).await;
        match &result {
            Ok(res) if is_gateway_error(res.status()) => {
                permit.error_response(&format!("the server answered {}", res.status()))
            }
            Ok(_) => permit.success(),
            Err(err) => permit.failure(&err.error.to_string()),
        }
        result
    }
}

/// A 502, 503 or 504 response, which the circuit breaker counts as a failure.
fn is_gateway_error(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
    )
}

/// The URL of a request to a server: the path of the server URL, then the request path segments
/// after the route path, then the query of the request. The segments keep their percent-encoding.
/// A request without segments keeps the server path unchanged.
fn upstream_url(server: &Url, segments: &[String], query: Option<&str>) -> Url {
    let mut url = server.clone();
    if !segments.is_empty() {
        let rest = segments
            .iter()
            .map(String::as_str)
            .filter(|segment| !is_dot_segment(segment))
            .collect::<Vec<_>>()
            .join("/");
        let path = format!("{}/{rest}", url.path().trim_end_matches('/'));
        url.set_path(&path);
    }
    url.set_query(query);
    url
}

/// A `.` or `..` segment, also percent-encoded. The URL parser resolves these segments, so they
/// could move the request out of the server path.
fn is_dot_segment(segment: &str) -> bool {
    matches!(
        segment.to_ascii_lowercase().as_str(),
        "." | ".." | "%2e" | ".%2e" | "%2e." | "%2e%2e"
    )
}

/// Logs a failed request, and turns the error into the error response for the client.
fn finish(
    result: Result<Response<ProxyBody>, SendError>,
) -> Result<Response<ProxyBody>, anyhow::Error> {
    let result = result.map_err(|err| err.error);
    if let Err(err) = &result {
        error!(%err);
    }
    ResponseRewriter::default().map_response(result)
}

/// Copies a request without a body, so that it can go to another server. Returns `None` for a
/// request with a body and for an upgrade request, because they cannot be sent again.
fn replayable_copy(req: &Request<ProxyBody>) -> Option<Request<ProxyBody>> {
    if req.body().size_hint().exact() != Some(0) || req.headers().contains_key(UPGRADE) {
        return None;
    }
    let mut copy = Request::new(empty_body());
    *copy.method_mut() = req.method().clone();
    *copy.uri_mut() = req.uri().clone();
    *copy.version_mut() = req.version();
    *copy.headers_mut() = req.headers().clone();
    *copy.extensions_mut() = req.extensions().clone();
    Some(copy)
}

type UpstreamClient = (Arc<ClientConfig>, Option<Arc<ConnectionPool>>);

/// Builds the upstream TLS client config and the connection pool of each proxy on a port. Proxies
/// with the same client certificate and connect timeout share one pool.
pub struct UpstreamClients<'a> {
    certs: &'a CertList,
    default_config: Arc<ClientConfig>,
    clients: HashMap<(Option<ShortId>, Duration), UpstreamClient>,
}

impl<'a> UpstreamClients<'a> {
    pub fn new(certs: &'a CertList) -> Result<Self, Error> {
        Ok(Self {
            certs,
            default_config: Arc::new(upstream_client_config(certs, None)?),
            clients: HashMap::new(),
        })
    }

    /// Returns the TLS client config and the connection pool for the client certificate and the
    /// connect timeout. The pool is `None` when the certificate is invalid, so the proxy does not
    /// connect without it.
    pub fn get(
        &mut self,
        client_cert: Option<ShortId>,
        connect_timeout: Duration,
    ) -> UpstreamClient {
        let key = (client_cert, connect_timeout);
        if let Some(client) = self.clients.get(&key) {
            return client.clone();
        }
        let client = self.build(client_cert, connect_timeout);
        self.clients.insert(key, client.clone());
        client
    }

    fn build(&self, client_cert: Option<ShortId>, connect_timeout: Duration) -> UpstreamClient {
        let config = match client_cert {
            None => Ok(self.default_config.clone()),
            Some(_) => upstream_client_config(self.certs, client_cert).map(Arc::new),
        };
        match config {
            Ok(config) => {
                let pool = Arc::new(ConnectionPool::new(config.clone(), connect_timeout));
                (config, Some(pool))
            }
            Err(err) => {
                error!(%err, "the proxy cannot connect to its upstream servers");
                (self.default_config.clone(), None)
            }
        }
    }
}

fn with_alpn(config: &ClientConfig, protocols: &[&[u8]]) -> Arc<ClientConfig> {
    let mut config = config.clone();
    config.alpn_protocols = protocols.iter().map(|protocol| protocol.to_vec()).collect();
    Arc::new(config)
}

fn build_client(
    tls_client_config: Arc<ClientConfig>,
    http2_only: bool,
    connect_timeout: Duration,
) -> ProxyClient {
    Client::builder(TokioExecutor::new())
        .http2_only(http2_only)
        .http2_max_frame_size(Some(HTTP2_MAX_FRAME_SIZE as u32))
        .build(HttpsConnector::new(tls_client_config).with_connect_timeout(connect_timeout))
}

fn empty_body() -> ProxyBody {
    BoxBody::new(Full::new(Bytes::new()).map_err(Into::into))
}

async fn upgrade_connection(req: Request<ProxyBody>, res: Response<ProxyBody>) {
    match tokio::try_join!(hyper::upgrade::on(req), hyper::upgrade::on(res)) {
        Ok((req, res)) => {
            let mut req = TokioIo::new(req);
            let mut res = TokioIo::new(res);
            if let Err(err) = tokio::io::copy_bidirectional(&mut req, &mut res).await {
                error!("upgraded io error: {}", err);
            }
        }
        Err(err) => {
            error!("upgrading io error: {}", err);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_timeout_reads_the_error_sources() {
        let timed_out = io::Error::new(io::ErrorKind::TimedOut, "timed out");
        assert!(is_timeout(&timed_out));

        let wrapped = anyhow::Error::new(timed_out).context("connect error");
        assert!(is_timeout(wrapped.as_ref()));

        let refused = io::Error::new(io::ErrorKind::ConnectionRefused, "refused");
        assert!(!is_timeout(&refused));
    }

    fn joined(server: &str, segments: &[&str], query: Option<&str>) -> String {
        let segments = segments.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        upstream_url(&server.parse().unwrap(), &segments, query).to_string()
    }

    #[test]
    fn the_server_path_is_joined_with_one_slash() {
        let base = "http://up/base/";
        assert_eq!(joined(base, &["x"], Some("q=1")), "http://up/base/x?q=1");
        assert_eq!(joined("http://up/base", &["x"], None), "http://up/base/x");
        assert_eq!(joined("http://up/", &["x"], None), "http://up/x");
        assert_eq!(joined(base, &["x", ""], None), "http://up/base/x/");
        assert_eq!(joined("http://up/base", &[], None), "http://up/base");
    }

    #[test]
    fn the_request_segments_keep_their_percent_encoding() {
        let url = joined("http://up/base/", &["a%20b", "c%2Fd"], None);
        assert_eq!(url, "http://up/base/a%20b/c%2Fd");
    }

    #[test]
    fn dot_segments_cannot_leave_the_server_path() {
        let segments = ["..", "%2e%2E", ".", "%2e", "secret"];
        assert_eq!(
            joined("http://up/base/", &segments, None),
            "http://up/base/secret"
        );
    }

    #[test]
    fn only_a_request_without_a_body_can_be_sent_again() {
        let mut get = Request::get("http://127.0.0.1:9000/a?b=c")
            .header("x-test", "1")
            .body(empty_body())
            .unwrap();
        get.extensions_mut().insert(UpstreamH2c);
        let copy = replayable_copy(&get).unwrap();
        assert_eq!(copy.method(), get.method());
        assert_eq!(copy.uri(), get.uri());
        assert_eq!(copy.headers(), get.headers());
        assert!(copy.extensions().get::<UpstreamH2c>().is_some());

        let body = BoxBody::new(Full::new(Bytes::from("data")).map_err(Into::into));
        let post = Request::post("http://127.0.0.1:9000/").body(body).unwrap();
        assert!(replayable_copy(&post).is_none());

        let upgrade = Request::get("http://127.0.0.1:9000/")
            .header(UPGRADE, "websocket")
            .body(empty_body())
            .unwrap();
        assert!(replayable_copy(&upgrade).is_none());
    }
}
