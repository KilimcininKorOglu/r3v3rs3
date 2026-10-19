use crate::proxy::http::error::ProxyError;
use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::phc::PasswordHash};
use base64::{Engine, engine::general_purpose::STANDARD};
use hyper::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use once_cell::sync::Lazy;
use r3v3rs3_api::policy::BasicAuth;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
};
use subtle::ConstantTimeEq;
use tracing::error;

const DEFAULT_REALM: &str = "r3v3rs3";

/// Maximum number of verified credentials kept in memory for each authenticator.
const MAX_VERIFIED_CREDENTIALS: usize = 1024;

/// Hash that the password of an unknown user is verified against, so the response time does not
/// reveal which usernames exist.
static UNKNOWN_USER_HASH: Lazy<Option<String>> =
    Lazy::new(|| match Argon2::default().hash_password(b"") {
        Ok(hash) => Some(hash.to_string()),
        Err(err) => {
            error!(%err, "failed to hash the unknown user password");
            None
        }
    });

#[derive(Clone, Debug, PartialEq, Eq)]
enum StoredPassword {
    Argon2(String),
    Plain(String),
}

#[derive(Default)]
struct FileUsers {
    last_modified: Option<std::time::SystemTime>,
    users: HashMap<String, StoredPassword>,
}

/// HTTP Basic authentication (RFC 7617) with argon2 password hashes or user file.
pub struct BasicAuthenticator {
    /// Password hash of each username defined in configuration.
    users: HashMap<String, StoredPassword>,
    user_file: Option<PathBuf>,
    file_users: Mutex<FileUsers>,
    challenge: HeaderValue,
    /// SHA-256 digests of credentials that passed verification.
    verified: Mutex<HashSet<[u8; 32]>>,
}

impl fmt::Debug for BasicAuthenticator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BasicAuthenticator")
            .field("users", &self.users.keys())
            .field("user_file", &self.user_file)
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

fn parse_user_line(line: &str) -> Option<(String, StoredPassword)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (username, secret) = line.split_once(':')?;
    let username = username.trim();
    let secret = secret.trim();
    if username.is_empty() || secret.is_empty() {
        return None;
    }
    let password = if secret.starts_with("$argon2") {
        StoredPassword::Argon2(secret.to_string())
    } else {
        StoredPassword::Plain(secret.strip_prefix("{PLAIN}").unwrap_or(secret).to_string())
    };
    Some((username.to_string(), password))
}

fn parse_user_file(content: &str) -> HashMap<String, StoredPassword> {
    content.lines().filter_map(parse_user_line).collect()
}

impl BasicAuthenticator {
    pub fn new(config: BasicAuth) -> Self {
        let users = config
            .users
            .into_iter()
            .filter(|user| match PasswordHash::new(&user.password_hash) {
                Ok(_) => true,
                Err(err) => {
                    error!(username = %user.username, %err, "ignoring basic auth user with an invalid password hash");
                    false
                }
            })
            .map(|user| (user.username, StoredPassword::Argon2(user.password_hash)))
            .collect();
        let (user_file, file_users) = match config.user_file {
            Some(path) => {
                let path = PathBuf::from(path);
                let mut users = FileUsers::default();
                if let Ok(meta) = std::fs::metadata(&path) {
                    users.last_modified = meta.modified().ok();
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        users.users = parse_user_file(&content);
                    }
                }
                (Some(path), Mutex::new(users))
            }
            None => (None, Mutex::new(FileUsers::default())),
        };
        Self {
            users,
            user_file,
            file_users,
            challenge: challenge(&config.realm),
            verified: Mutex::new(HashSet::new()),
        }
    }

    pub async fn authorize(&self, headers: &mut HeaderMap) -> Result<(), ProxyError> {
        let Some((username, password)) = headers.get(AUTHORIZATION).and_then(parse_credentials)
        else {
            return Err(self.unauthorized());
        };
        if !self.verify(&username, password).await {
            return Err(self.unauthorized());
        }
        headers.remove(AUTHORIZATION);
        Ok(())
    }

    fn unauthorized(&self) -> ProxyError {
        ProxyError::Unauthorized {
            challenge: self.challenge.clone(),
        }
    }

    fn get_user(&self, username: &str) -> Option<StoredPassword> {
        if let Some(user) = self.users.get(username) {
            return Some(user.clone());
        }
        if let Some(path) = &self.user_file {
            self.reload_file_if_modified(path);
            let state = self
                .file_users
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            return state.users.get(username).cloned();
        }
        None
    }

    fn reload_file_if_modified(&self, path: &Path) {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        let modified = meta.modified().ok();
        let mut state = self
            .file_users
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.last_modified == modified && !state.users.is_empty() {
            return;
        }
        state.last_modified = modified;
        if let Ok(content) = std::fs::read_to_string(path) {
            state.users = parse_user_file(&content);
            self.lock_verified().clear();
        }
    }

    async fn verify(&self, username: &str, password: String) -> bool {
        let Some(stored) = self.get_user(username) else {
            if let Some(hash) = UNKNOWN_USER_HASH.as_ref() {
                verify_password(hash.clone(), password).await;
            }
            return false;
        };
        let digest = credential_digest(&stored, &password);
        if self.lock_verified().contains(&digest) {
            return true;
        }
        let valid = match stored {
            StoredPassword::Argon2(hash) => verify_password(hash, password).await,
            StoredPassword::Plain(plain) => bool::from(plain.as_bytes().ct_eq(password.as_bytes())),
        };
        if valid {
            let mut verified = self.lock_verified();
            if verified.len() >= MAX_VERIFIED_CREDENTIALS {
                verified.clear();
            }
            verified.insert(digest);
        }
        valid
    }

    fn lock_verified(&self) -> std::sync::MutexGuard<'_, HashSet<[u8; 32]>> {
        self.verified.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Builds the `WWW-Authenticate` value. Control characters are removed from the realm and quotes
/// are escaped, so the value is always a valid header.
fn challenge(realm: &str) -> HeaderValue {
    let realm = if realm.is_empty() {
        DEFAULT_REALM
    } else {
        realm
    };
    let realm = realm
        .chars()
        .filter(|c| !c.is_control())
        .fold(String::new(), |mut quoted, c| {
            if c == '"' || c == '\\' {
                quoted.push('\\');
            }
            quoted.push(c);
            quoted
        });
    let value = format!("Basic realm=\"{realm}\", charset=\"UTF-8\"");
    HeaderValue::from_bytes(value.as_bytes()).unwrap_or_else(|err| {
        error!(%err, "invalid basic auth realm");
        HeaderValue::from_static("Basic realm=\"r3v3rs3\", charset=\"UTF-8\"")
    })
}

/// Returns the username and password of a `Basic` authorization header.
fn parse_credentials(value: &HeaderValue) -> Option<(String, String)> {
    let (scheme, token) = value.to_str().ok()?.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = String::from_utf8(STANDARD.decode(token.trim()).ok()?).ok()?;
    let (username, password) = decoded.split_once(':')?;
    Some((username.to_owned(), password.to_owned()))
}

fn credential_digest(secret: &StoredPassword, password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    match secret {
        StoredPassword::Argon2(hash) => {
            hasher.update(b"argon2:");
            hasher.update(hash.as_bytes());
        }
        StoredPassword::Plain(plain) => {
            hasher.update(b"plain:");
            hasher.update(plain.as_bytes());
        }
    }
    hasher.update([0]);
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

/// Verifies the password on a blocking thread, because argon2 takes CPU time.
async fn verify_password(hash: String, password: String) -> bool {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .verify_password(password.as_bytes(), hash.as_str())
            .is_ok()
    })
    .await
    .unwrap_or_else(|err| {
        error!(%err, "password verification task failed");
        false
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::policy::BasicAuthUser;

    fn authenticator(realm: &str) -> BasicAuthenticator {
        let hash = Argon2::default()
            .hash_password(b"s3cr:et")
            .unwrap()
            .to_string();
        BasicAuthenticator::new(BasicAuth {
            realm: realm.into(),
            users: vec![
                BasicAuthUser {
                    username: "alice".into(),
                    password: String::new(),
                    password_hash: hash,
                    password_set: false,
                },
                BasicAuthUser {
                    username: "broken".into(),
                    password: String::new(),
                    password_hash: "not-a-hash".into(),
                    password_set: false,
                },
            ],
            user_file: None,
        })
    }

    fn basic(credentials: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        let value = format!("Basic {}", STANDARD.encode(credentials));
        headers.insert(AUTHORIZATION, value.parse().unwrap());
        headers
    }

    #[test]
    fn parse_credentials_accepts_basic_scheme_only() {
        let header = |value: &str| HeaderValue::from_str(value).unwrap();
        assert_eq!(
            parse_credentials(&header("basic YWxpY2U6czNjcjpldA==")),
            Some(("alice".into(), "s3cr:et".into()))
        );
        assert_eq!(
            parse_credentials(&header("Bearer YWxpY2U6czNjcjpldA==")),
            None
        );
        assert_eq!(parse_credentials(&header("Basic not base64!")), None);
        assert_eq!(parse_credentials(&header("Basic YWxpY2U=")), None);
    }

    #[test]
    fn challenge_escapes_the_realm() {
        assert_eq!(challenge(""), "Basic realm=\"r3v3rs3\", charset=\"UTF-8\"");
        assert_eq!(
            challenge("St\"aff\\\nZone"),
            "Basic realm=\"St\\\"aff\\\\Zone\", charset=\"UTF-8\""
        );
    }

    #[tokio::test]
    async fn authorize_checks_the_password_and_removes_the_header() {
        let auth = authenticator("Staff");
        assert!(auth.users.contains_key("alice"));
        assert!(!auth.users.contains_key("broken"));

        let mut headers = basic("alice:s3cr:et");
        assert!(auth.authorize(&mut headers).await.is_ok());
        assert!(headers.get(AUTHORIZATION).is_none());
        assert_eq!(auth.lock_verified().len(), 1);

        let mut headers = basic("alice:s3cr:et");
        assert!(auth.authorize(&mut headers).await.is_ok());

        for credentials in ["alice:wrong", "bob:s3cr:et", "broken:"] {
            let mut headers = basic(credentials);
            let err = auth.authorize(&mut headers).await.unwrap_err();
            assert!(matches!(err, ProxyError::Unauthorized { .. }));
            assert!(headers.get(AUTHORIZATION).is_some());
        }
        let err = auth.authorize(&mut HeaderMap::new()).await.unwrap_err();
        let ProxyError::Unauthorized { challenge } = err else {
            panic!("expected unauthorized");
        };
        assert_eq!(challenge, "Basic realm=\"Staff\", charset=\"UTF-8\"");
        assert_eq!(auth.lock_verified().len(), 1);
    }

    #[tokio::test]
    async fn authorize_with_user_file() {
        let dir = std::env::temp_dir().join(format!("r3v3rs3-userfile-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("users.txt");
        let hash = Argon2::default()
            .hash_password(b"argonpass")
            .unwrap()
            .to_string();
        std::fs::write(
            &file_path,
            format!("# Comments\nplainuser:plainpass\nhashuser:{hash}\n"),
        )
        .unwrap();

        let auth = BasicAuthenticator::new(BasicAuth {
            realm: "FileRealm".into(),
            users: vec![],
            user_file: Some(file_path.to_str().unwrap().to_string()),
        });

        let mut headers = basic("plainuser:plainpass");
        assert!(auth.authorize(&mut headers).await.is_ok());

        let mut headers = basic("hashuser:argonpass");
        assert!(auth.authorize(&mut headers).await.is_ok());

        let mut headers = basic("plainuser:wrong");
        assert!(auth.authorize(&mut headers).await.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
