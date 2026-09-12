use super::error::ProxyError;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response};
use r3v3rs3_api::policy::AuthPolicy;
use std::{fmt, net::IpAddr, sync::Arc};
use tokio_rustls::rustls::ClientConfig;

mod basic;
mod bearer;
mod forward;

use basic::BasicAuthenticator;
use bearer::BearerAuthenticator;
use forward::ForwardAuthenticator;

/// Authenticates the requests of a route.
#[derive(Debug)]
pub enum Authenticator {
    Basic(BasicAuthenticator),
    Bearer(BearerAuthenticator),
    Forward(Box<ForwardAuthenticator>),
}

/// Details of the client request that are not in the request itself.
pub struct AuthContext<'a> {
    pub client: IpAddr,
    pub host: Option<&'a str>,
    pub proto: &'static str,
}

/// Why a request did not pass authentication.
pub enum AuthRejection {
    /// r3v3rs3 renders the error response.
    Error(ProxyError),
    /// The forward auth service response that is sent to the client.
    Response(Response<Full<Bytes>>),
}

impl From<ProxyError> for AuthRejection {
    fn from(err: ProxyError) -> Self {
        Self::Error(err)
    }
}

impl fmt::Display for AuthRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Error(err) => err.fmt(f),
            Self::Response(res) => write!(f, "forward auth denied with status {}", res.status()),
        }
    }
}

impl Authenticator {
    /// Returns `None` when the policy does not require authentication.
    pub fn new(policy: AuthPolicy, tls_client_config: &Arc<ClientConfig>) -> Option<Arc<Self>> {
        let authenticator = match policy {
            AuthPolicy::None => return None,
            AuthPolicy::Basic(basic) => Self::Basic(BasicAuthenticator::new(basic)),
            AuthPolicy::Bearer(bearer) => Self::Bearer(BearerAuthenticator::new(bearer)),
            AuthPolicy::Forward(forward) => Self::Forward(Box::new(ForwardAuthenticator::new(
                *forward,
                tls_client_config.clone(),
            ))),
        };
        Some(Arc::new(authenticator))
    }

    /// Checks the credentials of the request. Basic and bearer credentials are removed so that
    /// they do not reach the upstream server.
    pub async fn authorize<B>(
        &self,
        req: &mut Request<B>,
        ctx: &AuthContext<'_>,
    ) -> Result<(), AuthRejection> {
        match self {
            Self::Basic(basic) => Ok(basic.authorize(req.headers_mut()).await?),
            Self::Bearer(bearer) => Ok(bearer.authorize(req.headers_mut())?),
            Self::Forward(forward) => forward.authorize(req, ctx).await,
        }
    }
}
