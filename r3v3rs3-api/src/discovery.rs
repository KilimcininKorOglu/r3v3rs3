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
    Etcd,
    /// The apps of the deployment platform.
    Platform,
}

impl DiscoveryProvider {
    /// The providers that the discovery settings configure. The deployment platform is not one of
    /// them, because it publishes its proxies itself.
    pub const ALL: [DiscoveryProvider; 2] = [DiscoveryProvider::Docker, DiscoveryProvider::Etcd];

    /// The product name, which is the same in every language.
    pub fn name(self) -> &'static str {
        match self {
            Self::Docker => "Docker",
            Self::Etcd => "etcd",
            Self::Platform => "Platform",
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
    pub etcd: EtcdDiscoveryConfig,
}

impl DiscoveryConfig {
    /// A copy without secrets, which the admin API returns. `password_set` tells whether a
    /// secret is set.
    pub fn masked(&self) -> Self {
        let mut masked = self.clone();
        masked.etcd.password_set = masked.etcd.password.take().is_some();
        masked
    }

    /// Takes the secrets that an update does not set from the current settings. An empty secret
    /// removes the secret.
    pub fn keep_secrets(&mut self, current: &Self) {
        self.etcd.password_set = false;
        keep_secret(&mut self.etcd.password, &current.etcd.password);
    }
}

/// Takes the current secret when an update sets none. An empty secret removes the secret.
pub(crate) fn keep_secret(secret: &mut Option<String>, current: &Option<String>) {
    match secret.as_deref() {
        None => secret.clone_from(current),
        Some("") => *secret = None,
        Some(_) => {}
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
pub struct EtcdDiscoveryConfig {
    #[serde(default)]
    pub enabled: bool,
    /// The v3 HTTP API addresses of the cluster members. A connection error selects the next
    /// address.
    #[serde(default = "default_etcd_endpoints")]
    pub endpoints: Vec<String>,
    /// The client certificate for an `https` endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "abc-def")]
    pub client_cert: Option<crate::id::ShortId>,
    /// The user of etcd authentication. Empty sends no credentials.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[schema(example = "r3v3rs3")]
    pub username: String,
    /// The password of the user. The admin API does not return it. An update without a password
    /// keeps the current password, and an empty password removes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(write_only)]
    pub password: Option<String>,
    /// Whether a password is set.
    #[serde(default, skip_serializing_if = "is_false")]
    #[schema(read_only)]
    pub password_set: bool,
    #[serde(default = "default_etcd_prefix")]
    #[schema(example = "r3v3rs3")]
    pub prefix: String,
}

impl fmt::Debug for EtcdDiscoveryConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EtcdDiscoveryConfig")
            .field("enabled", &self.enabled)
            .field("endpoints", &self.endpoints)
            .field("client_cert", &self.client_cert)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "***"))
            .field("prefix", &self.prefix)
            .finish()
    }
}

fn default_etcd_endpoints() -> Vec<String> {
    vec!["http://127.0.0.1:2379".to_string()]
}

fn default_etcd_prefix() -> String {
    "r3v3rs3".to_string()
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
            "tcp" | "http" => parse_tcp_endpoint(rest, false).ok_or_else(invalid),
            "https" => parse_tcp_endpoint(rest, true).ok_or_else(invalid),
            _ => Err(invalid()),
        }
    }
}

/// Reads the host and the port of a TCP endpoint. A path, a query or a user name makes it invalid,
/// because an endpoint names a server only.
fn parse_tcp_endpoint(rest: &str, tls: bool) -> Option<Endpoint> {
    let scheme = if tls { "https" } else { "http" };
    let url = url::Url::parse(&format!("{scheme}://{rest}")).ok()?;
    if url.path() != "/" || url.query().is_some() || !url.username().is_empty() {
        return None;
    }
    Some(Endpoint::Tcp {
        tls,
        host: url.host_str()?.trim_matches(['[', ']']).to_string(),
        port: url.port_or_known_default()?,
    })
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

    #[test]
    fn the_etcd_password_is_masked_and_kept_until_an_update_sets_it() {
        let mut current = DiscoveryConfig::default();
        assert_eq!(current.etcd.endpoints, ["http://127.0.0.1:2379"]);
        assert_eq!(current.etcd.prefix, "r3v3rs3");
        current.etcd.username = "root".into();
        current.etcd.password = Some("secret".into());
        let masked = current.masked();
        assert_eq!(masked.etcd.password, None);
        assert!(masked.etcd.password_set);
        assert!(!serde_json::to_string(&masked).unwrap().contains("secret"));
        assert!(!format!("{:?}", current).contains("secret"));

        let mut update = masked.clone();
        update.keep_secrets(&current);
        assert_eq!(update, current);

        let mut update = masked;
        update.etcd.password = Some(String::new());
        update.keep_secrets(&current);
        assert_eq!(update.etcd.password, None);
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
