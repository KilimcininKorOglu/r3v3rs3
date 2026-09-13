use crate::error::Error;
use serde_derive::{Deserialize, Serialize};
use std::time::Duration;
use utoipa::ToSchema;

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_MAX_FAILS: u32 = 1;
pub const DEFAULT_FAIL_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Selects the upstream server of each new request, connection or UDP session.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadBalancing {
    /// Uses the servers in turn.
    #[default]
    RoundRobin,
    /// Uses a random server.
    Random,
    /// Uses the first healthy server in the list. The other servers are backups.
    First,
}

impl LoadBalancing {
    pub const ALL: [Self; 3] = [Self::RoundRobin, Self::Random, Self::First];

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RoundRobin => "round_robin",
            Self::Random => "random",
            Self::First => "first",
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Health check of the upstream servers of a proxy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HealthCheck {
    /// Consecutive failures that mark a server unhealthy. A failure is a failed connection or a
    /// request without a response. `0` disables the passive health check.
    #[serde(default = "default_max_fails")]
    pub max_fails: u32,

    /// Time that an unhealthy server stays behind the healthy servers.
    #[serde(default = "default_fail_timeout", with = "humantime_serde")]
    #[schema(value_type = String, example = "30s")]
    pub fail_timeout: Duration,
}

impl Default for HealthCheck {
    fn default() -> Self {
        Self {
            max_fails: DEFAULT_MAX_FAILS,
            fail_timeout: DEFAULT_FAIL_TIMEOUT,
        }
    }
}

impl HealthCheck {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn validate(&self) -> Result<(), Error> {
        validate_timeout(self.fail_timeout)
    }
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

pub fn is_default_connect_timeout(timeout: &Duration) -> bool {
    *timeout == DEFAULT_CONNECT_TIMEOUT
}

pub fn is_default_session_idle_timeout(timeout: &Duration) -> bool {
    *timeout == DEFAULT_SESSION_IDLE_TIMEOUT
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
    use crate::proxy::{HttpProxy, ProxyKind, Route, TcpProxy, UdpProxy};

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
            ..serde_json::from_str(r#"{"servers":[]}"#).unwrap()
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
