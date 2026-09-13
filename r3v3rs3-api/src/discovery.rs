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

/// The connection state of a discovery provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryState {
    /// The provider has not read its resources yet.
    Connecting,
    /// The provider has read its resources and watches them for changes.
    Running,
    /// The provider cannot read its resources. The proxies of its last read stay active.
    Error,
}

/// A resource definition that did not become a proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DiscoveryIssue {
    #[schema(example = "web-1")]
    pub resource: String,
    #[schema(example = "http.app: port not found: https")]
    pub message: String,
}

/// The state of a discovery provider and the result of its last read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DiscoveryStatus {
    pub provider: DiscoveryProvider,
    pub state: DiscoveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The number of proxies that the provider added.
    pub proxies: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub issues: Vec<DiscoveryIssue>,
    /// Unix time of the last update.
    pub updated_at: i64,
}

/// The settings of the discovery providers.
#[derive(
    Debug, serde_default::DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema,
)]
pub struct DiscoveryConfig {
    #[serde(default)]
    pub docker: DockerDiscoveryConfig,
}

#[derive(
    Debug, serde_default::DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema,
)]
pub struct DockerDiscoveryConfig {
    #[serde(default)]
    pub enabled: bool,
    /// `unix://<path>`, `tcp://<host>:<port>`, `http://<host>:<port>` or `https://<host>:<port>`.
    #[serde(default = "default_docker_endpoint")]
    #[schema(example = "unix:///var/run/docker.sock")]
    pub endpoint: String,
    /// The client certificate for an `https` endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "abc-def")]
    pub client_cert: Option<crate::id::ShortId>,
    /// The Docker network of the upstream addresses. Empty uses the only network of a container.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schema(example = "proxy")]
    pub network: String,
    /// Reads every container. Otherwise only the containers with `r3v3rs3.enable=true` are read.
    #[serde(default)]
    pub exposed_by_default: bool,
}

fn default_docker_endpoint() -> String {
    "unix:///var/run/docker.sock".to_string()
}

/// The address of a provider API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// The path of a Unix socket.
    Unix(String),
    Tcp {
        tls: bool,
        host: String,
        port: u16,
    },
}

impl std::str::FromStr for Endpoint {
    type Err = crate::error::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let invalid = || crate::error::Error::InvalidDiscoveryConfig {
            reason: format!("invalid endpoint: {s}"),
        };
        let (scheme, rest) = s.split_once("://").ok_or_else(invalid)?;
        match scheme {
            "unix" if rest.len() > 1 && rest.starts_with('/') => Ok(Self::Unix(rest.to_string())),
            "tcp" | "http" | "https" => {
                let tls = scheme == "https";
                let url =
                    url::Url::parse(&format!("{}://{rest}", if tls { "https" } else { "http" }))
                        .map_err(|_| invalid())?;
                let plain = url.path() == "/" && url.query().is_none() && url.username().is_empty();
                match (plain, url.host_str(), url.port_or_known_default()) {
                    (true, Some(host), Some(port)) => Ok(Self::Tcp {
                        tls,
                        host: host.trim_matches(['[', ']']).to_string(),
                        port,
                    }),
                    _ => Err(invalid()),
                }
            }
            _ => Err(invalid()),
        }
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;

    #[test]
    fn endpoints_parse_their_address() {
        assert_eq!(
            "unix:///var/run/docker.sock".parse::<Endpoint>().unwrap(),
            Endpoint::Unix("/var/run/docker.sock".into())
        );
        assert_eq!(
            "tcp://10.0.0.1:2375".parse::<Endpoint>().unwrap(),
            Endpoint::Tcp {
                tls: false,
                host: "10.0.0.1".into(),
                port: 2375
            }
        );
        assert_eq!(
            "https://[fd00::1]".parse::<Endpoint>().unwrap(),
            Endpoint::Tcp {
                tls: true,
                host: "fd00::1".into(),
                port: 443
            }
        );
        for invalid in [
            "",
            "/var/run/docker.sock",
            "unix://",
            "unix://docker.sock",
            "ftp://host:21",
            "http://host:80/v1",
            "http://user@host:80",
        ] {
            assert!(invalid.parse::<Endpoint>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn the_default_docker_config_uses_the_local_socket() {
        let config = DiscoveryConfig::default();
        assert!(!config.docker.enabled);
        assert_eq!(config.docker.endpoint, "unix:///var/run/docker.sock");
    }
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
