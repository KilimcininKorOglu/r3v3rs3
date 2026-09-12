use super::error::ProxyError;
use hyper::HeaderMap;
use r3v3rs3_api::policy::AuthPolicy;
use std::sync::Arc;

mod basic;
mod bearer;

use basic::BasicAuthenticator;
use bearer::BearerAuthenticator;

/// Authenticates the requests of a route.
#[derive(Debug)]
pub enum Authenticator {
    Basic(BasicAuthenticator),
    Bearer(BearerAuthenticator),
}

impl Authenticator {
    /// Returns `None` when the policy does not require authentication.
    pub fn new(policy: AuthPolicy) -> Option<Arc<Self>> {
        let authenticator = match policy {
            AuthPolicy::None => return None,
            AuthPolicy::Basic(basic) => Self::Basic(BasicAuthenticator::new(basic)),
            AuthPolicy::Bearer(bearer) => Self::Bearer(BearerAuthenticator::new(bearer)),
        };
        Some(Arc::new(authenticator))
    }

    /// Checks the credentials in the request headers, then removes them so that they do not
    /// reach the upstream server.
    pub async fn authorize(&self, headers: &mut HeaderMap) -> Result<(), ProxyError> {
        match self {
            Self::Basic(basic) => basic.authorize(headers).await,
            Self::Bearer(bearer) => bearer.authorize(headers),
        }
    }
}
