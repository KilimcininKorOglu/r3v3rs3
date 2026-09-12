use argon2::{
    password_hash::{PasswordHash, SaltString},
    Argon2, PasswordHasher,
};
use r3v3rs3_api::{
    error::Error,
    policy::{
        is_header_name, AuthPolicy, BasicAuth, BasicAuthUser, BearerAuth, BearerToken, ForwardAuth,
    },
    proxy::{Proxy, ProxyKind},
};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// Replaces the plain text passwords and tokens of an HTTP proxy with hashes and validates them.
/// Passwords use argon2. Tokens use SHA-256, because they are long random values.
pub fn seal_proxy(proxy: &mut Proxy) -> Result<(), Error> {
    let ProxyKind::Http(http) = &mut proxy.kind else {
        return Ok(());
    };
    let route_policies = http
        .routes
        .iter_mut()
        .filter_map(|route| route.auth.as_mut());
    for policy in std::iter::once(&mut http.auth).chain(route_policies) {
        match policy {
            AuthPolicy::None => {}
            AuthPolicy::Basic(basic) => seal_basic_auth(basic)?,
            AuthPolicy::Bearer(bearer) => seal_bearer_auth(bearer)?,
            AuthPolicy::Forward(forward) => validate_forward_auth(forward)?,
        }
    }
    Ok(())
}

/// Runs [`seal_proxy`] on a blocking thread, because hashing a password takes CPU time.
pub async fn seal(mut proxy: Proxy) -> Result<Proxy, Error> {
    tokio::task::spawn_blocking(move || seal_proxy(&mut proxy).map(|()| proxy))
        .await
        .map_err(|_| Error::FailedToHashPassword)?
}

fn seal_basic_auth(basic: &mut BasicAuth) -> Result<(), Error> {
    let mut usernames = HashSet::new();
    for user in &mut basic.users {
        if user.username_error().is_some() || !usernames.insert(user.username.clone()) {
            return Err(Error::InvalidUsername {
                username: user.username.clone(),
            });
        }
        seal_user(user)?;
    }
    Ok(())
}

fn seal_user(user: &mut BasicAuthUser) -> Result<(), Error> {
    if !user.password.is_empty() {
        user.password_hash = hash_password(&user.password)?;
        user.password.clear();
    }
    if PasswordHash::new(&user.password_hash).is_err() {
        return Err(Error::PasswordRequired {
            username: user.username.clone(),
        });
    }
    Ok(())
}

fn seal_bearer_auth(bearer: &mut BearerAuth) -> Result<(), Error> {
    let mut names = HashSet::new();
    for token in &mut bearer.tokens {
        if token.name.trim().is_empty() || !names.insert(token.name.clone()) {
            return Err(Error::InvalidTokenName {
                name: token.name.clone(),
            });
        }
        seal_token(token)?;
    }
    Ok(())
}

fn seal_token(token: &mut BearerToken) -> Result<(), Error> {
    let invalid = || Error::InvalidToken {
        name: token.name.clone(),
    };
    if !token.token.is_empty() {
        if token.token.len() < BearerToken::MIN_LENGTH {
            return Err(invalid());
        }
        token.token_hash = hex::encode(Sha256::digest(token.token.as_bytes()));
        token.token.clear();
    }
    let valid_hash =
        token.token_hash.len() == 64 && token.token_hash.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid_hash {
        return Err(invalid());
    }
    Ok(())
}

fn validate_forward_auth(forward: &ForwardAuth) -> Result<(), Error> {
    if forward.timeout.is_zero() {
        return Err(Error::InvalidTimeout);
    }
    match forward
        .response_headers
        .iter()
        .find(|name| !is_header_name(name))
    {
        Some(name) => Err(Error::InvalidHeaderName { name: name.clone() }),
        None => Ok(()),
    }
}

fn hash_password(password: &str) -> Result<String, Error> {
    let salt = SaltString::generate(rand::thread_rng());
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| Error::FailedToHashPassword)
}

#[cfg(test)]
mod tests {
    use super::*;
    use argon2::PasswordVerifier;
    use r3v3rs3_api::proxy::HttpProxy;

    fn user(username: &str, password: &str, password_hash: &str) -> BasicAuthUser {
        BasicAuthUser {
            username: username.into(),
            password: password.into(),
            password_hash: password_hash.into(),
        }
    }

    fn proxy(users: Vec<BasicAuthUser>) -> Proxy {
        Proxy {
            kind: ProxyKind::Http(HttpProxy {
                auth: AuthPolicy::Basic(BasicAuth {
                    realm: String::new(),
                    users,
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn users(proxy: &Proxy) -> &[BasicAuthUser] {
        match &proxy.kind {
            ProxyKind::Http(HttpProxy {
                auth: AuthPolicy::Basic(basic),
                ..
            }) => &basic.users,
            _ => panic!("expected basic auth"),
        }
    }

    #[test]
    fn plain_password_is_replaced_with_hash() {
        let mut proxy = proxy(vec![user("alice", "secret", "")]);
        seal_proxy(&mut proxy).unwrap();
        let alice = &users(&proxy)[0];
        assert!(alice.password.is_empty());
        let hash = PasswordHash::new(&alice.password_hash).unwrap();
        assert!(Argon2::default().verify_password(b"secret", &hash).is_ok());

        let sealed = proxy.clone();
        seal_proxy(&mut proxy).unwrap();
        assert_eq!(proxy, sealed);
    }

    #[test]
    fn user_without_password_is_rejected() {
        let mut missing = proxy(vec![user("alice", "", "")]);
        assert!(matches!(
            seal_proxy(&mut missing),
            Err(Error::PasswordRequired { .. })
        ));
        let mut invalid_hash = proxy(vec![user("alice", "", "not-a-hash")]);
        assert!(matches!(
            seal_proxy(&mut invalid_hash),
            Err(Error::PasswordRequired { .. })
        ));
    }

    #[test]
    fn bearer_token_is_replaced_with_digest() {
        let bearer = |tokens: Vec<BearerToken>| Proxy {
            kind: ProxyKind::Http(HttpProxy {
                auth: AuthPolicy::Bearer(BearerAuth { tokens }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let token = |name: &str, token: &str| BearerToken {
            name: name.into(),
            token: token.into(),
            token_hash: String::new(),
        };

        let mut proxy = bearer(vec![token("ci", "0123456789abcdef")]);
        seal_proxy(&mut proxy).unwrap();
        let ProxyKind::Http(HttpProxy {
            auth: AuthPolicy::Bearer(sealed),
            ..
        }) = &proxy.kind
        else {
            panic!("expected bearer auth");
        };
        assert!(sealed.tokens[0].token.is_empty());
        assert_eq!(
            sealed.tokens[0].token_hash,
            hex::encode(Sha256::digest("0123456789abcdef"))
        );

        for tokens in [
            vec![token("ci", "short")],
            vec![token("ci", "")],
            vec![token("", "0123456789abcdef")],
            vec![
                token("ci", "0123456789abcdef"),
                token("ci", "0123456789abcdef"),
            ],
        ] {
            assert!(seal_proxy(&mut bearer(tokens)).is_err());
        }
    }

    #[test]
    fn forward_auth_headers_and_timeout_are_validated() {
        let forward = |response_headers: Vec<String>, timeout: u64| Proxy {
            kind: ProxyKind::Http(HttpProxy {
                auth: AuthPolicy::Forward(Box::new(ForwardAuth {
                    url: "http://127.0.0.1:4180/auth".parse().unwrap(),
                    response_headers,
                    timeout: std::time::Duration::from_secs(timeout),
                })),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(seal_proxy(&mut forward(vec!["X-Auth-User".into()], 10)).is_ok());
        assert!(matches!(
            seal_proxy(&mut forward(vec!["X Auth".into()], 10)),
            Err(Error::InvalidHeaderName { .. })
        ));
        assert!(matches!(
            seal_proxy(&mut forward(vec![], 0)),
            Err(Error::InvalidTimeout)
        ));
    }

    #[test]
    fn invalid_or_duplicate_username_is_rejected() {
        let mut colon = proxy(vec![user("al:ice", "secret", "")]);
        assert!(matches!(
            seal_proxy(&mut colon),
            Err(Error::InvalidUsername { .. })
        ));
        let mut duplicate = proxy(vec![user("bob", "a", ""), user("bob", "b", "")]);
        assert!(matches!(
            seal_proxy(&mut duplicate),
            Err(Error::InvalidUsername { .. })
        ));
    }
}
