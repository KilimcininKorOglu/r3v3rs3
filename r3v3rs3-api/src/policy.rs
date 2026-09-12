use ipnet::IpNet;
use serde_derive::{Deserialize, Serialize};
use std::{net::IpAddr, time::Duration};
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
    fn rate_limit_serde_uses_period_names() {
        let limit: RateLimit = serde_json::from_str(r#"{"requests":10,"per":"minute"}"#).unwrap();
        assert_eq!(limit.per, RatePeriod::Minute);
        assert_eq!(limit.burst, 0);
    }
}
