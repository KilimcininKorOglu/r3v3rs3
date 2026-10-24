//! The container types of the deployment platform. The local Docker runtime and the agent protocol
//! share them, so every name is validated when it is parsed and a value that passes the parser is
//! safe to put into a Docker API path or query.

use crate::error::Error;
use crate::id::ShortId;
use serde_derive::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;
use utoipa::ToSchema;

/// The label that names the app of a container, network or volume that the platform creates.
pub const APP_LABEL: &str = "r3v3rs3.app";

/// The label that Docker Compose puts on the containers and the built images of a project.
pub const COMPOSE_PROJECT_LABEL: &str = "com.docker.compose.project";

/// The label that Docker Compose puts on containers naming their service.
pub const COMPOSE_SERVICE_LABEL: &str = "com.docker.compose.service";

/// The longest name of a container, a network, a volume or an app.
const MAX_NAME_LENGTH: usize = 63;

/// The longest image reference, as the Docker distribution specification allows.
const MAX_IMAGE_LENGTH: usize = 255;

/// The longest path inside a container.
const MAX_PATH_LENGTH: usize = 4096;

fn invalid(reason: String) -> Error {
    Error::InvalidContainerSpec { reason }
}

/// Accepts `[a-z0-9][a-z0-9_.-]{0,62}`.
fn check_name(kind: &str, value: &str) -> Result<(), Error> {
    let valid_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || "_.-".contains(c);
    let first_valid = value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if value.len() <= MAX_NAME_LENGTH && first_valid && value.chars().all(valid_char) {
        Ok(())
    } else {
        Err(invalid(format!("invalid {kind}: {value}")))
    }
}

macro_rules! name_type {
    ($(#[$doc:meta])* $name:ident, $kind:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                check_name($kind, s)?;
                Ok(Self(s.to_string()))
            }
        }

        impl TryFrom<String> for $name {
            type Error = Error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                check_name($kind, &value)?;
                Ok(Self(value))
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

name_type!(
    /// The name of an app. It is also the value of the `r3v3rs3.app` label.
    AppName,
    "app name"
);
name_type!(ContainerName, "container name");
name_type!(NetworkName, "network name");
name_type!(VolumeName, "volume name");
name_type!(
    /// The name of a Compose project, which prefixes the names of its containers.
    ProjectName,
    "project name"
);
name_type!(
    /// The name of a service in a Compose file.
    ServiceName,
    "service name"
);

/// An image reference such as `nginx:1.27`, `ghcr.io/owner/app@sha256:…` or
/// `registry.example:5000/app`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ImageRef(String);

impl ImageRef {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The repository part: the reference without its tag or digest.
    pub fn repository(&self) -> &str {
        self.split().0
    }

    /// The tag or the digest. A reference without either uses `latest`.
    pub fn tag_or_digest(&self) -> &str {
        self.split().1
    }

    fn split(&self) -> (&str, &str) {
        if let Some((repository, digest)) = self.0.split_once('@') {
            return (repository, digest);
        }
        let name_start = self.0.rfind('/').map_or(0, |index| index + 1);
        match self.0[name_start..].rfind(':') {
            Some(index) => (
                &self.0[..name_start + index],
                &self.0[name_start + index + 1..],
            ),
            None => (&self.0, "latest"),
        }
    }
}

fn check_image(value: &str) -> Result<(), Error> {
    let valid_char = |c: char| c.is_ascii_alphanumeric() || "._-/:@".contains(c);
    let valid = !value.is_empty()
        && value.len() <= MAX_IMAGE_LENGTH
        && value.chars().all(valid_char)
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value.matches('@').count() <= 1
        && !value.contains("//")
        && !value.ends_with([':', '/', '@']);
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("invalid image reference: {value}")))
    }
}

impl FromStr for ImageRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        check_image(s)?;
        Ok(Self(s.to_string()))
    }
}

impl TryFrom<String> for ImageRef {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        check_image(&value)?;
        Ok(Self(value))
    }
}

impl From<ImageRef> for String {
    fn from(value: ImageRef) -> Self {
        value.0
    }
}

impl fmt::Display for ImageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The name of an environment variable: `[A-Za-z_][A-Za-z0-9_]*`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EnvKey(String);

impl EnvKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn check_env_key(value: &str) -> Result<(), Error> {
    let valid = value.len() <= MAX_PATH_LENGTH
        && value.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(invalid(format!(
            "invalid environment variable name: {value}"
        )))
    }
}

impl FromStr for EnvKey {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        check_env_key(s)?;
        Ok(Self(s.to_string()))
    }
}

impl TryFrom<String> for EnvKey {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        check_env_key(&value)?;
        Ok(Self(value))
    }
}

impl From<EnvKey> for String {
    fn from(value: EnvKey) -> Self {
        value.0
    }
}

/// An absolute path inside a container, without `..` segments.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ContainerPath(String);

impl ContainerPath {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn check_container_path(value: &str) -> Result<(), Error> {
    let valid = value.starts_with('/')
        && value.len() <= MAX_PATH_LENGTH
        && !value.contains('\0')
        && !value.split('/').any(|segment| segment == "..");
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("invalid container path: {value}")))
    }
}

impl FromStr for ContainerPath {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        check_container_path(s)?;
        Ok(Self(s.to_string()))
    }
}

impl TryFrom<String> for ContainerPath {
    type Error = Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        check_container_path(&value)?;
        Ok(Self(value))
    }
}

impl From<ContainerPath> for String {
    fn from(value: ContainerPath) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: EnvKey,
    pub value: String,
}

/// A named volume mounted into a container. Host paths cannot be mounted, so a deployment cannot
/// read or change the files of the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct VolumeMount {
    #[schema(value_type = String, example = "shop-data")]
    pub volume: VolumeName,
    #[schema(value_type = String, example = "/data")]
    pub target: ContainerPath,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RestartPolicy {
    No,
    OnFailure,
    #[default]
    UnlessStopped,
    Always,
}

impl RestartPolicy {
    /// The name that the Docker Engine API uses.
    pub fn docker_name(self) -> &'static str {
        match self {
            Self::No => "no",
            Self::OnFailure => "on-failure",
            Self::UnlessStopped => "unless-stopped",
            Self::Always => "always",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResourceLimits {
    /// The memory limit in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// The CPU limit in billionths of a CPU. `1_500_000_000` is one and a half CPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nano_cpus: Option<u64>,
    /// Total memory + swap limit in bytes (`-1` for unlimited swap).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap_bytes: Option<i64>,
}

/// A container to create. The image runs its own entrypoint and command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerSpec {
    pub name: ContainerName,
    pub image: ImageRef,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkName>,
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
    #[serde(default)]
    pub restart: RestartPolicy,
    #[serde(default)]
    pub limits: ResourceLimits,
    /// A TCP port of the container that Docker publishes on a free port of `127.0.0.1`, so that
    /// r3v3rs3 on the host reaches the container and other hosts do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<u16>,
    /// Publishes the port of `publish` on this fixed port of `127.0.0.1` instead of a free
    /// port, so the address survives a new deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_port: Option<u16>,
    /// Extra entries of `/etc/hosts` of the container, as `host` and `ip` pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_hosts: Vec<(String, String)>,
    /// Docker log-opt max-size (default: "50m").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_max_size: Option<String>,
    /// Docker log-opt max-file (default: "3").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_max_file: Option<String>,
}

impl ContainerSpec {
    /// Checks the values that have no type of their own: environment values and labels.
    pub fn validate(&self) -> Result<(), Error> {
        if let Some(var) = self.env.iter().find(|var| var.value.contains('\0')) {
            return Err(invalid(format!(
                "the value of {} contains a NUL character",
                var.key.as_str()
            )));
        }
        for (key, value) in &self.labels {
            check_label(key, value)?;
        }
        Ok(())
    }
}

fn check_label(key: &str, value: &str) -> Result<(), Error> {
    let valid_key = !key.is_empty()
        && key.len() <= MAX_PATH_LENGTH
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
    if valid_key && !value.contains('\0') {
        Ok(())
    } else {
        Err(invalid(format!("invalid label: {key}")))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContainerHealth {
    Starting,
    Healthy,
    Unhealthy,
}

/// The state of one container.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerInfo {
    pub id: String,
    pub name: String,
    pub image_id: String,
    pub running: bool,
    pub exit_code: i64,
    #[serde(default)]
    pub oom_killed: bool,
    /// `None` when the image declares no health check.
    pub health: Option<ContainerHealth>,
    /// The IP address of the container on each network, IPv4 before IPv6.
    pub addresses: BTreeMap<String, String>,
    pub labels: BTreeMap<String, String>,
    /// The published TCP ports: the container port and its port on `127.0.0.1`.
    pub published: BTreeMap<u16, u16>,
}

/// One entry of a container list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerSummary {
    pub id: String,
    pub name: String,
    /// The Docker state, for example `running` or `exited`.
    pub state: String,
    pub labels: BTreeMap<String, String>,
}

/// Container log file size and status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ContainerLogInfo {
    pub container_id: String,
    pub container_name: String,
    pub log_path: Option<String>,
    pub size_bytes: u64,
    pub rotated_files_count: usize,
}

/// Aggregated container log files overview for an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppLogsOverview {
    pub app_id: String,
    pub total_size_bytes: u64,
    pub containers: Vec<ContainerLogInfo>,
}

/// The result of truncating / vacuuming container log files.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LogVacuumResult {
    pub app_id: Option<String>,
    pub truncated_files: usize,
    pub freed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInfo {
    pub id: String,
    /// The digests of the image, for example `nginx@sha256:…`. A pulled image has at least one.
    pub repo_digests: Vec<String>,
}

/// The resource usage of one container at one moment. The network counters are cumulative
/// since the container started; the caller keeps the previous sample for a delta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerStats {
    /// The CPU use of the last stats interval, as a percent of one core.
    pub cpu_percent: f64,
    pub memory_bytes: u64,
    pub memory_limit: u64,
    /// The bytes that every interface of the container received and sent, cumulative.
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    /// The number of periods that container tasks were throttled by CFS quota.
    #[serde(default)]
    pub cpu_throttled_periods: u64,
    /// Total enforcement periods that have elapsed for the container's cgroup.
    #[serde(default)]
    pub cpu_total_periods: u64,
    /// Total time tasks in the cgroup were throttled (in milliseconds).
    #[serde(default)]
    pub cpu_throttled_time_ms: u64,
    /// The percentage of elapsed CFS quota periods that were throttled (0.0 - 100.0).
    #[serde(default)]
    pub cpu_throttled_percent: f64,
    /// The memory allocation failure / limit hit counter (`failcnt` in cgroup v1 or `oom_kills` in v2).
    #[serde(default)]
    pub memory_fail_count: u64,
}

/// Health and diagnostic status of container resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceHealthStatus {
    Healthy,
    ThrottledWarning,
    ThrottledCritical,
    OomRisk,
    OomKilled,
}

/// Detailed diagnostic information about CPU throttling, CFS quotas, and memory/OOM pressure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AppResourceDiagnostics {
    #[schema(value_type = String)]
    pub app_id: ShortId,
    pub app_name: String,
    pub running: bool,
    pub status: ResourceHealthStatus,
    pub cpu_percent: f64,
    pub cpu_throttled_percent: f64,
    pub cpu_throttled_periods: u64,
    pub cpu_total_periods: u64,
    pub cpu_throttled_time_ms: u64,
    pub memory_bytes: u64,
    pub memory_limit: u64,
    pub memory_percent: f64,
    pub memory_fail_count: u64,
    pub oom_killed: bool,
    pub exit_code: i64,
    pub recommendation: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_accept_only_the_safe_characters() {
        for valid in ["web", "r3v3rs3-shop-01", "a.b_c-d", "0"] {
            assert!(valid.parse::<ContainerName>().is_ok(), "{valid}");
        }
        let too_long = "a".repeat(64);
        for invalid in [
            "", "-web", ".web", "Web", "web/1", "web 1", "a/../b", &too_long,
        ] {
            assert!(invalid.parse::<ContainerName>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn names_are_validated_when_deserialized() {
        let name: Result<NetworkName, _> = serde_json::from_str("\"--rm\"");
        assert!(name.is_err());
        let name: NetworkName = serde_json::from_str("\"r3v3rs3-shop\"").unwrap();
        assert_eq!(name.as_str(), "r3v3rs3-shop");
    }

    #[test]
    fn image_references_split_into_repository_and_tag() {
        let cases = [
            ("nginx", "nginx", "latest"),
            ("nginx:1.27", "nginx", "1.27"),
            (
                "registry.example:5000/app",
                "registry.example:5000/app",
                "latest",
            ),
            (
                "registry.example:5000/team/app:v2",
                "registry.example:5000/team/app",
                "v2",
            ),
            (
                "ghcr.io/owner/app@sha256:abc",
                "ghcr.io/owner/app",
                "sha256:abc",
            ),
        ];
        for (reference, repository, tag) in cases {
            let image: ImageRef = reference.parse().unwrap();
            assert_eq!(image.repository(), repository, "{reference}");
            assert_eq!(image.tag_or_digest(), tag, "{reference}");
        }
    }

    #[test]
    fn image_references_reject_unsafe_text() {
        for invalid in [
            "",
            "-nginx",
            "nginx:",
            "nginx latest",
            "nginx?x=1",
            "a@b@c",
            "a//b",
            "nginx&tag=1",
        ] {
            assert!(invalid.parse::<ImageRef>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn environment_keys_and_paths_are_checked() {
        assert!("DATABASE_URL".parse::<EnvKey>().is_ok());
        assert!("_x1".parse::<EnvKey>().is_ok());
        for invalid in ["", "1A", "A-B", "A=B"] {
            assert!(invalid.parse::<EnvKey>().is_err(), "{invalid}");
        }
        assert!("/var/lib/data".parse::<ContainerPath>().is_ok());
        for invalid in ["data", "/data/../etc", "/.."] {
            assert!(invalid.parse::<ContainerPath>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn a_spec_rejects_nul_values_and_invalid_labels() {
        let mut spec = ContainerSpec {
            name: "web".parse().unwrap(),
            image: "nginx".parse().unwrap(),
            env: vec![EnvVar {
                key: "A".parse().unwrap(),
                value: "1".into(),
            }],
            labels: BTreeMap::from([(APP_LABEL.to_string(), "shop".to_string())]),
            network: None,
            volumes: Vec::new(),
            restart: RestartPolicy::default(),
            limits: ResourceLimits::default(),
            publish: None,
            host_port: None,
            extra_hosts: Vec::new(),
            log_max_size: None,
            log_max_file: None,
        };
        assert!(spec.validate().is_ok());
        spec.env[0].value = "a\0b".into();
        assert!(spec.validate().is_err());
        spec.env.clear();
        spec.labels.insert("Bad Key".into(), "x".into());
        assert!(spec.validate().is_err());
    }
}
