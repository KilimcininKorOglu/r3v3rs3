use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::{net::SocketAddr, path::PathBuf, time::Duration};
use utoipa::ToSchema;

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppConfig {
    #[serde(with = "humantime_serde", default = "default_background_task_interval")]
    #[schema(value_type = String, example = "1h")]
    pub background_task_interval: Duration,

    #[serde(default)]
    pub admin: AdminConfig,

    #[serde(default)]
    pub log: LogConfig,

    #[serde(default = "default_http_challenge_addr")]
    #[schema(value_type = String, example = "0.0.0.0:80")]
    pub http_challenge_addr: SocketAddr,

    /// The listening address of the ACME TLS-ALPN-01 challenges when no port uses its port.
    #[serde(default = "default_tls_alpn_challenge_addr")]
    #[schema(value_type = String, example = "0.0.0.0:443")]
    pub tls_alpn_challenge_addr: SocketAddr,

    /// DNS server that is asked whether the TXT records of a DNS-01 challenge are visible.
    /// The system resolver is used when it is not set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>, example = "1.1.1.1:53")]
    pub dns_challenge_resolver: Option<SocketAddr>,

    #[serde(default)]
    pub discovery: crate::discovery::DiscoveryConfig,

    /// Only `config.toml` sets this section. An update through the admin API keeps it.
    #[serde(default)]
    pub acme_exec: AcmeExecConfig,

    /// Only `config.toml` sets this section. An update through the admin API keeps it.
    #[serde(default)]
    pub cluster: crate::cluster::ClusterConfig,
}

/// The programs that the exec DNS provider can run.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AcmeExecConfig {
    /// Absolute paths of the programs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<String>, example = json!(["/usr/local/bin/r3v3rs3-dns-hook"]))]
    pub programs: Vec<PathBuf>,

    /// Longest run time of one program call.
    #[serde(with = "humantime_serde", default = "default_acme_exec_timeout")]
    #[schema(value_type = String, example = "30s")]
    pub timeout: Duration,
}

fn default_acme_exec_timeout() -> Duration {
    Duration::from_secs(30)
}

impl AppConfig {
    /// A copy without secrets, which the admin API returns.
    pub fn masked(&self) -> Self {
        Self {
            discovery: self.discovery.masked(),
            cluster: self.cluster.masked(),
            ..self.clone()
        }
    }

    /// Takes the secrets that an update does not set from the current settings.
    pub fn keep_secrets(&mut self, current: &Self) {
        self.discovery.keep_secrets(&current.discovery);
    }

    /// Takes the settings that only `config.toml` sets from the current settings.
    pub fn keep_file_only(&mut self, current: &Self) {
        self.acme_exec.clone_from(&current.acme_exec);
        self.cluster.clone_from(&current.cluster);
    }
}

fn default_background_task_interval() -> Duration {
    Duration::from_secs(60 * 60)
}

fn default_http_challenge_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 80))
}

fn default_tls_alpn_challenge_addr() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 443))
}

#[derive(Clone, Serialize, ToSchema)]
pub struct AppInfo {
    #[schema(example = "0.0.0")]
    pub version: &'static str,
    #[schema(example = "aarch64-apple-darwin")]
    pub target: &'static str,
    #[schema(example = "debug")]
    pub profile: &'static str,
    #[schema(example = json!([]))]
    pub features: &'static [&'static str],
    #[schema(example = "rustc 1.69.0 (84c898d65 2023-04-16)")]
    pub rustc: &'static str,
    #[schema(value_type = String, example = "/home/r3v3rs3/.config/r3v3rs3")]
    pub config_path: PathBuf,
    #[schema(value_type = String, example = "/home/r3v3rs3/.config/r3v3rs3")]
    pub log_path: PathBuf,
}

#[derive(Debug, DefaultFromSerde, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AdminConfig {
    #[serde(with = "humantime_serde", default = "default_admin_session_expiry")]
    #[schema(value_type = String, example = "1d")]
    pub session_expiry: Duration,

    #[serde(default = "default_max_attempts")]
    pub max_login_attempts: u32,

    #[serde(with = "humantime_serde", default = "default_login_attempts_reset")]
    #[schema(value_type = String, example = "15m")]
    pub login_attempts_reset: Duration,
}

fn default_admin_session_expiry() -> Duration {
    Duration::from_secs(60 * 60)
}

fn default_max_attempts() -> u32 {
    10
}

fn default_login_attempts_reset() -> Duration {
    Duration::from_secs(60 * 15)
}

#[derive(Debug, DefaultFromSerde, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LogConfig {
    #[serde(with = "humantime_serde", default = "default_database_log_retention")]
    #[schema(value_type = String, example = "3months")]
    pub database_log_retention: Duration,
}

fn default_database_log_retention() -> Duration {
    Duration::from_secs(60 * 60 * 24 * 30 * 3)
}
