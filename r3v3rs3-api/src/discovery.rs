use serde_derive::{Deserialize, Serialize};
use std::fmt;
use utoipa::ToSchema;

/// A service discovery provider.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryProvider {
    Docker,
    Kubernetes,
    Consul,
    Etcd,
}

impl DiscoveryProvider {
    pub const ALL: [DiscoveryProvider; 4] = [
        DiscoveryProvider::Docker,
        DiscoveryProvider::Kubernetes,
        DiscoveryProvider::Consul,
        DiscoveryProvider::Etcd,
    ];

    /// The product name, which is the same in every language.
    pub fn name(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Kubernetes => "Kubernetes",
            Self::Consul => "Consul",
            Self::Etcd => "etcd",
        }
    }
}

impl fmt::Display for DiscoveryProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The origin of a discovered proxy.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
pub struct DiscoverySource {
    pub provider: DiscoveryProvider,
    /// The resource that defines the proxy, e.g. a container name, `ingress default/app`, a Consul
    /// service or a key prefix.
    #[schema(example = "web-1")]
    pub resource: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::{Proxy, ProxyEntry};

    #[test]
    fn a_manual_proxy_is_serialized_without_a_source() {
        let entry = ProxyEntry::from(("abc".parse().unwrap(), Proxy::default()));
        let json = serde_json::to_value(&entry).unwrap();
        assert!(json.get("source").is_none());
        assert!(!entry.is_discovered());

        let discovered = ProxyEntry {
            source: Some(DiscoverySource {
                provider: DiscoveryProvider::Docker,
                resource: "web-1".into(),
            }),
            ..entry
        };
        let json = serde_json::to_value(&discovered).unwrap();
        assert_eq!(
            json["source"],
            serde_json::json!({"provider": "docker", "resource": "web-1"})
        );
        let parsed: ProxyEntry = serde_json::from_value(json).unwrap();
        assert_eq!(parsed, discovered);
    }
}
