//! The apps, the targets and the deployments of the deployment platform.

use crate::container::{
    AppName, EnvKey, ImageRef, ResourceLimits, RestartPolicy, ServiceName, VolumeMount,
};
use crate::error::Error;
use crate::git::{GitRef, RelPath, RepoUrl};
use crate::id::ShortId;
use crate::subject_name::SubjectName;
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::collections::HashSet;
use utoipa::{IntoParams, ToSchema};

/// The id of the target that runs the containers on the server of r3v3rs3 itself.
pub const LOCAL_TARGET: &str = "local";

/// One engine of the database catalog: the image tags that the panel offers and the values
/// that the connection information shows. The catalog lives in the `engines.json` of the
/// `r3v3rs3-apps` repository; the server installs it here at the start and refreshes it daily.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EngineInfo {
    /// The key of the engine, for example `postgres`. An app names its engine with it.
    pub engine: String,
    /// The name of the engine in the panel.
    pub label: String,
    /// The image tags that the panel offers, pinned to a major version.
    pub versions: Vec<String>,
    /// The port that the container listens on.
    pub container_port: u16,
    /// The environment variable that holds the generated password. Empty for an engine that
    /// takes no password, such as Redis.
    pub password_env: String,
    /// The user that the connection information names.
    pub user: String,
    /// The database that the connection information names.
    pub default_db: String,
    /// The scheme of the connection string, for example `postgres`.
    pub url_scheme: String,
    /// The prefix of the environment variables of a connected app, for example `POSTGRES`.
    pub env_prefix: String,
    /// Whether the connection string ends in the database name.
    pub url_database: bool,
    /// The directory inside the container that the data volume mounts.
    pub data_path: String,
}

/// The document of the `engines.json` catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnginesCatalog {
    pub version: u32,
    pub updated_at: i64,
    pub engines: Vec<EngineInfo>,
}

/// The installed engine catalog and the time of the document that it came from.
#[derive(Debug, Default)]
struct InstalledEngines {
    updated_at: i64,
    engines: Vec<EngineInfo>,
}

static ENGINES: std::sync::RwLock<InstalledEngines> = std::sync::RwLock::new(InstalledEngines {
    updated_at: 0,
    engines: Vec::new(),
});

/// Installs a catalog when it is newer than the installed one, and reports whether it was
/// installed. An engine without a key, or two engines with the same key, reject the catalog.
pub fn install_engines(catalog: EnginesCatalog) -> bool {
    let mut keys = HashSet::new();
    let valid = !catalog.engines.is_empty()
        && catalog.engines.iter().all(|info| {
            !info.engine.is_empty()
                && info
                    .engine
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && keys.insert(info.engine.clone())
        });
    if !valid {
        return false;
    }
    let mut installed = write_engines();
    if catalog.updated_at < installed.updated_at {
        return false;
    }
    installed.updated_at = catalog.updated_at;
    installed.engines = catalog.engines;
    true
}

/// The installed engine catalog.
pub fn engines() -> Vec<EngineInfo> {
    read_engines().engines.clone()
}

/// The `updated_at` of the installed catalog, `0` when no catalog is installed.
pub fn engines_updated_at() -> i64 {
    read_engines().updated_at
}

/// The catalog entry of one engine key.
pub fn engine_info(key: &str) -> Option<EngineInfo> {
    read_engines()
        .engines
        .iter()
        .find(|info| info.engine == key)
        .cloned()
}

fn read_engines() -> std::sync::RwLockReadGuard<'static, InstalledEngines> {
    ENGINES
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write_engines() -> std::sync::RwLockWriteGuard<'static, InstalledEngines> {
    ENGINES
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The backup settings of the managed databases, stored in the platform database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(default)]
pub struct BackupConfig {
    /// The time of day that the scheduler takes the backups at, as `HH:MM`. Empty turns the
    /// scheduler off.
    pub schedule: String,
    /// The backups of one database that survive; the oldest go first.
    pub keep: u32,
    /// The S3 bucket that receives a copy of every backup.
    pub s3: Option<S3Config>,
    /// The SFTP server target that receives a copy of every backup.
    pub sftp: Option<SftpConfig>,
    /// The rsync remote destination that receives a copy of every backup.
    pub rsync: Option<RsyncConfig>,
    /// Whether periodic restore testing (decompression and integrity validation) is performed.
    pub restore_test_enabled: bool,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            schedule: String::new(),
            keep: 7,
            s3: None,
            sftp: None,
            rsync: None,
            restore_test_enabled: true,
        }
    }
}

/// An S3 compatible bucket that the backups upload to.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct S3Config {
    /// The base URL of the bucket, for example `https://s3.example.com/bucket`.
    pub endpoint: String,
    pub region: String,
    /// The key prefix of the uploaded backups, for example `databases`.
    pub prefix: String,
    pub access_key: String,
    /// Written only: the API answers an empty value, and an empty value in a request keeps the
    /// stored one.
    #[serde(default)]
    pub secret_key: String,
}

/// An SFTP server target that the backups upload to.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SftpConfig {
    /// Hostname or IP address of the SFTP server.
    pub host: String,
    /// Port of the SFTP server (default 22).
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Username for authentication.
    pub user: String,
    /// Remote directory path where backups will be stored.
    pub path: String,
    /// Written only: password for authentication (stored sealed).
    #[serde(default)]
    pub password: String,
    /// Written only: SSH private key for authentication (stored sealed).
    #[serde(default)]
    pub key: String,
}

fn default_ssh_port() -> u16 {
    22
}

/// An rsync remote destination that the backups sync to.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RsyncConfig {
    /// Destination path (e.g. `user@host:/path` or `rsync://host/module`).
    pub destination: String,
    /// Custom SSH or rsync options (e.g. `-e 'ssh -p 2222'`).
    #[serde(default)]
    pub ssh_options: String,
    /// Written only: password for authentication (stored sealed).
    #[serde(default)]
    pub password: String,
}

/// Report of a backup dump decompression and integrity verification test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BackupVerifyReport {
    pub backup_id: String,
    pub verified: bool,
    pub uncompressed_size: u64,
    pub format_detected: String,
    pub message: String,
}

/// Report of platform.key rotation and secret re-encryption across SQLite tables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct KeyRotationReport {
    pub new_key_id: String,
    pub resealed_app_env: u32,
    pub resealed_git_connections: u32,
    pub resealed_github_apps: u32,
    pub resealed_settings: u32,
    pub resealed_app_secrets: u32,
    #[serde(default)]
    pub resealed_registries: u32,
}

/// A container registry credential entry for private image pulls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RegistryEntry {
    pub id: ShortId,
    pub server: String,
    pub username: String,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Request to create or update container registry credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RegistryRequest {
    pub server: String,
    pub username: String,
    /// Written only: password or access token for authentication (stored sealed).
    #[serde(default)]
    pub password: Option<String>,
}

/// Auth credentials passed to the container runtime for image pulls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RegistryAuth {
    pub username: String,
    pub password: String,
    pub serveraddress: String,
}

/// One recorded sample of the metrics of an app. The network and the request counters are the
/// deltas against the previous sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct AppMetricsPoint {
    pub ts: i64,
    pub running: bool,
    /// The container health, empty when the image declares no health check.
    pub health: String,
    /// The CPU use as a percent of one core.
    pub cpu: f64,
    pub memory: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub requests: u64,
}

/// The query of the metrics of one app.
#[derive(Debug, Deserialize, IntoParams)]
pub struct MetricsQuery {
    /// How far back the history goes, from 1 to 24 hours. The default is 6.
    pub hours: Option<u32>,
}

/// One stored backup of a managed database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct BackupEntry {
    pub id: String,
    pub app: ShortId,
    /// The name of the database app.
    pub name: String,
    /// The key of the engine in the catalog.
    pub engine: String,
    /// The compressed size in bytes.
    pub size: u64,
    /// The SHA-256 of the compressed file.
    pub sha256: String,
    pub created_at: i64,
    /// Unix timestamp in milliseconds when the backup was verified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verified_at: Option<i64>,
    /// Verification result message or status ("valid", "error: ...").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_status: Option<String>,
}

/// The longest health check path.
const MAX_HEALTH_PATH_LENGTH: usize = 1024;

/// The `[platform]` section of `config.toml`. The admin API does not change it.
#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PlatformConfig {
    /// Whether the server deploys apps. A cluster does not run the platform.
    #[serde(default)]
    pub enabled: bool,

    /// The Docker Engine API of the local target.
    #[serde(default = "default_docker_endpoint")]
    #[schema(example = "unix:///var/run/docker.sock")]
    pub docker: String,

    /// The names or the ids of the ports that serve the apps, for example the HTTP port and the
    /// HTTPS port.
    #[serde(default)]
    #[schema(example = json!(["http", "https"]))]
    pub proxy_ports: Vec<String>,

    /// The ACME entry that orders the certificates of the app domains. Without it the apps get
    /// no certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub acme: Option<ShortId>,

    /// The TCP port that the agents connect to. Without it the server accepts no agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = 9443)]
    pub agent_port: Option<u16>,

    /// The host name or the address that an agent dials, as the enrollment command shows it.
    /// Without it the WebUI shows the host name of its own page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "master.example.com")]
    pub agent_host: Option<String>,
}

fn default_docker_endpoint() -> String {
    "unix:///var/run/docker.sock".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    /// The Docker Engine of the server that runs r3v3rs3.
    Local,
    /// A remote server that runs `r3v3rs3 agent`.
    Agent,
}

/// A Docker host that runs the containers of apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TargetEntry {
    #[schema(value_type = String)]
    pub id: ShortId,
    pub name: String,
    pub kind: TargetKind,
    /// The Unix time in milliseconds of the last contact with an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<u64>,
    /// Whether an agent enrolled for the target. The local target is always enrolled.
    #[serde(default)]
    pub enrolled: bool,
    /// Whether the target runs containers now: the local target, or a connected agent.
    #[serde(default)]
    pub online: bool,
    /// The version of the connected agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// A new agent target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TargetRequest {
    #[schema(value_type = String, example = "edge-1")]
    pub name: AppName,
}

/// An agent target with its new enrollment token. The token is shown only once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TargetToken {
    pub target: TargetEntry,
    /// The one-time token of `r3v3rs3 agent --token`.
    pub token: String,
    /// The agent port of the master, which `r3v3rs3 agent --master` names.
    pub agent_port: u16,
    /// The `agent_host` of the `[platform]` section, which the agent dials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_host: Option<String>,
}

/// Where the image of an app comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppSource {
    /// A ready image from a registry.
    Image {
        #[schema(value_type = String, example = "nginx:1.27")]
        image: ImageRef,
    },
    /// A branch of a Git repository that r3v3rs3 builds with its Dockerfile. A private
    /// repository needs a Git provider connection or the Git token of the app.
    Git {
        #[schema(value_type = String, example = "https://github.com/owner/shop.git")]
        repository: RepoUrl,
        #[serde(default = "GitRef::main")]
        #[schema(value_type = String, example = "main")]
        branch: GitRef,
        /// The directory of the repository that Docker receives as the build context.
        #[serde(default = "RelPath::root")]
        #[schema(value_type = String, example = ".")]
        context: RelPath,
        /// The Dockerfile, relative to the context.
        #[serde(default = "RelPath::dockerfile")]
        #[schema(value_type = String, example = "Dockerfile")]
        dockerfile: RelPath,
        /// The Git provider connection that clones the repository and installs its webhook.
        /// Without it the Git token of the app clones a private repository.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schema(value_type = Option<String>)]
        connection: Option<ShortId>,
    },
    /// A branch of a Git repository that r3v3rs3 starts with `docker compose`. A deployment
    /// recreates the changed services instead of running the new version next to the old one.
    Compose {
        #[schema(value_type = String, example = "https://github.com/owner/shop.git")]
        repository: RepoUrl,
        #[serde(default = "GitRef::main")]
        #[schema(value_type = String, example = "main")]
        branch: GitRef,
        /// The Compose file. Without it the first of `compose.yaml`, `compose.yml`,
        /// `docker-compose.yaml` and `docker-compose.yml` in the root of the repository.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schema(value_type = Option<String>, example = "compose.yaml")]
        file: Option<RelPath>,
        /// The service that receives the requests of the domains on `port`.
        #[schema(value_type = String, example = "web")]
        service: ServiceName,
        /// The Git provider connection that clones the repository and installs its webhook.
        /// Without it the Git token of the app clones a private repository.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schema(value_type = Option<String>)]
        connection: Option<ShortId>,
    },
}

impl AppSource {
    /// The repository of a Git or a Compose source and its Git provider connection.
    pub fn repository(&self) -> Option<(&RepoUrl, Option<ShortId>)> {
        match self {
            Self::Image { .. } => None,
            Self::Git {
                repository,
                connection,
                ..
            }
            | Self::Compose {
                repository,
                connection,
                ..
            } => Some((repository, *connection)),
        }
    }
}

/// What a deployment of an app runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppSpec {
    pub source: AppSource,

    /// The port that the container listens on.
    #[schema(example = 8080)]
    pub port: u16,

    /// The host names that route to the app.
    #[serde(default)]
    #[schema(value_type = Vec<String>, example = json!(["app.example.com"]))]
    pub domains: Vec<SubjectName>,

    /// The names or the ids of the ports that serve this app. Empty means the ports of
    /// `proxy_ports` in the `[platform]` section, or every HTTP and HTTPS port when that
    /// list is also empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<String>, example = json!(["http", "https"]))]
    pub ports: Vec<String>,

    /// An HTTP path that answers 2xx or 3xx when the app is ready. Without it a deployment waits
    /// until the port accepts a connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = "/healthz")]
    pub health_check_path: Option<String>,

    #[serde(default)]
    pub volumes: Vec<VolumeMount>,

    #[serde(default)]
    pub restart: RestartPolicy,

    #[serde(default)]
    pub limits: ResourceLimits,

    /// The fixed host port on `127.0.0.1` that every deployment publishes the container port
    /// on. Without it a deployment takes a free port, so the address changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = 5432)]
    pub host_port: Option<u16>,

    /// The key of the engine in the catalog that this app serves, for example `postgres`. A
    /// database app has no domains, so it gets no
    /// proxy: clients reach it on the host port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,

    /// The id of the database app that this app uses. A deployment injects its connection
    /// into the environment of the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub uses_database: Option<ShortId>,

    /// Whether updates to the app image (newer tag or digest) should be deployed automatically.
    #[serde(default)]
    pub auto_deploy: bool,

    /// Whether deployments of this app require manual approval before proceeding.
    #[serde(default)]
    pub require_approval: bool,

    /// An optional daily UTC time window (e.g. `22:00-04:00`) when deployments are allowed to
    /// run automatically. Deployments outside this window require approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy_window: Option<String>,

    /// Optional tag pattern (e.g. `v*`, `release-*`) that triggers an automatic deployment when pushed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag_trigger: Option<String>,

    /// Optional app store catalog entry id that this app was installed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_app_id: Option<String>,
}

impl AppSpec {
    pub fn validate(&self) -> Result<(), Error> {
        if self.port == 0 {
            return Err(invalid("the port must be between 1 and 65535"));
        }
        self.validate_domains()?;
        self.validate_compose()?;
        self.validate_optional_fields()?;
        self.validate_volumes()?;
        self.validate_database()
    }

    fn validate_optional_fields(&self) -> Result<(), Error> {
        if let Some(path) = &self.health_check_path {
            validate_health_path(path)?;
        }
        if let Some(window) = &self.deploy_window {
            validate_deploy_window(window)?;
        }
        self.validate_tag_trigger()
    }

    fn validate_tag_trigger(&self) -> Result<(), Error> {
        let Some(pattern) = &self.tag_trigger else {
            return Ok(());
        };
        if matches!(self.source, AppSource::Image { .. }) {
            return Err(invalid("a tag trigger requires a git or compose source"));
        }
        validate_tag_pattern(pattern)
    }

    fn validate_volumes(&self) -> Result<(), Error> {
        let mut targets = HashSet::new();
        if let Some(mount) = self
            .volumes
            .iter()
            .find(|m| !targets.insert(m.target.as_str()))
        {
            return Err(invalid(&format!(
                "the path {} is mounted twice",
                mount.target.as_str()
            )));
        }
        Ok(())
    }

    /// A database app is an image app without domains: its clients reach it on the host port,
    /// so a route and a certificate would only expose it. A database server cannot use another
    /// database, and its engine must be in the installed catalog.
    fn validate_database(&self) -> Result<(), Error> {
        if let Some(engine) = &self.database {
            if engine_info(engine).is_none() {
                return Err(invalid(&format!(
                    "the engine {engine} is not in the database catalog"
                )));
            }
            if !matches!(self.source, AppSource::Image { .. }) {
                return Err(invalid(&format!(
                    "a database app runs an image, not a {} source",
                    engine
                )));
            }
            if let Some(domain) = self.domains.first() {
                return Err(invalid(&format!(
                    "a database app has no domains, but {} is listed",
                    domain
                )));
            }
            if self.uses_database.is_some() {
                return Err(invalid("a database app cannot use another database"));
            }
        }
        Ok(())
    }

    /// A domain gets a certificate from ACME, so it is one DNS name, not a wildcard or an IP
    /// address.
    fn validate_domains(&self) -> Result<(), Error> {
        let mut seen = HashSet::new();
        for domain in &self.domains {
            let SubjectName::DnsName(name) = domain else {
                return Err(invalid(&format!("the domain {domain} is not a DNS name")));
            };
            if !seen.insert(name.to_ascii_lowercase()) {
                return Err(invalid(&format!("the domain {domain} is listed twice")));
            }
        }
        Ok(())
    }

    /// The Compose file sets the volumes, the restart policy and the limits of its services.
    fn validate_compose(&self) -> Result<(), Error> {
        let compose = matches!(self.source, AppSource::Compose { .. });
        let customized = !self.volumes.is_empty()
            || self.restart != RestartPolicy::default()
            || self.limits != ResourceLimits::default();
        if compose && customized {
            return Err(invalid(
                "a Compose app sets its volumes, restart policy and limits in its Compose file",
            ));
        }
        Ok(())
    }
}

fn validate_health_path(path: &str) -> Result<(), Error> {
    let valid = path.starts_with('/')
        && path.len() <= MAX_HEALTH_PATH_LENGTH
        && path.chars().all(|c| c.is_ascii_graphic());
    if valid {
        Ok(())
    } else {
        Err(invalid(&format!("invalid health check path: {path}")))
    }
}

/// Validates that a deployment window has the form `HH:MM-HH:MM`.
pub fn validate_deploy_window(window: &str) -> Result<(), Error> {
    let Some((start, end)) = window.split_once('-') else {
        return Err(invalid("deploy window must be in format HH:MM-HH:MM"));
    };
    validate_window_time(start)?;
    validate_window_time(end)
}

fn validate_window_time(s: &str) -> Result<(), Error> {
    let Some((h_str, m_str)) = s.split_once(':') else {
        return Err(invalid("deploy window time must be in format HH:MM"));
    };
    let Ok(h) = h_str.parse::<u32>() else {
        return Err(invalid("invalid hour in deploy window"));
    };
    let Ok(m) = m_str.parse::<u32>() else {
        return Err(invalid("invalid minute in deploy window"));
    };
    if h > 23 || m > 59 {
        return Err(invalid("hour must be 0-23 and minute 0-59"));
    }
    Ok(())
}

/// Checks whether the given hour and minute (UTC) fall within `HH:MM-HH:MM`.
pub fn is_within_deploy_window(window: &str, hour: u32, minute: u32) -> bool {
    let Some((start_str, end_str)) = window.split_once('-') else {
        return true;
    };
    let (Some(start), Some(end)) = (
        parse_window_minutes(start_str),
        parse_window_minutes(end_str),
    ) else {
        return true;
    };
    let cur = hour * 60 + minute;
    if start <= end {
        cur >= start && cur < end
    } else {
        cur >= start || cur < end
    }
}

fn parse_window_minutes(s: &str) -> Option<u32> {
    let (h_str, m_str) = s.split_once(':')?;
    let (h, m) = (h_str.parse::<u32>().ok()?, m_str.parse::<u32>().ok()?);
    Some(h * 60 + m)
}

/// Validates a git tag pattern.
pub fn validate_tag_pattern(pattern: &str) -> Result<(), Error> {
    if pattern.is_empty() || pattern.len() > 128 {
        return Err(invalid("tag pattern must be between 1 and 128 characters"));
    }
    if !pattern.chars().all(is_valid_tag_char) {
        return Err(invalid(
            "tag pattern contains invalid characters (allowed: alphanumeric, ., -, _, *, ?, /, +, @)",
        ));
    }
    Ok(())
}

fn is_valid_tag_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '*' | '?' | '/' | '+' | '@')
}

fn char_matches(p: u8, t: u8) -> bool {
    p == t || p == b'?'
}

fn skip_trailing_stars(p: &[u8], mut idx: usize) -> usize {
    while idx < p.len() && p[idx] == b'*' {
        idx += 1;
    }
    idx
}

/// Matches a git tag against a pattern supporting `*` (zero or more chars) and `?` (single char).
pub fn matches_tag_pattern(pattern: &str, tag: &str) -> bool {
    let p = pattern.as_bytes();
    let t = tag.as_bytes();
    let mut p_idx = 0;
    let mut t_idx = 0;
    let mut star = None;
    let mut match_idx = 0;

    while t_idx < t.len() {
        if p_idx < p.len() && char_matches(p[p_idx], t[t_idx]) {
            p_idx += 1;
            t_idx += 1;
        } else if p_idx < p.len() && p[p_idx] == b'*' {
            star = Some(p_idx);
            p_idx += 1;
            match_idx = t_idx;
        } else if let Some(s) = star {
            p_idx = s + 1;
            match_idx += 1;
            t_idx = match_idx;
        } else {
            return false;
        }
    }

    skip_trailing_stars(p, p_idx) == p.len()
}

fn invalid(reason: &str) -> Error {
    Error::InvalidContainerSpec {
        reason: reason.to_string(),
    }
}

/// The body that creates a database. The version is one image tag of the catalog entry of
/// the engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DatabaseRequest {
    #[schema(value_type = String, example = "main-postgres")]
    pub name: AppName,
    pub engine: String,
    #[schema(value_type = String, example = "postgres:17-alpine")]
    pub version: String,
    /// A fixed port of `127.0.0.1`. Empty takes a free port, which stays the port of the
    /// database afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(example = 5432)]
    pub host_port: Option<u16>,
}

/// The new database. The password is shown once, like a webhook secret.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct DatabaseCreated {
    pub app: AppEntry,
    /// The generated password. `None` for an engine that takes no password, such as Redis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    /// The fixed port of `127.0.0.1` that every deployment publishes the port on.
    pub host_port: u16,
    /// The user that the connection string names.
    pub user: String,
    /// The database that the connection string names.
    pub database: String,
}

/// The body that creates or replaces an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppRequest {
    #[schema(value_type = String, example = "shop")]
    pub name: AppName,
    /// The id of the target that runs the app.
    #[schema(value_type = String, example = "local")]
    pub target: ShortId,
    pub spec: AppSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppEntry {
    #[schema(value_type = String)]
    pub id: ShortId,
    #[schema(value_type = String)]
    pub name: AppName,
    #[schema(value_type = String)]
    pub target: ShortId,
    pub spec: AppSpec,
    /// Whether the app has a Git token. The admin API never returns the token.
    #[serde(default)]
    pub git_token_set: bool,
    /// Whether the app has a webhook secret, so that `POST /hooks/apps/{id}` deploys it. The
    /// admin API returns the secret only once, when it creates the secret.
    #[serde(default)]
    pub webhook_secret_set: bool,
    /// The webhook that the Git provider connection of the app installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook: Option<AppHook>,
    /// The image update status of the app, when checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<ImageUpdateStatus>,
    /// A newer version of this app is available in the app store catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_update: Option<CatalogUpdateStatus>,
    /// The Unix time in milliseconds.
    pub created_at: u64,
    /// The Unix time in milliseconds.
    pub updated_at: u64,
}

/// The app store catalog update status of an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct CatalogUpdateStatus {
    pub store_app_id: String,
    pub current_version: String,
    pub catalog_version: String,
}

/// The image update status of an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ImageUpdateStatus {
    /// Whether an update is available (either a newer tag or a newer digest for current tag).
    pub update_available: bool,
    /// The current image tag of the app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_tag: Option<String>,
    /// The newer tag found in registry, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_tag: Option<String>,
    /// The digest of the running deployment, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_digest: Option<String>,
    /// The remote digest of the latest tag or current tag in the registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_digest: Option<String>,
    /// The Unix time in milliseconds when the check was performed.
    pub checked_at: u64,
}

/// The webhook of an app at the Git provider of its connection. It sends the push events of the
/// repository to `POST /hooks/apps/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppHook {
    #[schema(value_type = String)]
    pub connection: ShortId,
    /// The repository at the provider.
    #[schema(example = "owner/shop")]
    pub repository: String,
    /// Whether the provider holds the webhook.
    pub installed: bool,
    /// Why the last installation failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The JSON of the API error of a failed installation, which a client shows in its own
    /// language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    /// The Unix time in milliseconds of the last installation.
    pub updated_at: u64,
}

/// The access token that clones the private repository of a Git app.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct GitTokenRequest {
    pub token: String,
}

/// The token stays out of debug output and logs.
impl std::fmt::Debug for GitTokenRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitTokenRequest").finish_non_exhaustive()
    }
}

/// A new webhook secret of an app. The admin API returns it only in this response.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WebhookSecret {
    /// The secret that signs the requests of the Git provider.
    pub secret: String,
}

/// The secret stays out of debug output and logs.
impl std::fmt::Debug for WebhookSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookSecret").finish_non_exhaustive()
    }
}

/// What a webhook request did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HookOutcome {
    /// The request started a deployment.
    Deployed,
    /// A deployment of the app was running, so a new deployment starts after it.
    Queued,
    /// The request was a ping, or an event or a branch that does not deploy the app.
    Ignored,
}

/// The response of a webhook request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HookResponse {
    pub outcome: HookOutcome,
    /// The deployment that the request started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<DeploymentEntry>,
}

/// The container log of the running deployment of an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppLog {
    /// The last lines of stdout and stderr. Empty when the app has no running deployment.
    pub log: String,
    /// Whether the app has a running deployment.
    pub running: bool,
}

/// The query of an app log request.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AppLogQuery {
    /// The number of lines, from 1 to 1000. The default is 200.
    pub tail: Option<u32>,
}

/// One entry of a directory inside a volume of an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FileEntry {
    /// The name of the entry, without its directory.
    #[schema(value_type = String, example = "app.db")]
    pub name: String,
    /// The size of a file in bytes, and `0` for a directory.
    pub size: u64,
    /// Whether the entry is a directory.
    pub dir: bool,
}

/// The query of an app file request.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct FilePathQuery {
    /// The absolute path of the file or directory inside the container, for example `/data/app.db`.
    pub path: String,
}

/// One environment variable of an app. The admin API does not return the value of a secret
/// variable. An update without a value keeps the current value of a secret variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EnvEntry {
    #[schema(value_type = String, example = "DATABASE_URL")]
    pub key: EnvKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default)]
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentStatus {
    Queued,
    PendingApproval,
    Building,
    Deploying,
    Running,
    /// A newer deployment of the app replaced this one.
    Superseded,
    Failed,
    Cancelled,
}

impl DeploymentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::PendingApproval => "pending_approval",
            Self::Building => "building",
            Self::Deploying => "deploying",
            Self::Running => "running",
            Self::Superseded => "superseded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentTrigger {
    Manual,
    Webhook,
    Rollback,
    Api,
    Auto,
}

impl DeploymentTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Webhook => "webhook",
            Self::Rollback => "rollback",
            Self::Api => "api",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeploymentEntry {
    #[schema(value_type = String)]
    pub id: ShortId,
    #[schema(value_type = String)]
    pub app: ShortId,
    pub status: DeploymentStatus,
    pub trigger: DeploymentTrigger,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit_sha: Option<String>,
    /// The image digest that the deployment runs. A rollback starts this digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_digest: Option<String>,
    /// The account that started the deployment.
    pub username: String,
    /// The Unix time in milliseconds.
    pub started_at: u64,
    /// The Unix time in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<u64>,
    /// The reason of a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

fn default_true() -> bool {
    true
}

/// A scheduled command configured for an app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AppScheduledCommand {
    #[schema(value_type = String)]
    pub id: ShortId,
    #[schema(value_type = String)]
    pub app_id: ShortId,
    pub name: String,
    pub schedule: String,
    pub command: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Request to create or update a scheduled command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ScheduledCommandRequest {
    pub name: String,
    pub schedule: String,
    pub command: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// The result of running a scheduled command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ScheduledCommandRunResult {
    pub success: bool,
    pub status: String,
    pub output: String,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(value: serde_json::Value) -> Result<AppSpec, String> {
        let spec: AppSpec = serde_json::from_value(value).map_err(|err| err.to_string())?;
        spec.validate().map_err(|err| err.to_string())?;
        Ok(spec)
    }

    /// Installs a one-engine catalog for the tests that validate a database spec.
    fn install_test_engines() {
        let info = EngineInfo {
            engine: "postgres".to_string(),
            label: "PostgreSQL".to_string(),
            versions: vec!["postgres:17-alpine".to_string()],
            container_port: 5432,
            password_env: "POSTGRES_PASSWORD".to_string(),
            user: "postgres".to_string(),
            default_db: "postgres".to_string(),
            url_scheme: "postgres".to_string(),
            env_prefix: "POSTGRES".to_string(),
            url_database: true,
            data_path: "/var/lib/postgresql/data".to_string(),
        };
        install_engines(EnginesCatalog {
            version: 1,
            updated_at: 1,
            engines: vec![info],
        });
    }

    #[test]
    fn a_spec_with_an_image_and_a_port_is_valid() {
        let parsed = spec(json!({
            "source": {"type": "image", "image": "nginx:1.27"},
            "port": 80,
            "domains": ["shop.example.com"],
            "health_check_path": "/healthz",
            "volumes": [{"volume": "shop-data", "target": "/data"}],
        }))
        .unwrap();
        assert_eq!(parsed.restart, RestartPolicy::UnlessStopped);
        assert_eq!(parsed.limits, ResourceLimits::default());
    }

    #[test]
    fn a_spec_rejects_what_a_deployment_cannot_serve() {
        let base = || json!({"source": {"type": "image", "image": "nginx"}, "port": 80});
        let cases = [
            ("port", json!(0)),
            ("domains", json!(["*.example.com"])),
            ("domains", json!(["192.0.2.1"])),
            ("domains", json!(["a.example.com", "A.example.com"])),
            ("health_check_path", json!("healthz")),
            ("health_check_path", json!("/a b")),
            (
                "volumes",
                json!([
                    {"volume": "a", "target": "/data"},
                    {"volume": "b", "target": "/data"},
                ]),
            ),
        ];
        for (field, value) in cases {
            let mut input = base();
            input[field] = value.clone();
            assert!(spec(input).is_err(), "{field}: {value}");
        }
        let mut unsafe_image = base();
        unsafe_image["source"]["image"] = json!("nginx --privileged");
        assert!(spec(unsafe_image).is_err());
    }

    #[test]
    fn a_database_spec_takes_a_host_port_and_no_domains() {
        install_test_engines();
        let parsed = spec(json!({
            "source": {"type": "image", "image": "postgres:17-alpine"},
            "port": 5432,
            "host_port": 15432,
            "database": "postgres",
            "volumes": [{"volume": "db-data", "target": "/var/lib/postgresql/data"}],
        }))
        .unwrap();
        assert_eq!(parsed.host_port, Some(15432));
        assert_eq!(parsed.database, Some("postgres".to_string()));
    }

    #[test]
    fn a_database_spec_rejects_what_a_database_cannot_be() {
        install_test_engines();
        let base = || {
            json!({
                "source": {"type": "image", "image": "postgres:17-alpine"},
                "port": 5432,
                "database": "postgres",
            })
        };
        let mut with_domain = base();
        with_domain["domains"] = json!(["db.example.com"]);
        assert!(spec(with_domain).is_err());
        let with_git = json!({
            "source": {"type": "git", "repository": "https://example.com/a.git"},
            "port": 5432,
            "database": "postgres",
        });
        assert!(spec(with_git).is_err());
    }

    #[test]
    fn a_git_source_gets_the_default_branch_and_paths() {
        let parsed = spec(json!({
            "source": {"type": "git", "repository": "https://github.com/owner/shop.git"},
            "port": 8080,
        }))
        .unwrap();
        let AppSource::Git {
            branch,
            context,
            dockerfile,
            ..
        } = parsed.source
        else {
            panic!("a git source");
        };
        assert_eq!(
            (branch.as_str(), context.as_str(), dockerfile.as_str()),
            ("main", ".", "Dockerfile")
        );
        for (field, value) in [
            ("repository", "http://github.com/owner/shop"),
            ("branch", "--upload-pack=x"),
            ("context", "../outside"),
            ("dockerfile", "/etc/passwd"),
        ] {
            let mut source = json!({"type": "git", "repository": "https://example.com/a.git"});
            source[field] = json!(value);
            assert!(
                spec(json!({"source": source, "port": 80})).is_err(),
                "{field}"
            );
        }
    }

    #[test]
    fn a_compose_source_names_its_service_and_leaves_the_rest_to_the_file() {
        let source = json!({
            "type": "compose",
            "repository": "https://github.com/owner/shop.git",
            "service": "web",
        });
        let parsed = spec(json!({"source": source, "port": 80})).unwrap();
        let AppSource::Compose {
            branch,
            file,
            service,
            ..
        } = parsed.source
        else {
            panic!("a compose source");
        };
        assert_eq!((branch.as_str(), service.as_str()), ("main", "web"));
        assert!(file.is_none());

        let mut without_service = source.clone();
        without_service.as_object_mut().unwrap().remove("service");
        assert!(spec(json!({"source": without_service, "port": 80})).is_err());
        for (field, value) in [
            ("volumes", json!([{"volume": "data", "target": "/data"}])),
            ("restart", json!("always")),
            ("limits", json!({"memory_bytes": 1024})),
        ] {
            let mut input = json!({"source": source, "port": 80});
            input[field] = value;
            assert!(spec(input).is_err(), "{field}");
        }
    }

    #[test]
    fn install_rejects_a_catalog_that_cannot_be_served() {
        let info = |engine: &str| EngineInfo {
            engine: engine.to_string(),
            label: "X".to_string(),
            versions: vec!["x:1".to_string()],
            container_port: 1,
            password_env: String::new(),
            user: String::new(),
            default_db: String::new(),
            url_scheme: "x".to_string(),
            env_prefix: "X".to_string(),
            url_database: false,
            data_path: "/data".to_string(),
        };
        let catalog = |engines: Vec<EngineInfo>| EnginesCatalog {
            version: 1,
            updated_at: 1,
            engines,
        };
        assert!(!install_engines(catalog(Vec::new())));
        assert!(!install_engines(catalog(vec![info("")])));
        assert!(!install_engines(catalog(vec![info("Postgres")])));
        assert!(!install_engines(catalog(vec![info("pg"), info("pg")])));
    }

    #[test]
    fn a_secret_env_entry_omits_its_value() {
        let entry = EnvEntry {
            key: "TOKEN".parse().unwrap(),
            value: None,
            secret: true,
        };
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({"key": "TOKEN", "secret": true})
        );
    }

    #[test]
    fn deploy_window_validation_and_check() {
        assert!(validate_deploy_window("22:00-04:00").is_ok());
        assert!(validate_deploy_window("09:00-17:00").is_ok());
        assert!(validate_deploy_window("invalid").is_err());
        assert!(validate_deploy_window("25:00-04:00").is_err());
        assert!(validate_deploy_window("22:00-04:65").is_err());

        // Overnight window: 22:00 to 04:00
        assert!(is_within_deploy_window("22:00-04:00", 23, 0));
        assert!(is_within_deploy_window("22:00-04:00", 2, 30));
        assert!(is_within_deploy_window("22:00-04:00", 22, 0));
        assert!(!is_within_deploy_window("22:00-04:00", 4, 0));
        assert!(!is_within_deploy_window("22:00-04:00", 12, 0));

        // Daytime window: 09:00 to 17:00
        assert!(is_within_deploy_window("09:00-17:00", 9, 0));
        assert!(is_within_deploy_window("09:00-17:00", 12, 0));
        assert!(!is_within_deploy_window("09:00-17:00", 8, 59));
        assert!(!is_within_deploy_window("09:00-17:00", 17, 0));
    }

    #[test]
    fn tag_pattern_validation_and_matching() {
        assert!(validate_tag_pattern("v*").is_ok());
        assert!(validate_tag_pattern("release-*").is_ok());
        assert!(validate_tag_pattern("*").is_ok());
        assert!(validate_tag_pattern("v1.2.3").is_ok());
        assert!(validate_tag_pattern("").is_err());
        assert!(validate_tag_pattern("v 1.0").is_err());
        assert!(validate_tag_pattern("v$1").is_err());

        assert!(matches_tag_pattern("v*", "v1.0.0"));
        assert!(matches_tag_pattern("v*", "v"));
        assert!(!matches_tag_pattern("v*", "1.0.0"));
        assert!(matches_tag_pattern("*", "anything"));
        assert!(matches_tag_pattern("release-*", "release-2026.1"));
        assert!(!matches_tag_pattern("release-*", "rel-2026.1"));
        assert!(matches_tag_pattern("*-prod", "v1.0-prod"));
        assert!(!matches_tag_pattern("*-prod", "v1.0-dev"));
        assert!(matches_tag_pattern("v?.*", "v1.2"));
        assert!(!matches_tag_pattern("v?.*", "v10.2"));
        assert!(matches_tag_pattern("v1.0", "v1.0"));
        assert!(!matches_tag_pattern("v1.0", "v1.0.1"));
    }
}
