use super::error::ProxyError;
use super::page::PagePreferences;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{HeaderMap, Request, Response, body::Body};
use once_cell::sync::Lazy;
use r3v3rs3_api::{id::ShortId, policy::AuthPolicy, vhost::VirtualHost};
use std::{error::Error, fmt, net::IpAddr, ops::ControlFlow, sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tokio_rustls::rustls::ClientConfig;

mod basic;
mod bearer;
pub(crate) mod forward;
mod oidc;
mod session;

pub(crate) static PASSWORD_HASH_SEMAPHORE: Lazy<Semaphore> = Lazy::new(|| {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    Semaphore::new(cpus.clamp(2, 4))
});

pub(crate) const HASH_PERMIT_TIMEOUT: Duration = Duration::from_secs(5);

use basic::BasicAuthenticator;
use bearer::BearerAuthenticator;
use forward::ForwardAuthenticator;
use oidc::OidcAuthenticator;
use session::SessionAuthenticator;
pub use session::SessionService;

/// Strips internal proxy authentication cookies so upstream servers never receive them.
pub(crate) fn remove_auth_cookies(headers: &mut HeaderMap) {
    oidc::remove_oidc_cookie(headers);
    session::remove_session_cookie(headers);
    super::cookie::remove_cookie(headers, crate::admin::auth::ADMIN_SESSION_COOKIE);
}

/// Authenticates the requests of a route.
#[derive(Debug)]
pub enum Authenticator {
    Basic(Box<BasicAuthenticator>),
    Bearer(BearerAuthenticator),
    Forward(Box<ForwardAuthenticator>),
    Session(SessionAuthenticator),
    Oidc(Box<OidcAuthenticator>),
}

/// Details of the client request that are not in the request itself.
pub struct AuthContext<'a> {
    pub client: IpAddr,
    pub peer: IpAddr,
    pub host: Option<&'a str>,
    pub proto: &'static str,
    /// Path of the matched route without a trailing slash, e.g. `/admin`. Empty for `/`.
    pub base_path: &'a str,
    /// Segments of the request path below the route path.
    pub path_segments: &'a [String],
    /// Language and theme of the pages that the authenticator renders.
    pub preferences: PagePreferences,
    /// Configured virtual hosts of the matched route.
    pub vhosts: &'a [VirtualHost],
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
    /// Returns `None` when the policy does not require authentication. The session policy signs in
    /// only the accounts that see `proxy`.
    pub fn new(
        policy: AuthPolicy,
        tls_client_config: &Arc<ClientConfig>,
        sessions: &Arc<SessionService>,
        proxy: ShortId,
    ) -> Option<Arc<Self>> {
        let authenticator = match policy {
            AuthPolicy::None => return None,
            AuthPolicy::Basic(basic) => Self::Basic(Box::new(BasicAuthenticator::new(basic))),
            AuthPolicy::Bearer(bearer) => Self::Bearer(BearerAuthenticator::new(bearer)),
            AuthPolicy::Forward(forward) => Self::Forward(Box::new(ForwardAuthenticator::new(
                *forward,
                tls_client_config.clone(),
            ))),
            AuthPolicy::Session => {
                Self::Session(SessionAuthenticator::new(sessions.clone(), proxy))
            }
            AuthPolicy::Oidc(oidc) => Self::Oidc(Box::new(OidcAuthenticator::new(
                *oidc,
                tls_client_config.clone(),
                sessions.clone(),
            ))),
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
            Self::Oidc(oidc) => oidc.serve(req, ctx).await,
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
            Self::Basic(basic) => Ok(basic.authorize(req.headers_mut(), ctx.client).await?),
            Self::Bearer(bearer) => Ok(bearer.authorize(req.headers_mut())?),
            Self::Forward(forward) => forward.authorize(req, ctx).await,
            Self::Session(session) => session.authorize(req, ctx).await,
            Self::Oidc(oidc) => oidc.authorize(req, ctx).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::{COOKIE, HeaderValue};

    #[test]
    fn strips_admin_session_cookie_from_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("token=admin_secret; user_pref=dark"),
        );
        remove_auth_cookies(&mut headers);
        assert_eq!(
            headers.get(COOKIE).map(|v| v.to_str().unwrap()),
            Some("user_pref=dark")
        );
    }

    #[test]
    fn removes_cookie_header_when_only_admin_cookie_present() {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, HeaderValue::from_static("token=admin_secret"));
        remove_auth_cookies(&mut headers);
        assert!(!headers.contains_key(COOKIE));
    }
}
