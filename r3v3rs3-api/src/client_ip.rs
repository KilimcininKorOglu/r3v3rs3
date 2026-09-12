use ipnet::IpNet;
use serde_derive::{Deserialize, Serialize};
use utoipa::ToSchema;

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
