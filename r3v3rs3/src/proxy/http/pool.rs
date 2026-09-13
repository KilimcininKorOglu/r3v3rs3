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
use std::fmt;
use std::sync::Arc;
use tokio_rustls::rustls::ClientConfig;
use tracing::error;

use super::rewriter::ResponseRewriter;

type ProxyBody = BoxBody<Bytes, anyhow::Error>;
type ProxyClient = Client<HttpsConnector<HttpConnector>, ProxyBody>;

/// Marks a request for a plain HTTP upstream server that accepts HTTP/2 with prior knowledge.
#[derive(Debug, Clone, Copy)]
pub struct UpstreamH2c;

/// Upstream clients shared by every connection of a port.
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
    pub fn new(tls_client_config: Arc<ClientConfig>) -> Self {
        let negotiated = with_alpn(&tls_client_config, &[b"h2", b"http/1.1"]);
        Self {
            client: build_client(negotiated.clone(), false),
            http1_client: build_client(with_alpn(&tls_client_config, &[b"http/1.1"]), false),
            h2c_client: build_client(negotiated, true),
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

    pub async fn request(
        &self,
        mut req: Request<ProxyBody>,
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

        let result: Result<_, anyhow::Error> = self
            .client_for(&req, upgrading_req.is_some())
            .request(req)
            .await
            .map_err(Into::into)
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

/// Builds the upstream TLS client config and the connection pool of each proxy on a port. Proxies
/// without a client certificate share one pool.
pub struct UpstreamClients<'a> {
    certs: &'a CertList,
    default_config: Arc<ClientConfig>,
    default_pool: Arc<ConnectionPool>,
}

impl<'a> UpstreamClients<'a> {
    pub fn new(certs: &'a CertList) -> Result<Self, Error> {
        let default_config = Arc::new(upstream_client_config(certs, None)?);
        Ok(Self {
            certs,
            default_pool: Arc::new(ConnectionPool::new(default_config.clone())),
            default_config,
        })
    }

    /// Returns the TLS client config and the connection pool for the client certificate. The pool
    /// is `None` when the certificate is invalid, so the proxy does not connect without it.
    pub fn for_cert(
        &self,
        client_cert: Option<ShortId>,
    ) -> (Arc<ClientConfig>, Option<Arc<ConnectionPool>>) {
        if client_cert.is_none() {
            return (self.default_config.clone(), Some(self.default_pool.clone()));
        }
        match upstream_client_config(self.certs, client_cert) {
            Ok(config) => {
                let config = Arc::new(config);
                let pool = Arc::new(ConnectionPool::new(config.clone()));
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

fn build_client(tls_client_config: Arc<ClientConfig>, http2_only: bool) -> ProxyClient {
    Client::builder(TokioExecutor::new())
        .http2_only(http2_only)
        .http2_max_frame_size(Some(HTTP2_MAX_FRAME_SIZE as u32))
        .build(HttpsConnector::new(tls_client_config))
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
