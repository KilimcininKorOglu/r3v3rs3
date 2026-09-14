use crate::proxy::health::{Permit, UpstreamGroup};
use crate::proxy::http::error::ProxyError;
use crate::proxy::http::{hyper_tls::client::HttpsConnector, HTTP2_MAX_FRAME_SIZE};
use crate::proxy::tls::upstream_client_config;
use crate::server::cert_list::CertList;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full, Limited};
use hyper::{
    body::Body,
    header::{HeaderValue, CONTENT_LENGTH, HOST, UPGRADE},
    http::uri::Scheme,
    Method, Request, Response, StatusCode, Uri,
};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo},
};
use r3v3rs3_api::upstream::{RetryOn, RetryPolicy};
use r3v3rs3_api::{error::Error, id::ShortId, proxy::Server};
use std::collections::HashMap;
use std::future::Future;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::{Arc, PoisonError};
use std::time::Duration;
use std::{fmt, io};
use tokio_rustls::rustls::ClientConfig;
use tracing::{error, warn};
use url::Url;

use super::affinity::{Affinity, CookieSlot};
use super::body_limit::BodyLimit;
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
    pub retry: RetryPolicy,
    /// `None` when the route has no sticky sessions.
    pub affinity: Option<Arc<Affinity>>,
    /// The largest request body in bytes. `0` has no limit.
    pub max_body_size: u64,
}

/// The servers to try for a request in order, and the part of the client URI that follows the
/// route path.
#[derive(Debug, Clone)]
struct UpstreamTarget {
    candidates: Vec<usize>,
    path_segments: Vec<String>,
    query: Option<String>,
    sticky: Option<StickyTarget>,
}

/// The sticky session of a request.
#[derive(Debug, Clone)]
struct StickyTarget {
    /// The server that the cookie of the request names, when that server can take the request.
    pinned: Option<usize>,
    /// Adds `Secure` to the cookie.
    secure: bool,
    cookie: CookieSlot,
}

impl Upstream {
    /// Selects the upstream server of the request, and sets the request URI and the `Host` header
    /// for that server. A route without a server leaves the request unchanged. `client` is the
    /// IP address of the client, and `secure` is true for a TLS connection. Returns the slot of
    /// the sticky cookie of the response when the route has sticky sessions.
    pub fn select<B>(
        &self,
        req: &mut Request<B>,
        path_segments: Vec<String>,
        client: IpAddr,
        secure: bool,
    ) -> Option<CookieSlot> {
        if self.servers.is_empty() {
            return None;
        }
        let mut candidates = self.group.candidates(client);
        let sticky = self
            .affinity
            .as_ref()
            .map(|affinity| self.pin(affinity, req, &mut candidates, secure));
        let cookie = sticky.as_ref().map(|sticky| sticky.cookie.clone());
        let target = UpstreamTarget {
            candidates,
            path_segments,
            query: req.uri().query().map(str::to_string),
            sticky,
        };
        if let Some(&first) = target.candidates.first() {
            self.apply(req, first, &target);
        }
        req.extensions_mut().insert(target);
        cookie
    }

    /// Moves the server of the sticky cookie to the front of the candidates, and removes the
    /// cookie from the request.
    fn pin<B>(
        &self,
        affinity: &Affinity,
        req: &mut Request<B>,
        candidates: &mut Vec<usize>,
        secure: bool,
    ) -> StickyTarget {
        let pinned = affinity
            .pinned(req.headers())
            .filter(|&index| self.group.is_pinnable(index));
        affinity.strip(req.headers_mut());
        if let Some(index) = pinned {
            candidates.retain(|&candidate| candidate != index);
            candidates.insert(0, index);
        }
        StickyTarget {
            pinned,
            secure,
            cookie: CookieSlot::default(),
        }
    }

    /// Sets the sticky cookie of the response to the server that answered. A client that keeps
    /// its server gets no new cookie.
    fn remember(&self, target: &UpstreamTarget, index: usize) {
        let (Some(affinity), Some(sticky)) = (&self.affinity, &target.sticky) else {
            return;
        };
        let cookie = if sticky.pinned == Some(index) {
            None
        } else {
            affinity.set_cookie(index, sticky.secure)
        };
        *sticky.cookie.lock().unwrap_or_else(PoisonError::into_inner) = cookie;
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

    /// Sends the request to the selected server. A failure that the retry policy names sends the
    /// request again to the next server, until the policy has no attempts left. When the circuit
    /// of every server blocks the request, the client receives 503. A request body larger than
    /// the limit fails the request with 413.
    pub async fn request(
        &self,
        req: Request<ProxyBody>,
    ) -> Result<Response<ProxyBody>, anyhow::Error> {
        let (req, limit) = BodyLimit::apply(req, self.max_body_size);
        let result = self.send_request(req).await.map_err(|err| {
            if limit.exceeded() {
                SendError::other(ProxyError::PayloadTooLarge)
            } else {
                err
            }
        });
        finish(result)
    }

    async fn send_request(
        &self,
        req: Request<ProxyBody>,
    ) -> Result<Response<ProxyBody>, SendError> {
        let Some(target) = req.extensions().get::<UpstreamTarget>().cloned() else {
            return self.pool.send(req, self.request_timeout).await;
        };
        let replay = Replay::prepare(req, self.retry.replay_body_limit).await?;
        self.send_with_retries(replay, &target).await
    }

    async fn send_with_retries(
        &self,
        mut replay: Replay,
        target: &UpstreamTarget,
    ) -> Result<Response<ProxyBody>, SendError> {
        let mut candidates = target.candidates.iter().copied();
        let mut result = Err(SendError::other(ProxyError::NoUpstreamAvailable));
        for attempt in 1..=self.retry.attempts.max(1) {
            let Some(mut req) = replay.next() else {
                break;
            };
            let Some(permit) = self.claim(&mut req, target, &mut candidates) else {
                break;
            };
            if attempt > 1 {
                warn!(uri = %req.uri(), attempt, "retrying the request on the next upstream server");
            }
            let index = permit.index();
            result = self.attempt(req, permit).await;
            if result.is_ok() {
                self.remember(target, index);
            }
            if !self.should_retry(&result, &replay.method) {
                break;
            }
        }
        result
    }

    /// True when the policy names the failure. A failure after the server received the request
    /// retries only an idempotent method.
    fn should_retry(
        &self,
        result: &Result<Response<ProxyBody>, SendError>,
        method: &Method,
    ) -> bool {
        retry_reason(result).is_some_and(|reason| {
            self.retry.retry_on.contains(&reason)
                && (reason == RetryOn::Connect || is_idempotent(method))
        })
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

/// The failure of one try that a retry policy can name.
fn retry_reason(result: &Result<Response<ProxyBody>, SendError>) -> Option<RetryOn> {
    match result {
        Err(err) if err.connect => Some(RetryOn::Connect),
        Err(err) => matches!(
            err.error.downcast_ref::<ProxyError>(),
            Some(ProxyError::UpstreamTimeout)
        )
        .then_some(RetryOn::Timeout),
        Ok(res) => match res.status() {
            StatusCode::BAD_GATEWAY => Some(RetryOn::Http502),
            StatusCode::SERVICE_UNAVAILABLE => Some(RetryOn::Http503),
            StatusCode::GATEWAY_TIMEOUT => Some(RetryOn::Http504),
            _ => None,
        },
    }
}

/// The idempotent methods of RFC 9110 section 9.2.2.
fn is_idempotent(method: &Method) -> bool {
    matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE | Method::PUT | Method::DELETE
    )
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

/// The request of the first try, and a copy of the request for the retries.
struct Replay {
    first: Option<Request<ProxyBody>>,
    copy: Option<Request<Bytes>>,
    method: Method,
}

impl Replay {
    /// Keeps a copy of a request without a body, and of a request whose body length is known and
    /// at most `limit` bytes. The body of such a request is read into memory. An upgrade request
    /// and a request with a longer or an unknown body length have no copy.
    async fn prepare(req: Request<ProxyBody>, limit: u64) -> Result<Self, SendError> {
        let method = req.method().clone();
        let length = body_length(&req).filter(|length| *length <= limit);
        if req.headers().contains_key(UPGRADE) || length.is_none() {
            return Ok(Self {
                first: Some(req),
                copy: None,
                method,
            });
        }
        if req.body().size_hint().exact() == Some(0) {
            let copy = copy_request(&req, Bytes::new());
            return Ok(Self {
                first: Some(req),
                copy: Some(copy),
                method,
            });
        }
        let (parts, body) = req.into_parts();
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let body = Limited::new(body, limit)
            .collect()
            .await
            .map_err(|err| SendError::other(anyhow::anyhow!(err)))?
            .to_bytes();
        let copy = Request::from_parts(parts, body);
        Ok(Self {
            first: Some(copy_request(&copy, full_body(copy.body().clone()))),
            copy: Some(copy),
            method,
        })
    }

    /// Returns the request of the first try, then a new copy for each retry.
    fn next(&mut self) -> Option<Request<ProxyBody>> {
        self.first.take().or_else(|| {
            let copy = self.copy.as_ref()?;
            Some(copy_request(copy, full_body(copy.body().clone())))
        })
    }
}

/// The body length from the body, or from the `Content-Length` header when the body does not
/// know it, e.g. on HTTP/3.
fn body_length<B: Body>(req: &Request<B>) -> Option<u64> {
    req.body().size_hint().exact().or_else(|| {
        req.headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok())
    })
}

/// Copies the method, the URI, the version, the headers and the extensions of a request.
fn copy_request<B, C>(req: &Request<B>, body: C) -> Request<C> {
    let mut copy = Request::new(body);
    *copy.method_mut() = req.method().clone();
    *copy.uri_mut() = req.uri().clone();
    *copy.version_mut() = req.version();
    *copy.headers_mut() = req.headers().clone();
    *copy.extensions_mut() = req.extensions().clone();
    copy
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
    full_body(Bytes::new())
}

fn full_body(data: Bytes) -> ProxyBody {
    BoxBody::new(Full::new(data).map_err(Into::into))
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

    async fn prepared(req: Request<ProxyBody>, limit: u64) -> Replay {
        match Replay::prepare(req, limit).await {
            Ok(replay) => replay,
            Err(err) => panic!("{}", err.error),
        }
    }

    async fn body_text(req: Request<ProxyBody>) -> String {
        let body = req.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(body.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn a_replay_copies_a_request_without_a_body_or_with_a_small_body() {
        let mut get = Request::get("http://127.0.0.1:9000/a?b=c")
            .header("x-test", "1")
            .body(empty_body())
            .unwrap();
        get.extensions_mut().insert(UpstreamH2c);
        let mut replay = prepared(get, 0).await;
        let first = replay.next().unwrap();
        let copy = replay.next().unwrap();
        assert_eq!(copy.method(), first.method());
        assert_eq!(copy.uri(), first.uri());
        assert_eq!(copy.headers(), first.headers());
        assert!(copy.extensions().get::<UpstreamH2c>().is_some());

        let data = || full_body(Bytes::from_static(b"data"));
        let post = Request::post("http://127.0.0.1:9000/")
            .body(data())
            .unwrap();
        let mut replay = prepared(post, 4).await;
        assert_eq!(body_text(replay.next().unwrap()).await, "data");
        assert_eq!(body_text(replay.next().unwrap()).await, "data");

        let post = Request::post("http://127.0.0.1:9000/")
            .body(data())
            .unwrap();
        let mut replay = prepared(post, 3).await;
        assert_eq!(body_text(replay.next().unwrap()).await, "data");
        assert!(replay.next().is_none());

        let upgrade = Request::get("http://127.0.0.1:9000/")
            .header(UPGRADE, "websocket")
            .body(empty_body())
            .unwrap();
        let mut replay = prepared(upgrade, 1024).await;
        assert!(replay.next().is_some());
        assert!(replay.next().is_none());
    }

    #[test]
    fn retry_reasons_name_the_failure_of_a_try() {
        let status = |code: u16| -> Result<Response<ProxyBody>, SendError> {
            Ok(Response::builder().status(code).body(empty_body()).unwrap())
        };
        assert_eq!(retry_reason(&status(502)), Some(RetryOn::Http502));
        assert_eq!(retry_reason(&status(503)), Some(RetryOn::Http503));
        assert_eq!(retry_reason(&status(504)), Some(RetryOn::Http504));
        assert_eq!(retry_reason(&status(500)), None);

        let timeout = SendError::other(ProxyError::UpstreamTimeout);
        assert_eq!(retry_reason(&Err(timeout)), Some(RetryOn::Timeout));
        let refused = SendError {
            error: anyhow::anyhow!("connection refused"),
            connect: true,
        };
        assert_eq!(retry_reason(&Err(refused)), Some(RetryOn::Connect));
        let reset = SendError::other(anyhow::anyhow!("connection reset"));
        assert_eq!(retry_reason(&Err(reset)), None);

        assert!(is_idempotent(&Method::PUT));
        assert!(!is_idempotent(&Method::POST));
        assert!(!is_idempotent(&Method::PATCH));
    }
}
