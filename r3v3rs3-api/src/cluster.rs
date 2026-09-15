use serde_derive::{Deserialize, Serialize};
use std::{fmt, path::PathBuf, time::Duration};
use utoipa::ToSchema;

/// The store that the nodes of a cluster share.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterBackend {
    #[default]
    Etcd,
    Consul,
}

/// The settings of a cluster whose nodes share their state in etcd or Consul. Only `config.toml`
/// sets them. An update through the admin API keeps them.
#[derive(
    serde_default::DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema,
)]
pub struct ClusterConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub backend: ClusterBackend,

    /// The HTTP API addresses of the store: `http://<host>:<port>`, `https://<host>:<port>` or
    /// `unix://<path>`. A connection error selects the next address.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(example = json!(["https://10.0.0.1:2379"]))]
    pub endpoints: Vec<String>,

    /// The etcd user. Empty sends no credentials.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,

    /// The password of the etcd user. The admin API does not return it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(write_only)]
    pub password: Option<String>,

    /// Whether a password is set.
    #[serde(default, skip_serializing_if = "is_false")]
    #[schema(read_only)]
    pub password_set: bool,

    /// The Consul ACL token. The admin API does not return it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(write_only)]
    pub token: Option<String>,

    /// Whether a token is set.
    #[serde(default, skip_serializing_if = "is_false")]
    #[schema(read_only)]
    pub token_set: bool,

    /// The Consul datacenter. Empty uses the datacenter of the agent.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub datacenter: String,

    /// Every key of the cluster starts with this prefix.
    #[serde(default = "default_prefix")]
    #[schema(example = "r3v3rs3")]
    pub prefix: String,

    /// The name of this node. Empty uses the host name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_name: String,

    #[serde(default)]
    pub tls: ClusterTlsConfig,

    /// The files of the value encryption keys. The first key encrypts. Every key decrypts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<String>, example = json!(["/etc/r3v3rs3/cluster.key"]))]
    pub encryption_key_files: Vec<PathBuf>,

    /// The TTL of the leader lock and of the node presence. Consul needs at least 10 seconds.
    #[serde(with = "humantime_serde", default = "default_lock_ttl")]
    #[schema(value_type = String, example = "15s")]
    pub lock_ttl: Duration,

    /// The longest wait for the store when the node starts.
    #[serde(with = "humantime_serde", default = "default_startup_timeout")]
    #[schema(value_type = String, example = "30s")]
    pub startup_timeout: Duration,

    /// How often a node publishes its rate limit counts.
    #[serde(with = "humantime_serde", default = "default_rate_limit_sync_interval")]
    #[schema(value_type = String, example = "1s")]
    pub rate_limit_sync_interval: Duration,

    /// Stores cached responses in the store for the other nodes.
    #[serde(default)]
    pub share_cache: bool,

    /// The largest cached response in bytes that a node stores for the other nodes.
    #[serde(default = "default_cache_max_value_size")]
    pub cache_max_value_size: u64,
}

impl ClusterConfig {
    /// A copy without secrets, which the admin API returns.
    pub fn masked(&self) -> Self {
        let mut masked = self.clone();
        masked.password_set = masked.password.take().is_some();
        masked.token_set = masked.token.take().is_some();
        masked
    }
}

impl fmt::Debug for ClusterConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let secret = |value: &Option<String>| value.as_ref().map(|_| "***");
        f.debug_struct("ClusterConfig")
            .field("enabled", &self.enabled)
            .field("backend", &self.backend)
            .field("endpoints", &self.endpoints)
            .field("username", &self.username)
            .field("password", &secret(&self.password))
            .field("token", &secret(&self.token))
            .field("datacenter", &self.datacenter)
            .field("prefix", &self.prefix)
            .field("node_name", &self.node_name)
            .field("tls", &self.tls)
            .field("encryption_key_files", &self.encryption_key_files)
            .field("lock_ttl", &self.lock_ttl)
            .field("startup_timeout", &self.startup_timeout)
            .field("rate_limit_sync_interval", &self.rate_limit_sync_interval)
            .field("share_cache", &self.share_cache)
            .field("cache_max_value_size", &self.cache_max_value_size)
            .finish()
    }
}

/// How a node follows the cluster store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClusterState {
    /// The node does not use a cluster store.
    #[default]
    Disabled,
    /// The node reads the store for the first time.
    Syncing,
    /// The node applies the changes of the store.
    Synced,
    /// The node lost the store for longer than `lock_ttl`. It serves the last state and rejects
    /// changes.
    Degraded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClusterStatus {
    pub state: ClusterState,

    /// The name of this node.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub node_name: String,

    /// Whether this node holds the leader lock. Only the leader orders ACME certificates, removes
    /// expired certificates and refreshes the CDN IP ranges.
    #[serde(default)]
    pub leader: bool,

    /// The store revision of the last applied change.
    #[serde(default)]
    pub revision: u64,

    /// The last error of the store connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The TLS files of the store connection. Without `ca_file`, the system root certificates verify
/// the store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ClusterTlsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "/etc/r3v3rs3/etcd-ca.pem")]
    pub ca_file: Option<PathBuf>,

    /// The client certificate. It needs `key_file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "/etc/r3v3rs3/etcd-client.pem")]
    pub cert_file: Option<PathBuf>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "/etc/r3v3rs3/etcd-client-key.pem")]
    pub key_file: Option<PathBuf>,
}

fn default_prefix() -> String {
    "r3v3rs3".to_string()
}

fn default_lock_ttl() -> Duration {
    Duration::from_secs(15)
}

fn default_startup_timeout() -> Duration {
    Duration::from_secs(30)
}

fn default_rate_limit_sync_interval() -> Duration {
    Duration::from_secs(1)
}

fn default_cache_max_value_size() -> u64 {
    1024 * 1024
}

fn is_false(value: &bool) -> bool {
    !value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_section_parses_and_the_secrets_are_masked() {
        let config: ClusterConfig = serde_json::from_value(serde_json::json!({
            "enabled": true,
            "backend": "consul",
            "endpoints": ["https://10.0.0.1:8501"],
            "token": "consul-secret",
            "encryption_key_files": ["/etc/r3v3rs3/cluster.key"],
            "lock_ttl": "20s",
            "tls": {"ca_file": "/etc/r3v3rs3/ca.pem"},
        }))
        .unwrap();
        assert_eq!(config.backend, ClusterBackend::Consul);
        assert_eq!(config.lock_ttl, Duration::from_secs(20));
        assert_eq!(config.prefix, "r3v3rs3");
        assert_eq!(config.cache_max_value_size, 1024 * 1024);
        assert!(!format!("{config:?}").contains("consul-secret"));

        let masked = config.masked();
        assert_eq!((masked.token.as_deref(), masked.token_set), (None, true));
        assert_eq!(
            (masked.password.as_deref(), masked.password_set),
            (None, false)
        );
        assert!(!serde_json::to_string(&masked)
            .unwrap()
            .contains("consul-secret"));
    }
}
