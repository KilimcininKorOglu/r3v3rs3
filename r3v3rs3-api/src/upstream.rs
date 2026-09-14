use crate::error::Error;
use serde_derive::{Deserialize, Serialize};
use std::time::Duration;
use utoipa::ToSchema;

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_MAX_FAILS: u32 = 1;
pub const DEFAULT_FAIL_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
pub const DEFAULT_WEIGHT: u16 = 1;
pub const DEFAULT_FAILURE_RATIO: u8 = 50;
pub const DEFAULT_MIN_REQUESTS: u32 = 20;
pub const DEFAULT_BREAKER_WINDOW: Duration = Duration::from_secs(10);
pub const DEFAULT_OPEN_DURATION: Duration = Duration::from_secs(30);
pub const DEFAULT_RETRY_ATTEMPTS: u8 = 2;
pub const MAX_RETRY_ATTEMPTS: u8 = 10;
pub const DEFAULT_STICKY_COOKIE_NAME: &str = "r3v3rs3_affinity";

/// Timeouts of the requests that an HTTP proxy sends to its upstream servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpstreamTimeouts {
    /// Time limit for the DNS lookup, the TCP connection and the TLS handshake of a new upstream
    /// connection.
    #[serde(default = "default_connect_timeout", with = "humantime_serde")]
    #[schema(value_type = String, example = "10s")]
    pub connect: Duration,

    /// Time limit from the start of the request until the response headers arrive, including a
    /// new connection. `0s` disables the limit. The response body and upgraded connections have
    /// no limit.
    #[serde(default = "default_request_timeout", with = "humantime_serde")]
    #[schema(value_type = String, example = "60s")]
    pub request: Duration,
}

impl Default for UpstreamTimeouts {
    fn default() -> Self {
        Self {
            connect: DEFAULT_CONNECT_TIMEOUT,
            request: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

impl UpstreamTimeouts {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<(), Error> {
        validate_timeout(self.connect)
    }
}

/// Sends a failed HTTP request again to the next upstream server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RetryPolicy {
    /// Tries of one request, including the first try, from 1 to 10. `1` disables retries.
    #[serde(default = "default_retry_attempts")]
    pub attempts: u8,

    /// Failures that send the request again. `timeout`, `http_502`, `http_503` and `http_504`
    /// apply only to the idempotent methods `GET`, `HEAD`, `OPTIONS`, `TRACE`, `PUT` and `DELETE`.
    #[serde(default = "default_retry_on")]
    pub retry_on: Vec<RetryOn>,

    /// Largest request body in bytes that r3v3rs3 keeps in memory to send again. The length of
    /// the body must be known. `0` retries only requests without a body.
    #[serde(default)]
    pub replay_body_limit: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            attempts: DEFAULT_RETRY_ATTEMPTS,
            retry_on: default_retry_on(),
            replay_body_limit: 0,
        }
    }
}

impl RetryPolicy {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Rejects attempts outside 1 to [`MAX_RETRY_ATTEMPTS`].
    pub fn validate(&self) -> Result<(), Error> {
        if (1..=MAX_RETRY_ATTEMPTS).contains(&self.attempts) {
            Ok(())
        } else {
            Err(Error::InvalidRetryAttempts)
        }
    }
}

/// A failure of one try that a [`RetryPolicy`] can retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub enum RetryOn {
    /// The connection fails or the connect timeout expires. The server received nothing.
    #[serde(rename = "connect")]
    Connect,
    /// The request timeout expires.
    #[serde(rename = "timeout")]
    Timeout,
    #[serde(rename = "http_502")]
    Http502,
    #[serde(rename = "http_503")]
    Http503,
    #[serde(rename = "http_504")]
    Http504,
}

impl RetryOn {
    pub const ALL: [Self; 5] = [
        Self::Connect,
        Self::Timeout,
        Self::Http502,
        Self::Http503,
        Self::Http504,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Timeout => "timeout",
            Self::Http502 => "http_502",
            Self::Http503 => "http_503",
            Self::Http504 => "http_504",
        }
    }
}

/// Selects the upstream server of each new request, connection or UDP session. A server with weight
/// 0 gets no new traffic.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalancing {
    /// Uses the servers in turn, each server as often as its weight.
    #[default]
    RoundRobin,
    /// Uses a random server. The chance of a server is proportional to its weight.
    Random,
    /// Uses the first healthy server in the list. The other servers are backups.
    First,
    /// Uses the same server for each client IP address while the servers stay healthy. The share
    /// of the client addresses of a server is proportional to its weight.
    ClientIpHash,
}

impl LoadBalancing {
    pub const ALL: [Self; 4] = [
        Self::RoundRobin,
        Self::Random,
        Self::First,
        Self::ClientIpHash,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::Random => "random",
            Self::First => "first",
            Self::ClientIpHash => "client_ip_hash",
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Health check of the upstream servers of a proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HealthCheck {
    /// Consecutive failures that mark a server unhealthy. A failure is a failed connection or a
    /// request without a response. `0` disables the passive health check.
    #[serde(default = "default_max_fails")]
    pub max_fails: u32,

    /// Time that an unhealthy server stays behind the healthy servers.
    #[serde(default = "default_fail_timeout", with = "humantime_serde")]
    #[schema(value_type = String, example = "30s")]
    pub fail_timeout: Duration,

    /// Time between two active checks of the servers. `0s` disables the active health check.
    #[serde(
        default,
        with = "humantime_serde",
        skip_serializing_if = "Duration::is_zero"
    )]
    #[schema(value_type = String, example = "10s")]
    pub interval: Duration,

    /// Time limit of one active check.
    #[serde(
        default = "default_health_check_timeout",
        with = "humantime_serde",
        skip_serializing_if = "is_default_health_check_timeout"
    )]
    #[schema(value_type = String, example = "5s")]
    pub timeout: Duration,

    /// Path that the active check of an HTTP proxy requests with `GET`, from the root of the
    /// server. A 2xx or 3xx response is healthy. Empty: the check opens a TCP connection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schema(example = "/health")]
    pub path: String,
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            max_fails: DEFAULT_MAX_FAILS,
            fail_timeout: DEFAULT_FAIL_TIMEOUT,
            interval: Duration::ZERO,
            timeout: DEFAULT_HEALTH_CHECK_TIMEOUT,
            path: String::new(),
        }
    }
}

impl HealthCheck {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn is_active(&self) -> bool {
        !self.interval.is_zero()
    }

    /// Rejects a zero fail timeout or check timeout, and a path that does not start with `/`.
    /// Only an HTTP proxy can have a path.
    pub fn validate(&self, http: bool) -> Result<(), Error> {
        validate_timeout(self.fail_timeout)?;
        validate_timeout(self.timeout)?;
        if self.path.is_empty() || (http && self.path.starts_with('/')) {
            return Ok(());
        }
        Err(Error::InvalidHealthCheckPath {
            path: self.path.clone(),
        })
    }
}

/// Stops the traffic to a failing server of an HTTP or TCP proxy for a time. Each server of each
/// route has its own circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CircuitBreaker {
    #[serde(default)]
    pub enabled: bool,

    /// Percentage of failed requests in a window that opens the circuit, from 1 to 100. An HTTP
    /// request fails when the connection fails, a timeout expires or the server answers 502, 503
    /// or 504. A TCP connection fails when it cannot connect.
    #[serde(default = "default_failure_ratio")]
    pub failure_ratio: u8,

    /// Requests that a window must have before the failure ratio can open the circuit.
    #[serde(default = "default_min_requests")]
    pub min_requests: u32,

    /// Time that one window counts the requests of a server.
    #[serde(default = "default_breaker_window", with = "humantime_serde")]
    #[schema(value_type = String, example = "10s")]
    pub window: Duration,

    /// Time that an open circuit sends no traffic to the server. Then one trial request tests the
    /// server.
    #[serde(default = "default_open_duration", with = "humantime_serde")]
    #[schema(value_type = String, example = "30s")]
    pub open_duration: Duration,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self {
            enabled: false,
            failure_ratio: DEFAULT_FAILURE_RATIO,
            min_requests: DEFAULT_MIN_REQUESTS,
            window: DEFAULT_BREAKER_WINDOW,
            open_duration: DEFAULT_OPEN_DURATION,
        }
    }
}

impl CircuitBreaker {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Rejects a failure ratio outside 1 to 100, zero minimum requests, and a zero window or open
    /// duration.
    pub fn validate(&self) -> Result<(), Error> {
        let valid = (1..=100).contains(&self.failure_ratio)
            && self.min_requests > 0
            && !self.window.is_zero()
            && !self.open_duration.is_zero();
        if valid {
            Ok(())
        } else {
            Err(Error::InvalidCircuitBreaker)
        }
    }
}

/// Keeps each client of an HTTP proxy on one upstream server with a signed cookie.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct StickyCookie {
    #[serde(default)]
    pub enabled: bool,

    /// Name of the cookie, a token of RFC 6265.
    #[serde(default = "default_sticky_cookie_name")]
    pub name: String,

    /// `Max-Age` of the cookie. Without a value, the cookie ends with the browser session.
    #[serde(
        default,
        with = "humantime_serde",
        skip_serializing_if = "Option::is_none"
    )]
    #[schema(value_type = Option<String>, example = "1h")]
    pub max_age: Option<Duration>,
}

impl Default for StickyCookie {
    fn default() -> Self {
        Self {
            enabled: false,
            name: default_sticky_cookie_name(),
            max_age: None,
        }
    }
}

impl StickyCookie {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Rejects a name that is not a cookie token and a zero `Max-Age`.
    pub fn validate(&self) -> Result<(), Error> {
        if !is_cookie_token(&self.name) {
            return Err(Error::InvalidStickyCookieName {
                name: self.name.clone(),
            });
        }
        self.max_age.map_or(Ok(()), validate_timeout)
    }
}

/// A token of RFC 6265: visible ASCII characters without separators.
fn is_cookie_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_graphic() && !b"()<>@,;:\\\"/[]?={}".contains(&byte))
}

/// The circuit of an upstream server.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CircuitState {
    /// The server gets traffic.
    #[default]
    Closed,
    /// The server gets no traffic.
    Open,
    /// One trial request tests the server.
    HalfOpen,
}

impl CircuitState {
    pub fn is_closed(&self) -> bool {
        *self == Self::Closed
    }
}

/// The health of one upstream server of a proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UpstreamHealth {
    /// The URL or the address of the server.
    pub addr: String,
    pub weight: u16,
    pub healthy: bool,
    /// Consecutive failed connections or requests.
    pub failures: u32,
    /// The error of the last failed connection, request or active check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// The circuit of the server. A closed circuit is not serialized.
    #[serde(default, skip_serializing_if = "CircuitState::is_closed")]
    pub circuit: CircuitState,
}

pub fn default_connect_timeout() -> Duration {
    DEFAULT_CONNECT_TIMEOUT
}

fn default_request_timeout() -> Duration {
    DEFAULT_REQUEST_TIMEOUT
}

pub fn default_session_idle_timeout() -> Duration {
    DEFAULT_SESSION_IDLE_TIMEOUT
}

fn default_max_fails() -> u32 {
    DEFAULT_MAX_FAILS
}

fn default_fail_timeout() -> Duration {
    DEFAULT_FAIL_TIMEOUT
}

fn default_health_check_timeout() -> Duration {
    DEFAULT_HEALTH_CHECK_TIMEOUT
}

fn is_default_health_check_timeout(timeout: &Duration) -> bool {
    *timeout == DEFAULT_HEALTH_CHECK_TIMEOUT
}

pub fn is_default_connect_timeout(timeout: &Duration) -> bool {
    *timeout == DEFAULT_CONNECT_TIMEOUT
}

pub fn is_default_session_idle_timeout(timeout: &Duration) -> bool {
    *timeout == DEFAULT_SESSION_IDLE_TIMEOUT
}

fn default_retry_attempts() -> u8 {
    DEFAULT_RETRY_ATTEMPTS
}

fn default_retry_on() -> Vec<RetryOn> {
    vec![RetryOn::Connect]
}

fn default_sticky_cookie_name() -> String {
    DEFAULT_STICKY_COOKIE_NAME.to_string()
}

fn default_failure_ratio() -> u8 {
    DEFAULT_FAILURE_RATIO
}

fn default_min_requests() -> u32 {
    DEFAULT_MIN_REQUESTS
}

fn default_breaker_window() -> Duration {
    DEFAULT_BREAKER_WINDOW
}

fn default_open_duration() -> Duration {
    DEFAULT_OPEN_DURATION
}

pub fn default_weight() -> u16 {
    DEFAULT_WEIGHT
}

pub fn is_default_weight(weight: &u16) -> bool {
    *weight == DEFAULT_WEIGHT
}

/// Rejects a server list in which every server has weight 0, because no server of it gets traffic.
/// An empty list is valid.
pub fn validate_weights(weights: impl IntoIterator<Item = u16>) -> Result<(), Error> {
    let mut weights = weights.into_iter().peekable();
    if weights.peek().is_some() && weights.all(|weight| weight == 0) {
        return Err(Error::AllServersDrained);
    }
    Ok(())
}

/// Rejects a zero timeout, because a zero time limit fails every connection.
pub fn validate_timeout(timeout: Duration) -> Result<(), Error> {
    if timeout.is_zero() {
        return Err(Error::InvalidTimeout);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::UpstreamServer;
    use crate::proxy::{HttpProxy, ProxyKind, Route, Server, TcpProxy, UdpProxy};

    #[test]
    fn default_upstream_settings_are_not_serialized() {
        let tcp: TcpProxy = serde_json::from_str("{}").unwrap();
        assert_eq!(tcp.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(tcp.load_balancing, LoadBalancing::RoundRobin);
        assert_eq!(tcp.health_check, HealthCheck::default());
        assert_eq!(serde_json::to_string(&tcp).unwrap(), "{}");

        let udp: UdpProxy = serde_json::from_str("{}").unwrap();
        assert_eq!(udp.session_idle_timeout, DEFAULT_SESSION_IDLE_TIMEOUT);
        assert_eq!(serde_json::to_string(&udp).unwrap(), "{}");

        let http: HttpProxy = serde_json::from_str(r#"{"routes":[]}"#).unwrap();
        assert!(http.timeouts.is_default());
        assert_eq!(serde_json::to_string(&http).unwrap(), r#"{"routes":[]}"#);
    }

    #[test]
    fn timeouts_use_the_humantime_format() {
        let json = r#"{"connect":"500ms","request":"0s"}"#;
        let timeouts: UpstreamTimeouts = serde_json::from_str(json).unwrap();
        assert_eq!(timeouts.connect, Duration::from_millis(500));
        assert!(timeouts.request.is_zero());
        assert_eq!(serde_json::to_string(&timeouts).unwrap(), json);

        let partial: UpstreamTimeouts = serde_json::from_str(r#"{"request":"5s"}"#).unwrap();
        assert_eq!(partial.connect, DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(partial.request, Duration::from_secs(5));
    }

    #[test]
    fn load_balancing_and_health_check_use_snake_case_and_humantime() {
        let json =
            r#"{"load_balancing":"first","health_check":{"max_fails":3,"fail_timeout":"1m"}}"#;
        let tcp: TcpProxy = serde_json::from_str(json).unwrap();
        assert_eq!(tcp.load_balancing, LoadBalancing::First);
        assert_eq!(tcp.health_check.max_fails, 3);
        assert_eq!(tcp.health_check.fail_timeout, Duration::from_secs(60));
        assert_eq!(serde_json::to_string(&tcp).unwrap(), json);

        for policy in LoadBalancing::ALL {
            let json = serde_json::to_string(&policy).unwrap();
            assert_eq!(json, format!("\"{}\"", policy.as_str()));
        }

        let json = r#"{"max_fails":0,"fail_timeout":"30s","interval":"10s","timeout":"2s","path":"/health"}"#;
        let health_check: HealthCheck = serde_json::from_str(json).unwrap();
        assert!(health_check.is_active());
        assert_eq!(health_check.timeout, Duration::from_secs(2));
        assert_eq!(serde_json::to_string(&health_check).unwrap(), json);
        assert!(!HealthCheck::default().is_active());
    }

    #[test]
    fn only_an_http_proxy_has_a_health_check_path() {
        let with_path = HealthCheck {
            path: "/health".into(),
            ..Default::default()
        };
        assert!(with_path.validate(true).is_ok());
        assert!(with_path.validate(false).is_err());

        let relative = HealthCheck {
            path: "health".into(),
            ..Default::default()
        };
        assert!(relative.validate(true).is_err());

        let no_timeout = HealthCheck {
            timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(no_timeout.validate(true).is_err());
    }

    #[test]
    fn weights_default_to_one_and_every_group_needs_a_server_above_zero() {
        let tcp: TcpProxy =
            serde_json::from_str(r#"{"upstream_servers":[{"addr":"/ip4/10.0.0.1/tcp/80"}]}"#)
                .unwrap();
        assert_eq!(tcp.upstream_servers[0].weight, DEFAULT_WEIGHT);
        assert!(!serde_json::to_string(&tcp).unwrap().contains("weight"));

        let route: Route =
            serde_json::from_str(r#"{"servers":[{"url":"http://a/","weight":3}]}"#).unwrap();
        assert_eq!(route.servers[0].weight, 3);
        assert!(serde_json::to_string(&route)
            .unwrap()
            .contains(r#""weight":3"#));

        assert!(validate_weights([]).is_ok());
        assert!(validate_weights([0, 1]).is_ok());
        assert!(validate_weights([0, 0]).is_err());

        let drained = |weight| TcpProxy {
            upstream_servers: vec![UpstreamServer {
                weight,
                ..tcp.upstream_servers[0].clone()
            }],
            ..tcp.clone()
        };
        assert!(ProxyKind::Tcp(drained(0)).validate_upstream().is_err());
        assert!(ProxyKind::Tcp(drained(2)).validate_upstream().is_ok());
        let http = HttpProxy {
            routes: vec![Route {
                servers: vec![Server {
                    weight: 0,
                    ..route.servers[0].clone()
                }],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(ProxyKind::Http(Box::new(http)).validate_upstream().is_err());
    }

    #[test]
    fn circuit_breaker_defaults_are_not_serialized_and_invalid_values_are_rejected() {
        let json = r#"{"circuit_breaker":{"enabled":true,"open_duration":"1m"}}"#;
        let tcp: TcpProxy = serde_json::from_str(json).unwrap();
        assert!(tcp.circuit_breaker.enabled);
        assert_eq!(tcp.circuit_breaker.failure_ratio, DEFAULT_FAILURE_RATIO);
        assert_eq!(tcp.circuit_breaker.min_requests, DEFAULT_MIN_REQUESTS);
        assert_eq!(tcp.circuit_breaker.open_duration, Duration::from_secs(60));
        assert!(ProxyKind::Tcp(tcp).validate_upstream().is_ok());
        assert_eq!(serde_json::to_string(&TcpProxy::default()).unwrap(), "{}");

        let invalid = [
            CircuitBreaker {
                failure_ratio: 0,
                ..Default::default()
            },
            CircuitBreaker {
                failure_ratio: 101,
                ..Default::default()
            },
            CircuitBreaker {
                min_requests: 0,
                ..Default::default()
            },
            CircuitBreaker {
                window: Duration::ZERO,
                ..Default::default()
            },
            CircuitBreaker {
                open_duration: Duration::ZERO,
                ..Default::default()
            },
        ];
        for circuit_breaker in invalid {
            let http = HttpProxy {
                circuit_breaker,
                ..Default::default()
            };
            assert!(ProxyKind::Http(Box::new(http)).validate_upstream().is_err());
        }

        let half_open = serde_json::to_string(&CircuitState::HalfOpen).unwrap();
        assert_eq!(half_open, r#""half_open""#);
    }

    #[test]
    fn retry_policies_use_snake_case_names_and_need_one_to_ten_attempts() {
        use crate::proxy::{Route, Server};

        let json = r#"{"routes":[],"retry":{"attempts":3,"retry_on":["timeout","http_502"],"replay_body_limit":1024}}"#;
        let http: HttpProxy = serde_json::from_str(json).unwrap();
        let expected = RetryPolicy {
            attempts: 3,
            retry_on: vec![RetryOn::Timeout, RetryOn::Http502],
            replay_body_limit: 1024,
        };
        assert_eq!(http.retry, expected);
        assert_eq!(RetryPolicy::default().retry_on, [RetryOn::Connect]);
        let default = serde_json::to_value(HttpProxy::default()).unwrap();
        assert!(default.get("retry").is_none());
        for reason in RetryOn::ALL {
            assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
        }

        for attempts in [0, MAX_RETRY_ATTEMPTS + 1] {
            let retry = RetryPolicy {
                attempts,
                ..Default::default()
            };
            let proxy = HttpProxy {
                retry: retry.clone(),
                ..Default::default()
            };
            assert!(ProxyKind::Http(Box::new(proxy))
                .validate_upstream()
                .is_err());

            let route = Route {
                servers: vec![Server::new("http://127.0.0.1:9000/".parse().unwrap())],
                retry: Some(retry),
                ..Default::default()
            };
            let proxy = HttpProxy {
                routes: vec![route],
                ..Default::default()
            };
            assert!(ProxyKind::Http(Box::new(proxy))
                .validate_upstream()
                .is_err());
        }
    }

    #[test]
    fn sticky_cookies_need_a_token_name_and_a_positive_max_age() {
        let json = r#"{"routes":[],"load_balancing":"client_ip_hash","sticky":{"enabled":true,"name":"srv","max_age":"1h"}}"#;
        let http: HttpProxy = serde_json::from_str(json).unwrap();
        assert_eq!(http.load_balancing, LoadBalancing::ClientIpHash);
        assert_eq!(
            http.sticky,
            StickyCookie {
                enabled: true,
                name: "srv".into(),
                max_age: Some(Duration::from_secs(3600)),
            }
        );
        assert_eq!(serde_json::to_string(&http).unwrap(), json);

        let enabled: StickyCookie = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!(enabled.name, DEFAULT_STICKY_COOKIE_NAME);
        assert_eq!(enabled.max_age, None);
        assert!(enabled.validate().is_ok());
        let default = serde_json::to_value(HttpProxy::default()).unwrap();
        assert!(default.get("sticky").is_none());

        for name in ["", "a b", "a;b", "a=b", "çerez"] {
            let sticky = StickyCookie {
                name: name.into(),
                ..Default::default()
            };
            let proxy = HttpProxy {
                sticky,
                ..Default::default()
            };
            assert!(
                ProxyKind::Http(Box::new(proxy))
                    .validate_upstream()
                    .is_err(),
                "{name}"
            );
        }
        let zero = StickyCookie {
            max_age: Some(Duration::ZERO),
            ..Default::default()
        };
        assert!(zero.validate().is_err());
    }

    #[test]
    fn zero_timeouts_are_rejected() {
        let zero = UpstreamTimeouts {
            connect: Duration::ZERO,
            request: Duration::ZERO,
        };
        assert!(zero.validate().is_err());

        let route = Route {
            timeouts: Some(zero),
            ..Default::default()
        };
        let http = HttpProxy {
            routes: vec![route],
            ..serde_json::from_str(r#"{"routes":[]}"#).unwrap()
        };
        assert!(ProxyKind::Http(Box::new(http)).validate_upstream().is_err());

        let tcp = TcpProxy {
            connect_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(ProxyKind::Tcp(tcp).validate_upstream().is_err());

        let udp = UdpProxy {
            session_idle_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(ProxyKind::Udp(udp).validate_upstream().is_err());

        let no_fail_timeout = HealthCheck {
            fail_timeout: Duration::ZERO,
            ..Default::default()
        };
        let udp = UdpProxy {
            health_check: no_fail_timeout,
            ..Default::default()
        };
        assert!(ProxyKind::Udp(udp).validate_upstream().is_err());

        let request_disabled = UpstreamTimeouts {
            request: Duration::ZERO,
            ..Default::default()
        };
        assert!(request_disabled.validate().is_ok());
        assert!(ProxyKind::Tcp(TcpProxy::default())
            .validate_upstream()
            .is_ok());
    }
}
