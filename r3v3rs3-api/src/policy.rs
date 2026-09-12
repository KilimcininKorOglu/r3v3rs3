use ipnet::IpNet;
use serde_derive::{Deserialize, Serialize};
use std::net::IpAddr;
use utoipa::ToSchema;

/// Allows or denies requests by the resolved client IP address.
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
}

impl IpFilter {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    pub fn allows(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        if self.deny.iter().any(|net| net.contains(&ip)) {
            return false;
        }
        self.allow.is_empty() || self.allow.iter().any(|net| net.contains(&ip))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(allow: &[&str], deny: &[&str]) -> IpFilter {
        IpFilter {
            allow: allow.iter().map(|net| net.parse().unwrap()).collect(),
            deny: deny.iter().map(|net| net.parse().unwrap()).collect(),
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
}
