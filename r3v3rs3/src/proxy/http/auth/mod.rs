use super::error::ProxyError;
use hyper::HeaderMap;
use r3v3rs3_api::policy::AuthPolicy;
use std::sync::Arc;

mod basic;

use basic::BasicAuthenticator;

/// Authenticates the requests of a route.
#[derive(Debug)]
pub enum Authenticator {
    Basic(BasicAuthenticator),
}

impl Authenticator {
    /// Returns `None` when the policy does not require authentication.
    pub fn new(policy: AuthPolicy) -> Option<Arc<Self>> {
        match policy {
            AuthPolicy::None => None,
            AuthPolicy::Basic(basic) => Some(Arc::new(Self::Basic(BasicAuthenticator::new(basic)))),
        }
    }

    /// Checks the credentials in the request headers, then removes them so that they do not
    /// reach the upstream server.
    pub async fn authorize(&self, headers: &mut HeaderMap) -> Result<(), ProxyError> {
        match self {
            Self::Basic(basic) => basic.authorize(headers).await,
        }
    }
}
