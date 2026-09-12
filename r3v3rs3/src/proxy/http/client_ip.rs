//! Resolves the real client IP from trusted proxy headers.

use crate::cdn::CdnTable;
use hyper::HeaderMap;
use ipnet::IpNet;
use r3v3rs3_api::{cdn::CdnProvider, client_ip::ClientIpConfig};
use std::net::{IpAddr, SocketAddr};

/// Client IP headers that only a trusted proxy may set.
pub const CLIENT_IP_HEADERS: &[&str] = &[
    "forwarded",
    "x-forwarded-for",
    "x-real-ip",
    "cf-connecting-ip",
    "true-client-ip",
    "cloudfront-viewer-address",
    "fastly-client-ip",
    "incap-client-ip",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientAddr {
    /// The address of the TCP or QUIC peer.
    pub peer: IpAddr,
    /// The resolved client address.
    pub ip: IpAddr,
    /// True when the peer is a trusted proxy.
    pub trusted_peer: bool,
}

impl ClientAddr {
    pub fn direct(peer: IpAddr) -> Self {
        let peer = peer.to_canonical();
        Self {
            peer,
            ip: peer,
            trusted_peer: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClientIpResolver {
    trust_cdn: bool,
    trusted_proxies: Vec<IpNet>,
}

enum Trust {
    Cdn(CdnProvider),
    Proxy,
}

impl ClientIpResolver {
    pub fn new(config: &ClientIpConfig) -> Self {
        Self {
            trust_cdn: config.trust_cdn,
            trusted_proxies: config.trusted_proxies.clone(),
        }
    }

    pub fn resolve(&self, peer: IpAddr, headers: &HeaderMap, table: &CdnTable) -> ClientAddr {
        let direct = ClientAddr::direct(peer);
        let Some(trust) = self.trust(direct.peer, table) else {
            return direct;
        };
        let provider_ip = match trust {
            Trust::Cdn(provider) => provider_header_ip(provider, headers),
            Trust::Proxy => None,
        };
        let ip = provider_ip
            .or_else(|| self.forwarded_for_ip(headers, table))
            .unwrap_or(direct.peer);
        ClientAddr {
            peer: direct.peer,
            ip,
            trusted_peer: true,
        }
    }

    fn trust(&self, ip: IpAddr, table: &CdnTable) -> Option<Trust> {
        if self.trusted_proxies.iter().any(|net| net.contains(&ip)) {
            return Some(Trust::Proxy);
        }
        if !self.trust_cdn {
            return None;
        }
        table.lookup(ip).map(Trust::Cdn)
    }

    /// Returns the rightmost untrusted address of X-Forwarded-For.
    fn forwarded_for_ip(&self, headers: &HeaderMap, table: &CdnTable) -> Option<IpAddr> {
        let entries: Vec<IpAddr> = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .filter_map(parse_forwarded_ip)
            .collect();
        entries
            .iter()
            .rev()
            .find(|ip| self.trust(**ip, table).is_none())
            .or(entries.first())
            .copied()
    }
}

fn provider_header_ip(provider: CdnProvider, headers: &HeaderMap) -> Option<IpAddr> {
    match provider {
        CdnProvider::Cloudflare => header_ip(headers, "cf-connecting-ip"),
        CdnProvider::Bunny => header_ip(headers, "x-real-ip"),
        CdnProvider::Cloudfront => header_ip(headers, "cloudfront-viewer-address"),
        _ => None,
    }
}

fn header_ip(headers: &HeaderMap, name: &str) -> Option<IpAddr> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_forwarded_ip)
}

/// Parses an address with an optional port, such as `198.51.100.10:46532` or `[2001:db8::1]:443`.
fn parse_forwarded_ip(value: &str) -> Option<IpAddr> {
    let value = value.trim();
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip.to_canonical());
    }
    if let Ok(addr) = value.parse::<SocketAddr>() {
        return Some(addr.ip().to_canonical());
    }
    // CloudFront-Viewer-Address does not bracket IPv6 addresses.
    let (ip, port) = value.rsplit_once(':')?;
    port.parse::<u16>().ok()?;
    ip.parse::<IpAddr>().ok().map(|ip| ip.to_canonical())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cdn::CdnRanges;
    use r3v3rs3_api::cdn::CdnRangesSource;
    use std::collections::BTreeMap;

    const CLOUDFLARE_EDGE: &str = "173.245.48.10";
    const CLOUDFRONT_EDGE: &str = "54.192.0.10";
    const FASTLY_EDGE: &str = "151.101.0.10";

    fn table() -> CdnTable {
        let providers = BTreeMap::from([
            (
                CdnProvider::Cloudflare,
                vec!["173.245.48.0/20".parse().unwrap()],
            ),
            (
                CdnProvider::Cloudfront,
                vec!["54.192.0.0/16".parse().unwrap()],
            ),
            (CdnProvider::Fastly, vec!["151.101.0.0/16".parse().unwrap()]),
        ]);
        CdnTable::new(
            CdnRanges {
                updated_at: 1,
                providers,
            },
            CdnRangesSource::Downloaded,
        )
    }

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, value.parse().unwrap());
        }
        map
    }

    fn resolver(trust_cdn: bool, proxies: &[&str]) -> ClientIpResolver {
        ClientIpResolver::new(&ClientIpConfig {
            trust_cdn,
            trusted_proxies: proxies.iter().map(|p| p.parse().unwrap()).collect(),
        })
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn untrusted_peer_ignores_headers() {
        let headers = headers(&[
            ("cf-connecting-ip", "203.0.113.9"),
            ("x-forwarded-for", "203.0.113.9"),
        ]);
        let client = resolver(true, &[]).resolve(ip("198.51.100.1"), &headers, &table());
        assert_eq!(client.ip, ip("198.51.100.1"));
        assert!(!client.trusted_peer);
    }

    #[test]
    fn cloudflare_edge_uses_cf_connecting_ip() {
        let headers = headers(&[
            ("cf-connecting-ip", "203.0.113.9"),
            ("x-forwarded-for", "10.0.0.1, 203.0.113.9"),
        ]);
        let client = resolver(true, &[]).resolve(ip(CLOUDFLARE_EDGE), &headers, &table());
        assert_eq!(client.ip, ip("203.0.113.9"));
        assert!(client.trusted_peer);
    }

    #[test]
    fn cdn_trust_can_be_disabled() {
        let headers = headers(&[("cf-connecting-ip", "203.0.113.9")]);
        let client = resolver(false, &[]).resolve(ip(CLOUDFLARE_EDGE), &headers, &table());
        assert_eq!(client.ip, ip(CLOUDFLARE_EDGE));
        assert!(!client.trusted_peer);
    }

    #[test]
    fn cloudfront_viewer_address_supports_ipv4_and_ipv6() {
        let v4 = headers(&[("cloudfront-viewer-address", "198.51.100.10:46532")]);
        let client = resolver(true, &[]).resolve(ip(CLOUDFRONT_EDGE), &v4, &table());
        assert_eq!(client.ip, ip("198.51.100.10"));

        let v6 = headers(&[("cloudfront-viewer-address", "2001:db8::1:46532")]);
        let client = resolver(true, &[]).resolve(ip(CLOUDFRONT_EDGE), &v6, &table());
        assert_eq!(client.ip, ip("2001:db8::1"));
    }

    #[test]
    fn forwarded_for_uses_rightmost_untrusted_address() {
        // The client spoofed 1.2.3.4. The trusted load balancer appended the Fastly edge.
        let headers = headers(&[("x-forwarded-for", "1.2.3.4, 203.0.113.9, 151.101.0.10")]);
        let client = resolver(true, &["10.0.0.0/8"]).resolve(ip("10.0.0.5"), &headers, &table());
        assert_eq!(client.ip, ip("203.0.113.9"));
        assert!(client.trusted_peer);
    }

    #[test]
    fn fastly_edge_falls_back_to_forwarded_for() {
        let headers = headers(&[
            ("fastly-client-ip", "1.2.3.4"),
            ("x-forwarded-for", "203.0.113.9"),
        ]);
        let client = resolver(true, &[]).resolve(ip(FASTLY_EDGE), &headers, &table());
        assert_eq!(client.ip, ip("203.0.113.9"));
    }

    #[test]
    fn trusted_peer_without_headers_uses_peer() {
        let client =
            resolver(true, &["10.0.0.0/8"]).resolve(ip("10.0.0.5"), &HeaderMap::new(), &table());
        assert_eq!(client.ip, ip("10.0.0.5"));
        assert!(client.trusted_peer);
    }

    #[test]
    fn ipv4_mapped_peer_is_canonical() {
        let headers = headers(&[("cf-connecting-ip", "203.0.113.9")]);
        let client = resolver(true, &[]).resolve(ip("::ffff:173.245.48.10"), &headers, &table());
        assert_eq!(client.peer, ip(CLOUDFLARE_EDGE));
        assert_eq!(client.ip, ip("203.0.113.9"));
    }
}
