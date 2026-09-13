use crate::proxy::http::error::ProxyError;
use crate::proxy::http::{hyper_tls::client::HttpsConnector, HTTP2_MAX_FRAME_SIZE};
use crate::proxy::tls::upstream_client_config;
use crate::server::cert_list::CertList;
use bytes::Bytes;
use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::{header::UPGRADE, http::uri::Scheme, Request, Response, StatusCode};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::{TokioExecutor, TokioIo},
};
use r3v3rs3_api::{error::Error, id::ShortId};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, io};
use tokio_rustls::rustls::ClientConfig;
use tracing::error;

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

    /// Sends the request. `request_timeout` limits the time until the response headers arrive,
    /// and `Duration::ZERO` disables the limit.
    pub async fn request(
        &self,
        mut req: Request<ProxyBody>,
        request_timeout: Duration,
    ) -> Result<Response<ProxyBody>, anyhow::Error> {
        let upgrading_req = if req.headers().contains_key(UPGRADE) {
            let mut cloned_req = Request::builder().uri(req.uri()).body(empty_body())?;
            cloned_req.headers_mut().clone_from(req.headers());
            Some(std::mem::replace(&mut req, cloned_req))
        } else {
            None
        };

        // The connection decides the protocol. An HTTP/1 connection rejects an HTTP/2 request
        // version, and an HTTP/2 connection ignores the request version.
        *req.version_mut() = hyper::Version::HTTP_11;

        let sending = self.client_for(&req, upgrading_req.is_some()).request(req);
        let result = response_headers(sending, request_timeout)
            .await
            .map(|res| res.map(|body| BoxBody::new(body.map_err(Into::into))));

        let result = match (result, upgrading_req) {
            (Ok(res), Some(upgrading_req)) if res.status() == StatusCode::SWITCHING_PROTOCOLS => {
                let mut cloned_res = Response::builder()
                    .status(res.status())
                    .body(empty_body())?;
                cloned_res.headers_mut().clone_from(res.headers());
                tokio::spawn(upgrade_connection(upgrading_req, res));
                Ok(cloned_res)
            }
            (result, _) => result,
        };

        if let Err(err) = &result {
            error!(%err);
        }

        ResponseRewriter::default().map_response(result)
    }
}

/// Waits for the response headers. A request timeout or a connect timeout becomes
/// [`ProxyError::UpstreamTimeout`].
async fn response_headers<F, B>(
    sending: F,
    request_timeout: Duration,
) -> Result<Response<B>, anyhow::Error>
where
    F: Future<Output = Result<Response<B>, hyper_util::client::legacy::Error>>,
{
    let result = if request_timeout.is_zero() {
        sending.await
    } else {
        tokio::time::timeout(request_timeout, sending)
            .await
            .map_err(|_| ProxyError::UpstreamTimeout)?
    };
    result.map_err(|err| {
        if is_timeout(&err) {
            ProxyError::UpstreamTimeout.into()
        } else {
            err.into()
        }
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

/// The upstream connections of a route and the time limit of its requests.
#[derive(Debug, Clone)]
pub struct Upstream {
    pub pool: Arc<ConnectionPool>,
    /// `Duration::ZERO` disables the limit.
    pub request_timeout: Duration,
}

impl Upstream {
    pub async fn request(
        &self,
        req: Request<ProxyBody>,
    ) -> Result<Response<ProxyBody>, anyhow::Error> {
        self.pool.request(req, self.request_timeout).await
    }
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
}
