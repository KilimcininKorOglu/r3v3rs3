use crate::error::Error;
use crate::proxy::ServerUrl;
use ipnet::IpNet;
use serde_derive::{Deserialize, Serialize};
use std::{net::IpAddr, time::Duration};
use utoipa::ToSchema;

/// Allows or denies requests by the resolved client IP address and country.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct IpFilter {
    /// When not empty, only clients in these blocks are allowed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = [String], example = json!(["192.168.0.0/16"]))]
    pub allow: Vec<IpNet>,

    /// Clients in these blocks are always rejected, even when they are in `allow`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = [String], example = json!(["203.0.113.0/24"]))]
    pub deny: Vec<IpNet>,

    /// When not empty, only clients from these ISO country codes (e.g. ["TR", "US"]) are allowed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = [String], example = json!(["TR", "US"]))]
    pub allow_countries: Vec<String>,

    /// Clients from these ISO country codes are always rejected, even when in `allow_countries`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = [String], example = json!(["RU", "CN"]))]
    pub deny_countries: Vec<String>,
}

impl IpFilter {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty()
            && self.deny.is_empty()
            && self.allow_countries.is_empty()
            && self.deny_countries.is_empty()
    }

    pub fn allows(&self, ip: IpAddr) -> bool {
        self.allows_with_country(ip, None)
    }

    fn is_country_denied(&self, country: Option<&str>) -> bool {
        match country {
            Some(c) => self
                .deny_countries
                .iter()
                .any(|d| d.eq_ignore_ascii_case(c)),
            None => false,
        }
    }

    fn matches_country_allow(&self, country: Option<&str>) -> bool {
        match country {
            Some(c) => self
                .allow_countries
                .iter()
                .any(|a| a.eq_ignore_ascii_case(c)),
            None => false,
        }
    }

    pub fn allows_with_country(&self, ip: IpAddr, country: Option<&str>) -> bool {
        let ip = ip.to_canonical();
        if self.deny.iter().any(|net| net.contains(&ip)) || self.is_country_denied(country) {
            return false;
        }
        let in_allow_cidr =
            !self.allow.is_empty() && self.allow.iter().any(|net| net.contains(&ip));
        if in_allow_cidr {
            return true;
        }
        if !self.allow_countries.is_empty() {
            return self.matches_country_allow(country);
        }
        self.allow.is_empty()
    }
}

/// Limits the request rate of each client IP address.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RateLimit {
    /// Requests allowed in each period. Zero disables the limit.
    #[serde(default)]
    pub requests: u32,

    #[serde(default)]
    pub per: RatePeriod,

    /// Requests a client can send at once before the limit applies. Zero uses `requests`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub burst: u32,
}

impl RateLimit {
    pub fn is_disabled(&self) -> bool {
        self.requests == 0
    }

    /// Time until a client can send one more request after it reaches the limit.
    pub fn replenish_interval(&self) -> Option<Duration> {
        (self.requests > 0).then(|| self.per.duration() / self.requests)
    }

    pub fn burst_size(&self) -> u32 {
        if self.burst == 0 {
            self.requests
        } else {
            self.burst
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RatePeriod {
    #[default]
    Second,
    Minute,
    Hour,
}

impl RatePeriod {
    pub const ALL: [RatePeriod; 3] = [RatePeriod::Second, RatePeriod::Minute, RatePeriod::Hour];

    pub fn duration(self) -> Duration {
        match self {
            Self::Second => Duration::from_secs(1),
            Self::Minute => Duration::from_secs(60),
            Self::Hour => Duration::from_secs(3600),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Second => "second",
            Self::Minute => "minute",
            Self::Hour => "hour",
        }
    }
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// Limits concurrent in-flight requests per client IP address or request header (e.g. API key).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ConcurrencyLimit {
    /// Maximum concurrent in-flight requests allowed. Zero disables the limit.
    #[serde(default)]
    pub max: u32,

    /// Request header name to group concurrency by (e.g. "X-API-Key").
    /// When None or empty, the client IP address is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
}

impl ConcurrencyLimit {
    pub fn is_disabled(&self) -> bool {
        self.max == 0
    }

    pub fn validate(&self) -> Result<(), Error> {
        if let Some(ref header) = self.header
            && !is_header_name(header)
        {
            return Err(Error::InvalidHeaderName {
                name: header.clone(),
            });
        }
        Ok(())
    }
}

/// Requires clients to authenticate before a request reaches the upstream server.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthPolicy {
    #[default]
    None,
    Basic(BasicAuth),
    Bearer(BearerAuth),
    /// Boxed, because the URL makes this variant much larger than the others.
    Forward(Box<ForwardAuth>),
    /// Requires a sign-in with a panel account on the proxied host.
    Session,
    /// OpenID Connect / OAuth 2.0 authentication.
    Oidc(Box<OidcAuth>),
}

impl AuthPolicy {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

/// HTTP Basic authentication (RFC 7617).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BasicAuth {
    /// Protection space shown by the browser. Empty uses `r3v3rs3`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub realm: String,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<BasicAuthUser>,

    /// Path to a file on disk containing username:password or username:hash pairs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_file: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BasicAuthUser {
    pub username: String,

    /// New plain text password. The server replaces it with `password_hash` and never stores it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,

    /// Argon2 PHC string of the password. The admin API does not return it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password_hash: String,

    /// Set by the admin API when the user has a password. An update without `password` and
    /// `password_hash` keeps the password of the user with the same name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub password_set: bool,
}

impl BasicAuthUser {
    /// Returns an error message when the username cannot be sent in a Basic credential.
    pub fn username_error(&self) -> Option<&'static str> {
        if self.username.is_empty() {
            Some("Username is required")
        } else if self.username.contains(':') {
            Some("Username must not contain a colon")
        } else if self.username.chars().any(char::is_control) {
            Some("Username must not contain control characters")
        } else {
            None
        }
    }
}

/// Bearer token authentication (RFC 6750).
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BearerAuth {
    #[serde(default)]
    pub tokens: Vec<BearerToken>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BearerToken {
    /// Label that identifies the token.
    pub name: String,

    /// New plain text token. The server replaces it with `token_hash` and never stores it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,

    /// Lowercase hex SHA-256 digest of the token. The admin API does not return it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_hash: String,

    /// Set by the admin API when the token has a value. An update without `token` and
    /// `token_hash` keeps the value of the token with the same name.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub token_set: bool,
}

impl BearerToken {
    /// Tokens are hashed without a salt, so they must be long random values.
    pub const MIN_LENGTH: usize = 16;
}

/// Delegates authentication to an external HTTP service, like nginx `auth_request`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ForwardAuth {
    /// The service that receives a GET request for each client request.
    #[schema(value_type = String, example = "http://127.0.0.1:4180/oauth2/auth")]
    pub url: ServerUrl,

    /// Headers of a successful auth response that are copied to the upstream request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_headers: Vec<String>,

    #[serde(default = "default_forward_auth_timeout", with = "humantime_serde")]
    #[schema(value_type = String, example = "10s")]
    pub timeout: Duration,
}

pub const DEFAULT_FORWARD_AUTH_TIMEOUT: Duration = Duration::from_secs(10);

fn default_forward_auth_timeout() -> Duration {
    DEFAULT_FORWARD_AUTH_TIMEOUT
}

/// OpenID Connect / OAuth 2.0 authentication.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OidcAuth {
    /// OIDC Issuer URL (e.g. `https://accounts.google.com` or `https://auth.example.com/realms/main`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub issuer_url: String,

    /// Authorization endpoint URL. If empty and issuer_url is provided, standard paths or discovery are used.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub authorization_url: String,

    /// Token endpoint URL.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_url: String,

    /// UserInfo endpoint URL.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub userinfo_url: String,

    /// OAuth 2.0 Client ID.
    pub client_id: String,

    /// OAuth 2.0 Client Secret. The server seals it and never returns it in plain text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_secret: String,

    /// Set by the server when a client_secret is configured.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub client_secret_set: bool,

    /// Requested scopes (defaults to ["openid", "email", "profile"]).
    #[serde(default = "default_oidc_scopes", skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,

    /// Allowed emails or domain wildcard patterns (e.g. ["*@example.com", "admin@corp.com"]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_emails: Vec<String>,
}

pub fn default_oidc_scopes() -> Vec<String> {
    vec!["openid".into(), "email".into(), "profile".into()]
}

/// Checks whether an email matches any pattern in the allowed list. Empty allowed list matches all.
pub fn is_email_allowed(email: &str, allowed: &[String]) -> bool {
    if allowed.is_empty() {
        return true;
    }
    let email_lower = email.to_lowercase();
    allowed.iter().any(|pattern| {
        let pat = pattern.trim().to_lowercase();
        if pat == "*" {
            true
        } else if let Some(suffix) = pat.strip_prefix("*@") {
            email_lower.ends_with(&format!("@{suffix}"))
        } else if let Some(suffix) = pat.strip_prefix('@') {
            email_lower.ends_with(&format!("@{suffix}"))
        } else {
            email_lower == pat
        }
    })
}

/// Returns true when `name` is a valid HTTP header name (an RFC 9110 token).
pub fn is_header_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(allow: &[&str], deny: &[&str]) -> IpFilter {
        IpFilter {
            allow: allow.iter().map(|net| net.parse().unwrap()).collect(),
            deny: deny.iter().map(|net| net.parse().unwrap()).collect(),
            ..Default::default()
        }
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn empty_filter_allows_everyone() {
        let filter = IpFilter::default();
        assert!(filter.is_empty());
        assert!(filter.allows(ip("203.0.113.9")));
        assert!(filter.allows(ip("2001:db8::1")));
    }

    #[test]
    fn allow_list_rejects_other_clients() {
        let filter = filter(&["192.168.0.0/16"], &[]);
        assert!(filter.allows(ip("192.168.1.10")));
        assert!(!filter.allows(ip("10.0.0.1")));
    }

    #[test]
    fn deny_takes_precedence_over_allow() {
        let filter = filter(&["192.168.0.0/16"], &["192.168.1.0/24"]);
        assert!(filter.allows(ip("192.168.2.1")));
        assert!(!filter.allows(ip("192.168.1.10")));
    }

    #[test]
    fn deny_only_allows_other_clients() {
        let filter = filter(&[], &["203.0.113.0/24", "2001:db8::/32"]);
        assert!(!filter.allows(ip("203.0.113.9")));
        assert!(!filter.allows(ip("2001:db8::1")));
        assert!(filter.allows(ip("198.51.100.1")));
    }

    #[test]
    fn ipv4_mapped_address_matches_ipv4_block() {
        let filter = filter(&[], &["203.0.113.0/24"]);
        assert!(!filter.allows(ip("::ffff:203.0.113.9")));
    }

    #[test]
    fn country_allow_and_deny_rules() {
        let filter = IpFilter {
            allow_countries: vec!["TR".into(), "DE".into()],
            deny_countries: vec!["RU".into()],
            ..Default::default()
        };
        assert!(!filter.is_empty());
        assert!(filter.allows_with_country(ip("1.2.3.4"), Some("TR")));
        assert!(filter.allows_with_country(ip("1.2.3.4"), Some("de"))); // case-insensitive
        assert!(!filter.allows_with_country(ip("1.2.3.4"), Some("US")));
        assert!(!filter.allows_with_country(ip("1.2.3.4"), Some("RU")));
        assert!(!filter.allows_with_country(ip("1.2.3.4"), None));

        // Deny country takes precedence
        let filter_conflict = IpFilter {
            allow_countries: vec!["TR".into()],
            deny_countries: vec!["TR".into()],
            ..Default::default()
        };
        assert!(!filter_conflict.allows_with_country(ip("1.2.3.4"), Some("TR")));
    }

    #[test]
    fn country_filter_allows_whitelisted_cidr_even_without_country() {
        let filter = IpFilter {
            allow: vec!["192.168.1.0/24".parse().unwrap()],
            allow_countries: vec!["TR".into()],
            ..Default::default()
        };
        // Whitelisted local subnet passes even with no country
        assert!(filter.allows_with_country(ip("192.168.1.50"), None));
        // Other IPs must match country
        assert!(filter.allows_with_country(ip("8.8.8.8"), Some("TR")));
        assert!(!filter.allows_with_country(ip("8.8.8.8"), Some("US")));
        assert!(!filter.allows_with_country(ip("10.0.0.1"), None));
    }

    #[test]
    fn rate_limit_is_disabled_by_default() {
        let limit = RateLimit::default();
        assert!(limit.is_disabled());
        assert_eq!(limit.replenish_interval(), None);
    }

    #[test]
    fn rate_limit_interval_and_burst() {
        let limit = RateLimit {
            requests: 30,
            per: RatePeriod::Minute,
            burst: 0,
        };
        assert_eq!(limit.replenish_interval(), Some(Duration::from_secs(2)));
        assert_eq!(limit.burst_size(), 30);
        assert_eq!(RateLimit { burst: 5, ..limit }.burst_size(), 5);
    }

    #[test]
    fn auth_policy_serde_uses_type_tag() {
        let policy: AuthPolicy = serde_json::from_str(
            r#"{"type":"basic","realm":"Staff","users":[{"username":"alice","password_hash":"$argon2id$x"}]}"#,
        )
        .unwrap();
        let AuthPolicy::Basic(basic) = &policy else {
            panic!("expected basic auth");
        };
        assert_eq!(basic.realm, "Staff");
        assert_eq!(basic.users[0].username, "alice");
        assert!(basic.users[0].password.is_empty());

        let none: AuthPolicy = serde_json::from_str(r#"{"type":"none"}"#).unwrap();
        assert!(none.is_none());
        let session: AuthPolicy = serde_json::from_str(r#"{"type":"session"}"#).unwrap();
        assert_eq!(session, AuthPolicy::Session);
        assert_eq!(
            serde_json::to_string(&session).unwrap(),
            r#"{"type":"session"}"#
        );
        assert_eq!(
            serde_json::to_string(&policy).unwrap(),
            r#"{"type":"basic","realm":"Staff","users":[{"username":"alice","password_hash":"$argon2id$x"}]}"#
        );
    }

    #[test]
    fn bearer_auth_serde_uses_type_tag() {
        let policy: AuthPolicy = serde_json::from_str(
            r#"{"type":"bearer","tokens":[{"name":"ci","token_hash":"abc"}]}"#,
        )
        .unwrap();
        let AuthPolicy::Bearer(bearer) = policy else {
            panic!("expected bearer auth");
        };
        assert_eq!(bearer.tokens[0].name, "ci");
        assert_eq!(bearer.tokens[0].token_hash, "abc");
        assert!(bearer.tokens[0].token.is_empty());
    }

    #[test]
    fn forward_auth_serde_uses_humantime_timeout() {
        let policy: AuthPolicy = serde_json::from_str(
            r#"{"type":"forward","url":"http://127.0.0.1:4180/auth","response_headers":["X-Auth-User"]}"#,
        )
        .unwrap();
        let AuthPolicy::Forward(forward) = policy else {
            panic!("expected forward auth");
        };
        assert_eq!(forward.url.to_string(), "http://127.0.0.1:4180/auth");
        assert_eq!(forward.response_headers, vec!["X-Auth-User"]);
        assert_eq!(forward.timeout, DEFAULT_FORWARD_AUTH_TIMEOUT);

        let json = serde_json::to_string(&AuthPolicy::Forward(Box::new(ForwardAuth {
            timeout: Duration::from_secs(3),
            ..*forward
        })))
        .unwrap();
        assert!(json.contains(r#""timeout":"3s""#));
    }

    #[test]
    fn header_name_rules() {
        assert!(is_header_name("X-Auth-User"));
        assert!(!is_header_name(""));
        assert!(!is_header_name("X Auth"));
        assert!(!is_header_name("X-Auth:"));
    }

    #[test]
    fn basic_auth_username_rules() {
        let user = |username: &str| BasicAuthUser {
            username: username.into(),
            ..Default::default()
        };
        assert_eq!(user("alice").username_error(), None);
        assert!(user("").username_error().is_some());
        assert!(user("al:ice").username_error().is_some());
        assert!(user("al\nice").username_error().is_some());
    }

    #[test]
    fn rate_limit_serde_uses_period_names() {
        let limit: RateLimit = serde_json::from_str(r#"{"requests":10,"per":"minute"}"#).unwrap();
        assert_eq!(limit.per, RatePeriod::Minute);
        assert_eq!(limit.burst, 0);
    }

    #[test]
    fn oidc_serde_and_allowed_emails() {
        let json = r#"{"type":"oidc","issuer_url":"https://accounts.google.com","client_id":"cid","allowed_emails":["*@company.com","admin@corp.com"]}"#;
        let policy: AuthPolicy = serde_json::from_str(json).unwrap();
        let AuthPolicy::Oidc(oidc) = policy else {
            panic!("expected oidc auth");
        };
        assert_eq!(oidc.issuer_url, "https://accounts.google.com");
        assert_eq!(oidc.client_id, "cid");
        assert_eq!(oidc.scopes, default_oidc_scopes());
        assert!(is_email_allowed("john@company.com", &oidc.allowed_emails));
        assert!(is_email_allowed("admin@corp.com", &oidc.allowed_emails));
        assert!(!is_email_allowed(
            "intruder@other.com",
            &oidc.allowed_emails
        ));
        assert!(is_email_allowed("anyone@anywhere.com", &[]));
    }

    #[test]
    fn concurrency_limit_disabled_and_validation() {
        let default_limit = ConcurrencyLimit::default();
        assert!(default_limit.is_disabled());
        assert!(default_limit.validate().is_ok());

        let active_limit = ConcurrencyLimit {
            max: 5,
            header: Some("X-API-Key".into()),
        };
        assert!(!active_limit.is_disabled());
        assert!(active_limit.validate().is_ok());

        let invalid_header = ConcurrencyLimit {
            max: 5,
            header: Some("invalid header".into()),
        };
        assert!(invalid_header.validate().is_err());
    }
}
