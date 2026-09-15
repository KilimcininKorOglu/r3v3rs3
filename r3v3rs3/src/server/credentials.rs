use argon2::{
    password_hash::{PasswordHash, SaltString},
    Argon2, PasswordHasher,
};
use r3v3rs3_api::{
    error::Error,
    header_rules::HeaderRules,
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
    std::iter::once(&mut http.auth)
        .chain(route_policies)
        .try_for_each(seal_policy)?;
    http.compression.validate()?;
    http.cache.validate()?;
    let route_rules = http
        .routes
        .iter()
        .filter_map(|route| route.headers.as_ref());
    std::iter::once(&http.headers)
        .chain(route_rules)
        .try_for_each(HeaderRules::validate)
}

/// Replaces the plain text passwords and tokens of a policy with hashes and validates the policy.
pub fn seal_policy(policy: &mut AuthPolicy) -> Result<(), Error> {
    match policy {
        AuthPolicy::None | AuthPolicy::Session => Ok(()),
        AuthPolicy::Basic(basic) => seal_basic_auth(basic),
        AuthPolicy::Bearer(bearer) => seal_bearer_auth(bearer),
        AuthPolicy::Forward(forward) => validate_forward_auth(forward),
    }
}

/// Runs [`seal_proxy`] on a blocking thread.
pub async fn seal(proxy: Proxy) -> Result<Proxy, Error> {
    seal_blocking(proxy, seal_proxy).await
}

/// Runs `seal` on a blocking thread, because hashing a password takes CPU time.
pub async fn seal_blocking<T: Send + 'static>(
    mut value: T,
    seal: fn(&mut T) -> Result<(), Error>,
) -> Result<T, Error> {
    tokio::task::spawn_blocking(move || seal(&mut value).map(|()| value))
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

/// The auth policies of an HTTP proxy with the path of their route. The proxy policy has no path.
fn policies_mut(proxy: &mut Proxy) -> Vec<(Option<&str>, &mut AuthPolicy)> {
    let ProxyKind::Http(http) = &mut proxy.kind else {
        return Vec::new();
    };
    let routes = http
        .routes
        .iter_mut()
        .filter_map(|route| Some((Some(route.path.as_str()), route.auth.as_mut()?)));
    std::iter::once((None, &mut http.auth))
        .chain(routes)
        .collect()
}

/// Removes the password hashes and the token digests that the admin API returns, and marks the
/// users and the tokens that have a secret.
pub fn mask_proxy(proxy: &mut Proxy) {
    for (_, policy) in policies_mut(proxy) {
        mask_policy(policy);
    }
}

/// Removes the password hashes and the token digests of a policy, and marks the users and the
/// tokens that have a secret.
pub fn mask_policy(policy: &mut AuthPolicy) {
    match policy {
        AuthPolicy::Basic(basic) => basic.users.iter_mut().for_each(|user| {
            user.password_set = !user.password_hash.is_empty();
            user.password_hash.clear();
        }),
        AuthPolicy::Bearer(bearer) => bearer.tokens.iter_mut().for_each(|token| {
            token.token_set = !token.token_hash.is_empty();
            token.token_hash.clear();
        }),
        _ => {}
    }
}

/// Gives the users and the tokens of an update without a secret the hashes of the current proxy.
/// A user or a token matches by its name and the path of its route.
pub fn restore_secrets(proxy: &mut Proxy, current: &Proxy) {
    let mut current = current.clone();
    let mut previous = policies_mut(&mut current);
    for (path, policy) in policies_mut(proxy) {
        let old = previous.iter_mut().find(|(other, _)| *other == path);
        if let Some((_, old)) = old {
            restore_policy(policy, old);
        }
    }
}

/// Gives the users and the tokens of `policy` without a secret the hashes of the users and the
/// tokens with the same names in `previous`.
pub fn restore_policy(policy: &mut AuthPolicy, previous: &mut AuthPolicy) {
    match (policy, previous) {
        (AuthPolicy::Basic(basic), AuthPolicy::Basic(old)) => {
            restore(&mut basic.users, &mut old.users, user_secret)
        }
        (AuthPolicy::Bearer(bearer), AuthPolicy::Bearer(old)) => {
            restore(&mut bearer.tokens, &mut old.tokens, token_secret)
        }
        _ => {}
    }
}

/// The name, the new plain text secret and the hash of a user or a token.
type SecretParts<T> = fn(&mut T) -> (&str, &str, &mut String);

fn user_secret(user: &mut BasicAuthUser) -> (&str, &str, &mut String) {
    (&user.username, &user.password, &mut user.password_hash)
}

fn token_secret(token: &mut BearerToken) -> (&str, &str, &mut String) {
    (&token.name, &token.token, &mut token.token_hash)
}

/// Copies the hash of the previous item with the same name to each item without a secret.
fn restore<T>(items: &mut [T], previous: &mut [T], parts: SecretParts<T>) {
    for item in items.iter_mut() {
        let (name, secret, hash) = parts(item);
        if !secret.is_empty() || !hash.is_empty() {
            continue;
        }
        let old = previous
            .iter_mut()
            .map(parts)
            .find(|(old_name, _, _)| *old_name == name);
        if let Some((_, _, old_hash)) = old {
            hash.clone_from(old_hash);
        }
    }
}

fn seal_user(user: &mut BasicAuthUser) -> Result<(), Error> {
    user.password_set = false;
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
    token.token_set = false;
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
            password_set: false,
        }
    }

    fn http_proxy(http: HttpProxy) -> Proxy {
        Proxy {
            kind: ProxyKind::Http(Box::new(http)),
            ..Default::default()
        }
    }

    fn proxy(users: Vec<BasicAuthUser>) -> Proxy {
        http_proxy(HttpProxy {
            auth: AuthPolicy::Basic(BasicAuth {
                realm: String::new(),
                users,
            }),
            ..Default::default()
        })
    }

    fn auth(proxy: &Proxy) -> &AuthPolicy {
        let ProxyKind::Http(http) = &proxy.kind else {
            panic!("expected an HTTP proxy");
        };
        &http.auth
    }

    fn users(proxy: &Proxy) -> &[BasicAuthUser] {
        let AuthPolicy::Basic(basic) = auth(proxy) else {
            panic!("expected basic auth");
        };
        &basic.users
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
    fn a_masked_update_keeps_the_hashes_of_the_same_names() {
        let mut current = proxy(vec![user("alice", "", "$argon2id$alice")]);
        let ProxyKind::Http(http) = &mut current.kind else {
            panic!("expected an HTTP proxy");
        };
        let mut route = r3v3rs3_api::proxy::Route {
            path: "/api".into(),
            ..Default::default()
        };
        route.auth = Some(AuthPolicy::Bearer(BearerAuth {
            tokens: vec![BearerToken {
                name: "ci".into(),
                token_hash: "digest".into(),
                ..Default::default()
            }],
        }));
        http.routes.push(route);

        let mut masked = current.clone();
        mask_proxy(&mut masked);
        assert!(users(&masked)[0].password_hash.is_empty());
        assert!(users(&masked)[0].password_set);
        let ProxyKind::Http(http) = &masked.kind else {
            panic!("expected an HTTP proxy");
        };
        let Some(AuthPolicy::Bearer(bearer)) = &http.routes[0].auth else {
            panic!("expected bearer auth");
        };
        assert!(bearer.tokens[0].token_hash.is_empty());
        assert!(bearer.tokens[0].token_set);

        let mut update = masked.clone();
        restore_secrets(&mut update, &current);
        mask_proxy(&mut update);
        assert_eq!(update, masked);
        let mut update = masked;
        restore_secrets(&mut update, &current);
        assert_eq!(users(&update)[0].password_hash, "$argon2id$alice");

        let mut renamed = proxy(vec![user("bob", "", "")]);
        restore_secrets(&mut renamed, &current);
        assert!(users(&renamed)[0].password_hash.is_empty());
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
        let bearer = |tokens: Vec<BearerToken>| {
            http_proxy(HttpProxy {
                auth: AuthPolicy::Bearer(BearerAuth { tokens }),
                ..Default::default()
            })
        };
        let token = |name: &str, token: &str| BearerToken {
            name: name.into(),
            token: token.into(),
            token_hash: String::new(),
            token_set: false,
        };

        let mut proxy = bearer(vec![token("ci", "0123456789abcdef")]);
        seal_proxy(&mut proxy).unwrap();
        let AuthPolicy::Bearer(sealed) = auth(&proxy) else {
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
        let forward = |response_headers: Vec<String>, timeout: u64| {
            http_proxy(HttpProxy {
                auth: AuthPolicy::Forward(Box::new(ForwardAuth {
                    url: "http://127.0.0.1:4180/auth".parse().unwrap(),
                    response_headers,
                    timeout: std::time::Duration::from_secs(timeout),
                })),
                ..Default::default()
            })
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
    fn header_rules_are_validated() {
        use r3v3rs3_api::header_rules::HeaderRule;
        let remove = |name: &str| {
            http_proxy(HttpProxy {
                headers: HeaderRules {
                    request: vec![HeaderRule::Remove { name: name.into() }],
                    response: vec![],
                },
                ..Default::default()
            })
        };
        assert!(seal_proxy(&mut remove("X-Secret")).is_ok());
        assert!(matches!(
            seal_proxy(&mut remove("Host")),
            Err(Error::ProtectedHeader { .. })
        ));
        assert!(matches!(
            seal_proxy(&mut remove("X Secret")),
            Err(Error::InvalidHeaderName { .. })
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
