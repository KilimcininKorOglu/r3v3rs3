use ipnet::IpNet;
use serde_derive::{Deserialize, Serialize};
use std::net::IpAddr;
use utoipa::ToSchema;

/// Canonicalizes an IP address for rate limiting and lockouts: IPv4 addresses are kept as-is,
/// and IPv6 addresses are aggregated to their /64 prefix.
pub fn canonical_client_ip(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            let mut prefix = [0u8; 16];
            prefix[..8].copy_from_slice(&octets[..8]);
            IpAddr::V6(std::net::Ipv6Addr::from(prefix))
        }
    }
}

/// Controls how the real client IP is resolved for an HTTP proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClientIpConfig {
    /// Trust client IP headers from the edge servers of known CDNs.
    #[serde(default = "default_trust_cdn", skip_serializing_if = "is_true")]
    pub trust_cdn: bool,

    /// Additional trusted proxies, such as a local load balancer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = [String], example = json!(["10.0.0.0/8"]))]
    pub trusted_proxies: Vec<IpNet>,
}

impl Default for ClientIpConfig {
    fn default() -> Self {
        Self {
            trust_cdn: default_trust_cdn(),
            trusted_proxies: Vec::new(),
        }
    }
}

impl ClientIpConfig {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

fn default_trust_cdn() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_client_ip_aggregates_ipv6_to_prefix64() {
        let ip1: IpAddr = "2001:db8::1".parse().unwrap();
        let ip2: IpAddr = "2001:db8::abcd:1234".parse().unwrap();
        let expected: IpAddr = "2001:db8::".parse().unwrap();

        assert_eq!(canonical_client_ip(ip1), expected);
        assert_eq!(canonical_client_ip(ip2), expected);
        assert_eq!(canonical_client_ip(ip1), canonical_client_ip(ip2));

        let v4: IpAddr = "192.0.2.1".parse().unwrap();
        assert_eq!(canonical_client_ip(v4), v4);

        let mapped: IpAddr = "::ffff:192.0.2.1".parse().unwrap();
        assert_eq!(canonical_client_ip(mapped), v4);
    }
}
