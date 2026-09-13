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
    #[serde(default)]
    pub consul: ConsulDiscoveryConfig,
}

impl DiscoveryConfig {
    /// A copy without secrets, which the admin API returns. `token_set` tells whether a token is
    /// set.
    pub fn masked(&self) -> Self {
        let mut masked = self.clone();
        masked.consul.token_set = masked.consul.token.take().is_some();
        masked
    }

    /// Takes the secrets that an update does not set from the current settings. An empty secret
    /// removes the secret.
    pub fn keep_secrets(&mut self, current: &Self) {
        let consul = &mut self.consul;
        consul.token_set = false;
        match consul.token.as_deref() {
            None => consul.token.clone_from(&current.consul.token),
            Some("") => consul.token = None,
            Some(_) => {}
        }
    }
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

#[derive(
    serde_default::DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema,
)]
pub struct ConsulDiscoveryConfig {
    #[serde(default)]
    pub enabled: bool,
    /// The HTTP API of a Consul agent: `http://<host>:<port>`, `https://<host>:<port>` or
    /// `unix://<path>`.
    #[serde(default = "default_consul_address")]
    #[schema(example = "http://127.0.0.1:8500")]
    pub address: String,
    /// The client certificate for an `https` address.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "abc-def")]
    pub client_cert: Option<crate::id::ShortId>,
    /// The ACL token. The admin API does not return it. An update without a token keeps the
    /// current token, and an empty token removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(write_only)]
    pub token: Option<String>,
    /// Whether a token is set.
    #[serde(default, skip_serializing_if = "is_false")]
    #[schema(read_only)]
    pub token_set: bool,
    /// The datacenter to read. Empty reads the datacenter of the agent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schema(example = "dc1")]
    pub datacenter: String,
    /// Reads the `r3v3rs3.*` tags of the services in the catalog.
    #[serde(default = "default_true")]
    pub catalog: bool,
    /// Reads the keys under `prefix` in the key-value store.
    #[serde(default = "default_true")]
    pub kv: bool,
    #[serde(default = "default_consul_prefix")]
    #[schema(example = "r3v3rs3")]
    pub prefix: String,
    /// Reads every service. Otherwise only the services with the `r3v3rs3.enable=true` tag are
    /// read.
    #[serde(default)]
    pub exposed_by_default: bool,
}

impl fmt::Debug for ConsulDiscoveryConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsulDiscoveryConfig")
            .field("enabled", &self.enabled)
            .field("address", &self.address)
            .field("client_cert", &self.client_cert)
            .field("token", &self.token.as_ref().map(|_| "***"))
            .field("datacenter", &self.datacenter)
            .field("catalog", &self.catalog)
            .field("kv", &self.kv)
            .field("prefix", &self.prefix)
            .field("exposed_by_default", &self.exposed_by_default)
            .finish()
    }
}

fn default_consul_address() -> String {
    "http://127.0.0.1:8500".to_string()
}

fn default_consul_prefix() -> String {
    "r3v3rs3".to_string()
}

fn default_true() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !value
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
        assert!(!config.consul.enabled);
        assert!(config.consul.catalog && config.consul.kv);
        assert_eq!(config.consul.prefix, "r3v3rs3");
    }

    fn with_token(token: Option<&str>) -> DiscoveryConfig {
        let mut config = DiscoveryConfig::default();
        config.consul.token = token.map(str::to_string);
        config
    }

    #[test]
    fn the_token_is_masked_and_kept_until_an_update_sets_it() {
        let current = with_token(Some("secret"));
        let masked = current.masked();
        assert_eq!(masked.consul.token, None);
        assert!(masked.consul.token_set);
        assert!(!serde_json::to_string(&masked).unwrap().contains("secret"));
        assert!(!format!("{:?}", current).contains("secret"));
        assert!(!with_token(None).masked().consul.token_set);

        let mut update = masked;
        update.keep_secrets(&current);
        assert_eq!(update, current);

        let mut update = with_token(Some("new"));
        update.keep_secrets(&current);
        assert_eq!(update.consul.token.as_deref(), Some("new"));

        let mut update = with_token(Some(""));
        update.keep_secrets(&current);
        assert_eq!(update.consul.token, None);
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
