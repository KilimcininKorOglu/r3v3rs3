use super::error::ProxyError;
use super::page::PagePreferences;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{body::Body, Request, Response};
use r3v3rs3_api::policy::AuthPolicy;
use std::{error::Error, fmt, net::IpAddr, ops::ControlFlow, sync::Arc};
use tokio_rustls::rustls::ClientConfig;

mod basic;
mod bearer;
mod forward;
mod session;

use basic::BasicAuthenticator;
use bearer::BearerAuthenticator;
use forward::ForwardAuthenticator;
use session::SessionAuthenticator;
pub use session::SessionService;

/// Authenticates the requests of a route.
#[derive(Debug)]
pub enum Authenticator {
    Basic(BasicAuthenticator),
    Bearer(BearerAuthenticator),
    Forward(Box<ForwardAuthenticator>),
    Session(SessionAuthenticator),
}

/// Details of the client request that are not in the request itself.
pub struct AuthContext<'a> {
    pub client: IpAddr,
    pub host: Option<&'a str>,
    pub proto: &'static str,
    /// Path of the matched route without a trailing slash, e.g. `/admin`. Empty for `/`.
    pub base_path: &'a str,
    /// Segments of the request path below the route path.
    pub path_segments: &'a [String],
    /// Language and theme of the pages that the authenticator renders.
    pub preferences: PagePreferences,
}

/// Why a request did not pass authentication.
pub enum AuthRejection {
    /// r3v3rs3 renders the error response.
    Error(ProxyError),
    /// A response for the client, like the forward auth response or a sign-in redirect. Boxed,
    /// because a response is much larger than an error.
    Response(Box<Response<Full<Bytes>>>),
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
            Self::Response(res) => write!(f, "authentication denied with status {}", res.status()),
        }
    }
}

impl Authenticator {
    /// Returns `None` when the policy does not require authentication.
    pub fn new(
        policy: AuthPolicy,
        tls_client_config: &Arc<ClientConfig>,
        sessions: &Arc<SessionService>,
    ) -> Option<Arc<Self>> {
        let authenticator = match policy {
            AuthPolicy::None => return None,
            AuthPolicy::Basic(basic) => Self::Basic(BasicAuthenticator::new(basic)),
            AuthPolicy::Bearer(bearer) => Self::Bearer(BearerAuthenticator::new(bearer)),
            AuthPolicy::Forward(forward) => Self::Forward(Box::new(ForwardAuthenticator::new(
                *forward,
                tls_client_config.clone(),
            ))),
            AuthPolicy::Session => Self::Session(SessionAuthenticator::new(sessions.clone())),
        };
        Some(Arc::new(authenticator))
    }

    /// Answers the requests that the authenticator serves itself, like the sign-in page. Other
    /// requests continue to [`Self::authorize`].
    pub async fn serve<B>(
        &self,
        req: Request<B>,
        ctx: &AuthContext<'_>,
    ) -> ControlFlow<Response<Full<Bytes>>, Request<B>>
    where
        B: Body<Data = Bytes>,
        B::Error: Into<Box<dyn Error + Send + Sync>>,
    {
        match self {
            Self::Session(session) => session.serve(req, ctx).await,
            _ => ControlFlow::Continue(req),
        }
    }

    /// Checks the credentials of the request. Basic and bearer credentials and the session cookie
    /// are removed so that they do not reach the upstream server.
    pub async fn authorize<B>(
        &self,
        req: &mut Request<B>,
        ctx: &AuthContext<'_>,
    ) -> Result<(), AuthRejection> {
        match self {
            Self::Basic(basic) => Ok(basic.authorize(req.headers_mut()).await?),
            Self::Bearer(bearer) => Ok(bearer.authorize(req.headers_mut())?),
            Self::Forward(forward) => forward.authorize(req, ctx).await,
            Self::Session(session) => session.authorize(req, ctx),
        }
    }
}
