use crate::error::Error;
use serde_derive::{Deserialize, Serialize};
use std::time::Duration;
use utoipa::ToSchema;

pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub const DEFAULT_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

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

pub fn default_connect_timeout() -> Duration {
    DEFAULT_CONNECT_TIMEOUT
}

fn default_request_timeout() -> Duration {
    DEFAULT_REQUEST_TIMEOUT
}

pub fn default_session_idle_timeout() -> Duration {
    DEFAULT_SESSION_IDLE_TIMEOUT
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
    fn default_timeouts_are_not_serialized() {
        let tcp: TcpProxy = serde_json::from_str("{}").unwrap();
        assert_eq!(tcp.connect_timeout, DEFAULT_CONNECT_TIMEOUT);
        assert_eq!(serde_json::to_string(&tcp).unwrap(), "{}");

        let udp: UdpProxy = serde_json::from_str("{}").unwrap();
        assert_eq!(udp.session_idle_timeout, DEFAULT_SESSION_IDLE_TIMEOUT);
        assert_eq!(serde_json::to_string(&udp).unwrap(), "{}");

        let http: HttpProxy = serde_json::from_str(r#"{"routes":[]}"#).unwrap();
        assert!(http.timeouts.is_default());
        assert!(!serde_json::to_string(&http).unwrap().contains("timeouts"));
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
    fn zero_connect_and_idle_timeouts_are_rejected() {
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
        assert!(ProxyKind::Http(Box::new(http)).validate_timeouts().is_err());

        let tcp = TcpProxy {
            connect_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(ProxyKind::Tcp(tcp).validate_timeouts().is_err());

        let udp = UdpProxy {
            session_idle_timeout: Duration::ZERO,
            ..Default::default()
        };
        assert!(ProxyKind::Udp(udp).validate_timeouts().is_err());

        let request_disabled = UpstreamTimeouts {
            request: Duration::ZERO,
            ..Default::default()
        };
        assert!(request_disabled.validate().is_ok());
        assert!(ProxyKind::Tcp(TcpProxy::default())
            .validate_timeouts()
            .is_ok());
    }
}
