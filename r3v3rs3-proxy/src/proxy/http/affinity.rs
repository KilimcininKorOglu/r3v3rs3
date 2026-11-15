//! Sticky sessions: a signed cookie that names the upstream server of a client in a route.

use super::cookie::{cookie_values, remove_cookie};
use crate::proxy::health::GroupKey;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD, prelude::BASE64_STANDARD};
use hmac::{Hmac, KeyInit, Mac};
use hyper::header::{HeaderMap, HeaderValue};
use r3v3rs3_api::{proxy::Server, upstream::StickyCookie};
use sha2::Sha256;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};
use subtle::ConstantTimeEq;
use tracing::error;

/// The `Set-Cookie` value that the upstream request hands to the response. It stays empty when
/// the client keeps its server.
pub type CookieSlot = Arc<Mutex<Option<HeaderValue>>>;

/// Signs the cookie values. Derived from the persistent master key (`r3v3rs3.key`) or generated
/// randomly on demand when uninitialized.
static KEY: RwLock<Option<[u8; 32]>> = RwLock::new(None);

/// Derives a 32-byte HMAC key from a master key using domain separation.
pub fn derive_affinity_key(master_key: &[u8]) -> [u8; 32] {
    let mut mac = match Hmac::<Sha256>::new_from_slice(master_key) {
        Ok(mac) => mac,
        Err(_) => {
            use sha2::Digest;
            let mut hasher = Sha256::new();
            hasher.update(master_key);
            hasher.update(b"r3v3rs3-sticky-session-affinity-v1");
            return hasher.finalize().into();
        }
    };
    mac.update(b"r3v3rs3-sticky-session-affinity-v1");
    mac.finalize().into_bytes().into()
}

/// Sets the global HMAC signing key for sticky session cookies.
pub fn init_key(key: [u8; 32]) {
    if let Ok(mut guard) = KEY.write() {
        *guard = Some(key);
    }
}

/// Initializes the affinity key by reading the persistent master key (`r3v3rs3.key`)
/// from the configuration directory.
pub fn init_from_config_dir(config_dir: &Path) {
    let primary = config_dir.join("r3v3rs3.key");
    let legacy = config_dir.join("platform.key");
    let key_bytes = load_key_from_file(&primary).or_else(|| load_key_from_file(&legacy));
    if let Some(bytes) = key_bytes {
        init_key(derive_affinity_key(&bytes));
    }
}

fn load_key_from_file(path: &Path) -> Option<Vec<u8>> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_master_key_bytes(&text)
}

fn parse_master_key_bytes(text: &str) -> Option<Vec<u8>> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))?;
    BASE64_STANDARD.decode(line).ok()
}

fn get_key() -> [u8; 32] {
    if let Ok(guard) = KEY.read()
        && let Some(key) = *guard
    {
        return key;
    }
    generate_and_store_key()
}

fn generate_and_store_key() -> [u8; 32] {
    if let Ok(mut guard) = KEY.write() {
        if let Some(key) = *guard {
            return key;
        }
        let generated: [u8; 32] = rand::random();
        *guard = Some(generated);
        return generated;
    }
    rand::random()
}

/// The sticky cookie of a route.
#[derive(Debug)]
pub struct Affinity {
    name: String,
    /// The cookie value of each server, in server order.
    tokens: Vec<String>,
    /// The `Set-Cookie` values of each server: without and with `Secure`.
    cookies: Vec<(HeaderValue, HeaderValue)>,
}

impl Affinity {
    /// Signs a cookie value for each server of the route. `None` when a cookie is not a valid
    /// header value, e.g. because the route path has a control character.
    pub fn new(
        config: &StickyCookie,
        key: GroupKey,
        servers: &[Server],
        base_path: &str,
    ) -> Option<Self> {
        let (proxy, route) = key;
        let path = if base_path.is_empty() { "/" } else { base_path };
        let max_age = config
            .max_age
            .map(|max_age| format!("; Max-Age={}", max_age.as_secs()))
            .unwrap_or_default();
        let attributes = format!("; Path={path}; HttpOnly; SameSite=Lax{max_age}");
        let tokens = servers
            .iter()
            .map(|server| sign(&format!("{proxy}/{}/{}", route.unwrap_or(0), server.url)))
            .collect::<Option<Vec<_>>>()?;
        let cookies = tokens
            .iter()
            .map(|token| {
                let cookie = format!("{}={token}{attributes}", config.name);
                let plain = HeaderValue::from_str(&cookie);
                let secure = HeaderValue::from_str(&format!("{cookie}; Secure"));
                plain.and_then(|plain| Ok((plain, secure?)))
            })
            .collect::<Result<Vec<_>, _>>();
        match cookies {
            Ok(cookies) => Some(Self {
                name: config.name.clone(),
                tokens,
                cookies,
            }),
            Err(err) => {
                error!(%err, %proxy, path, "the sticky cookie is not a valid header value");
                None
            }
        }
    }

    /// The server that the cookie of the request names. A value that r3v3rs3 did not sign for a
    /// server of this route names no server.
    pub fn pinned(&self, headers: &HeaderMap) -> Option<usize> {
        cookie_values(headers, &self.name).find_map(|value| {
            self.tokens
                .iter()
                .position(|token| bool::from(token.as_bytes().ct_eq(value.as_bytes())))
        })
    }

    /// Removes the cookie from the `Cookie` headers, so the upstream server does not receive it.
    pub fn strip(&self, headers: &mut HeaderMap) {
        remove_cookie(headers, &self.name);
    }

    /// The `Set-Cookie` value that pins the client to the server. `secure` adds `Secure`.
    pub fn set_cookie(&self, index: usize, secure: bool) -> Option<HeaderValue> {
        self.cookies
            .get(index)
            .map(|(plain, with_secure)| if secure { with_secure } else { plain }.clone())
    }
}

/// The HMAC-SHA256 of the input with the process key, in unpadded base64url.
fn sign(input: &str) -> Option<String> {
    let key = get_key();
    let mut mac = match Hmac::<Sha256>::new_from_slice(&key) {
        Ok(mac) => mac,
        Err(err) => {
            error!(%err, "failed to create the sticky cookie signer");
            return None;
        }
    };
    mac.update(input.as_bytes());
    Some(URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::COOKIE;
    use std::time::Duration;

    fn servers() -> Vec<Server> {
        ["http://10.0.0.1:8080/", "http://10.0.0.2:8080/"]
            .into_iter()
            .map(|url| Server::new(url.parse().unwrap()))
            .collect()
    }

    fn affinity(route: usize, base_path: &str, max_age: Option<Duration>) -> Affinity {
        let config = StickyCookie {
            enabled: true,
            max_age,
            ..Default::default()
        };
        let key = ("sticky".parse().unwrap(), Some(route));
        Affinity::new(&config, key, &servers(), base_path).unwrap()
    }

    fn cookie_value(affinity: &Affinity, index: usize) -> String {
        let cookie = affinity.set_cookie(index, false).unwrap();
        let pair = cookie.to_str().unwrap().split(';').next().unwrap();
        pair.split_once('=').unwrap().1.to_string()
    }

    fn headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(COOKIE, value.parse().unwrap());
        }
        headers
    }

    #[test]
    fn only_a_signed_value_of_the_route_names_a_server() {
        let affinity = affinity(0, "", None);
        let second = cookie_value(&affinity, 1);
        let signed = headers(&["a=1", &format!("b=2; r3v3rs3_affinity={second}")]);
        assert_eq!(affinity.pinned(&signed), Some(1));

        let forged = headers(&["r3v3rs3_affinity=http://10.0.0.1:8080/"]);
        assert_eq!(affinity.pinned(&forged), None);

        let other_route = self::affinity(1, "/api", None);
        let foreign = headers(&[&format!(
            "r3v3rs3_affinity={}",
            cookie_value(&other_route, 1)
        )]);
        assert_eq!(affinity.pinned(&foreign), None);
        assert_eq!(affinity.pinned(&HeaderMap::new()), None);
    }

    #[test]
    fn strip_removes_only_the_sticky_cookie() {
        let affinity = affinity(0, "", None);
        let mut mixed = headers(&["a=1; r3v3rs3_affinity=x; b=2", "r3v3rs3_affinity=y", "c=3"]);
        affinity.strip(&mut mixed);
        let values = mixed.get_all(COOKIE).iter().collect::<Vec<_>>();
        assert_eq!(values, ["a=1; b=2", "c=3"]);

        let mut only = headers(&["r3v3rs3_affinity=x"]);
        affinity.strip(&mut only);
        assert!(!only.contains_key(COOKIE));
    }

    #[test]
    fn set_cookie_has_the_route_path_and_the_flags() {
        let session = affinity(0, "", None);
        let cookie = session.set_cookie(0, false).unwrap();
        let value = cookie_value(&session, 0);
        assert_eq!(
            cookie,
            format!("r3v3rs3_affinity={value}; Path=/; HttpOnly; SameSite=Lax").as_str()
        );

        let persistent = affinity(1, "/api", Some(Duration::from_secs(3600)));
        let cookie = persistent.set_cookie(1, true).unwrap();
        assert!(
            cookie
                .to_str()
                .unwrap()
                .ends_with("; Path=/api; HttpOnly; SameSite=Lax; Max-Age=3600; Secure")
        );
        assert!(persistent.set_cookie(2, true).is_none());
        assert_ne!(cookie_value(&session, 0), cookie_value(&session, 1));
    }

    #[test]
    fn affinity_key_is_persistent_when_derived_from_same_master_key() {
        let master_key = b"01234567890123456789012345678901";
        let derived1 = derive_affinity_key(master_key);
        let derived2 = derive_affinity_key(master_key);
        assert_eq!(derived1, derived2);

        init_key(derived1);
        let aff1 = affinity(0, "", None);
        let val1 = cookie_value(&aff1, 0);

        init_key(derived2);
        let aff2 = affinity(0, "", None);
        let val2 = cookie_value(&aff2, 0);

        assert_eq!(val1, val2);

        let req_headers = headers(&[&format!("r3v3rs3_affinity={val1}")]);
        assert_eq!(aff2.pinned(&req_headers), Some(0));
    }
}
