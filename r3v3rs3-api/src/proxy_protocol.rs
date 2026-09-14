//! Reads the client address from the PROXY protocol header that a load balancer sends.

use crate::error::Error;
use crate::multiaddr::Multiaddr;
use ipnet::IpNet;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::time::Duration;
use utoipa::ToSchema;

/// The default time that a trusted peer has to send the header.
pub const DEFAULT_PROXY_PROTOCOL_TIMEOUT: Duration = Duration::from_secs(5);

/// The PROXY protocol versions that a port accepts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProxyProtocolAccept {
    V1,
    V2,
    #[default]
    Any,
}

/// The PROXY protocol version that a TCP proxy sends to its upstream servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProxyProtocolVersion {
    V1,
    V2,
}

/// Reads the PROXY protocol header on a TCP, TLS, HTTP or HTTPS port.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProxyProtocolReceive {
    #[serde(default, skip_serializing_if = "is_any")]
    pub accept: ProxyProtocolAccept,
    /// Only a peer in these blocks must send the header. r3v3rs3 does not read a header from
    /// another peer.
    #[serde(default)]
    #[schema(value_type = [String], example = json!(["10.0.0.0/8"]))]
    pub trusted: Vec<IpNet>,
    /// A trusted peer that does not send the complete header in this time is disconnected.
    #[serde(
        default = "default_timeout",
        with = "humantime_serde",
        skip_serializing_if = "is_default_timeout"
    )]
    #[schema(value_type = String, example = "5s")]
    pub timeout: Duration,
}

impl ProxyProtocolReceive {
    /// Rejects a UDP or QUIC port, an empty trusted list and a zero timeout.
    pub fn validate(&self, listen: &Multiaddr) -> Result<(), Error> {
        if listen.is_udp() || listen.is_quic() {
            Err(Error::ProxyProtocolNotSupported)
        } else if self.trusted.is_empty() {
            Err(Error::ProxyProtocolTrustedMissing)
        } else if self.timeout.is_zero() {
            Err(Error::InvalidTimeout)
        } else {
            Ok(())
        }
    }

    /// Whether the peer must send the header.
    pub fn trusts(&self, peer: std::net::IpAddr) -> bool {
        let peer = peer.to_canonical();
        self.trusted.iter().any(|net| net.contains(&peer))
    }
}

fn is_any(accept: &ProxyProtocolAccept) -> bool {
    *accept == ProxyProtocolAccept::Any
}

fn default_timeout() -> Duration {
    DEFAULT_PROXY_PROTOCOL_TIMEOUT
}

fn is_default_timeout(timeout: &Duration) -> bool {
    *timeout == DEFAULT_PROXY_PROTOCOL_TIMEOUT
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listen(addr: &str) -> Multiaddr {
        addr.parse().unwrap()
    }

    #[test]
    fn a_config_round_trips_and_keeps_its_defaults_out() {
        let value = json!({ "accept": "v2", "trusted": ["10.0.0.0/8"], "timeout": "2s" });
        let config: ProxyProtocolReceive = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(config.accept, ProxyProtocolAccept::V2);
        assert_eq!(config.timeout, Duration::from_secs(2));
        assert_eq!(serde_json::to_value(&config).unwrap(), value);

        let defaults: ProxyProtocolReceive =
            serde_json::from_value(json!({ "trusted": ["::1/128"] })).unwrap();
        assert_eq!(defaults.accept, ProxyProtocolAccept::Any);
        assert_eq!(defaults.timeout, DEFAULT_PROXY_PROTOCOL_TIMEOUT);
        assert_eq!(
            serde_json::to_value(&defaults).unwrap(),
            json!({ "trusted": ["::1/128"] })
        );
    }

    #[test]
    fn validate_rejects_udp_ports_an_empty_trusted_list_and_a_zero_timeout() {
        let config = ProxyProtocolReceive {
            trusted: vec!["127.0.0.0/8".parse().unwrap()],
            ..Default::default()
        };
        for addr in [
            "/ip4/0.0.0.0/tcp/80",
            "/ip4/0.0.0.0/tcp/443/tls",
            "/ip4/0.0.0.0/tcp/80/http",
            "/ip4/0.0.0.0/tcp/443/https",
        ] {
            assert!(config.validate(&listen(addr)).is_ok(), "{addr}");
        }
        for addr in ["/ip4/0.0.0.0/udp/53", "/ip4/0.0.0.0/udp/443/quic/http"] {
            assert!(
                matches!(
                    config.validate(&listen(addr)),
                    Err(Error::ProxyProtocolNotSupported)
                ),
                "{addr}"
            );
        }

        let tcp = listen("/ip4/0.0.0.0/tcp/80");
        let empty = ProxyProtocolReceive::default();
        assert!(matches!(
            empty.validate(&tcp),
            Err(Error::ProxyProtocolTrustedMissing)
        ));
        let zero = ProxyProtocolReceive {
            timeout: Duration::ZERO,
            ..config
        };
        assert!(matches!(zero.validate(&tcp), Err(Error::InvalidTimeout)));
    }

    #[test]
    fn trusts_matches_mapped_ipv4_peers() {
        let config = ProxyProtocolReceive {
            trusted: vec!["127.0.0.0/8".parse().unwrap()],
            ..Default::default()
        };
        assert!(config.trusts("127.0.0.1".parse().unwrap()));
        assert!(config.trusts("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!config.trusts("10.0.0.1".parse().unwrap()));
    }
}
