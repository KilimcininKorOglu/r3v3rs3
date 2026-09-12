use crate::proxy::http::error::ProxyError;
use hyper::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use r3v3rs3_api::policy::BearerAuth;
use sha2::{Digest, Sha256};
use std::fmt;
use subtle::{Choice, ConstantTimeEq};
use tracing::error;

const MISSING_TOKEN_CHALLENGE: &str = "Bearer realm=\"r3v3rs3\"";
const INVALID_TOKEN_CHALLENGE: &str = "Bearer realm=\"r3v3rs3\", error=\"invalid_token\"";

/// Bearer token authentication (RFC 6750) with SHA-256 token digests.
pub struct BearerAuthenticator {
    digests: Vec<[u8; 32]>,
}

impl fmt::Debug for BearerAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BearerAuthenticator")
            .field("tokens", &self.digests.len())
            .finish()
    }
}

impl BearerAuthenticator {
    pub fn new(config: BearerAuth) -> Self {
        let digests = config
            .tokens
            .into_iter()
            .filter_map(|token| {
                let digest = hex::decode(&token.token_hash)
                    .ok()
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
                if digest.is_none() {
                    error!(name = %token.name, "ignoring bearer token with an invalid hash");
                }
                digest
            })
            .collect();
        Self { digests }
    }

    pub fn authorize(&self, headers: &mut HeaderMap) -> Result<(), ProxyError> {
        let Some(token) = headers.get(AUTHORIZATION).and_then(parse_token) else {
            return Err(unauthorized(MISSING_TOKEN_CHALLENGE));
        };
        if !self.is_valid(token.as_bytes()) {
            return Err(unauthorized(INVALID_TOKEN_CHALLENGE));
        }
        headers.remove(AUTHORIZATION);
        Ok(())
    }

    /// Compares the digest with every known digest in constant time, so the response time does
    /// not reveal which token matched or how much of it matched.
    fn is_valid(&self, token: &[u8]) -> bool {
        let digest: [u8; 32] = Sha256::digest(token).into();
        let matched = self.digests.iter().fold(Choice::from(0), |matched, known| {
            matched | known[..].ct_eq(&digest[..])
        });
        bool::from(matched)
    }
}

fn unauthorized(challenge: &'static str) -> ProxyError {
    ProxyError::Unauthorized {
        challenge: HeaderValue::from_static(challenge),
    }
}

/// Returns the token of a `Bearer` authorization header.
fn parse_token(value: &HeaderValue) -> Option<&str> {
    let (scheme, token) = value.to_str().ok()?.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::policy::BearerToken;

    const TOKEN: &str = "token-for-ci-0123456789";

    fn authenticator() -> BearerAuthenticator {
        let token = |name: &str, token_hash: String| BearerToken {
            name: name.into(),
            token: String::new(),
            token_hash,
        };
        BearerAuthenticator::new(BearerAuth {
            tokens: vec![
                token("ci", hex::encode(Sha256::digest(TOKEN))),
                token("broken", "zz".into()),
            ],
        })
    }

    fn authorization(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, value.parse().unwrap());
        headers
    }

    fn challenge(result: Result<(), ProxyError>) -> HeaderValue {
        match result {
            Err(ProxyError::Unauthorized { challenge }) => challenge,
            other => panic!("expected unauthorized, got {other:?}"),
        }
    }

    #[test]
    fn valid_token_is_accepted_and_removed() {
        let auth = authenticator();
        assert_eq!(auth.digests.len(), 1);
        let mut headers = authorization(&format!("bearer {TOKEN}"));
        assert!(auth.authorize(&mut headers).is_ok());
        assert!(headers.get(AUTHORIZATION).is_none());
    }

    #[test]
    fn missing_or_invalid_token_is_rejected() {
        let auth = authenticator();
        assert_eq!(
            challenge(auth.authorize(&mut HeaderMap::new())),
            MISSING_TOKEN_CHALLENGE
        );
        assert_eq!(
            challenge(auth.authorize(&mut authorization(&format!("Basic {TOKEN}")))),
            MISSING_TOKEN_CHALLENGE
        );
        let mut headers = authorization("Bearer token-for-ci-012345678");
        assert_eq!(
            challenge(auth.authorize(&mut headers)),
            INVALID_TOKEN_CHALLENGE
        );
        assert!(headers.get(AUTHORIZATION).is_some());
    }
}
