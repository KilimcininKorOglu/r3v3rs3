//! The fields of the app form and the request that they make.

use super::accounts::{HINT_CLASS, INPUT_CLASS, LABEL_CLASS};
use super::app_repository::RepositoryPicker;
use super::resource_page::text_input;
use r3v3rs3_api::container::{ResourceLimits, RestartPolicy, VolumeMount};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git::{GitRef, RelPath, RepoUrl};
use r3v3rs3_api::git_connection::GitConnectionEntry;
use r3v3rs3_api::i18n::Locale;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppRequest, AppSource, AppSpec, BuildKitConfig, CacheMountPreset, GitConfigFile,
    LOCAL_TARGET, PreviewConfig, TargetEntry,
};
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::store::StoreApp;
use r3v3rs3_api::upstream::{DEFAULT_STICKY_COOKIE_NAME, StickyCookie};
use std::str::FromStr;
use std::time::Duration;
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

const BYTES_PER_MB: u64 = 1024 * 1024;
const NANO_CPUS_PER_CPU: f64 = 1_000_000_000.0;

/// Each source kind with its value in the select and the translation key of its name.
const KINDS: [(SourceKind, &str, &str); 3] = [
    (SourceKind::Image, "image", "apps.source_image"),
    (SourceKind::Git, "git", "apps.source_git"),
    (SourceKind::Compose, "compose", "apps.source_compose"),
];

/// Each restart policy with its value in the select and the translation key of its name.
const RESTARTS: [(RestartPolicy, &str, &str); 4] = [
    (
        RestartPolicy::UnlessStopped,
        "unless-stopped",
        "apps.restart_unless_stopped",
    ),
    (RestartPolicy::Always, "always", "apps.restart_always"),
    (
        RestartPolicy::OnFailure,
        "on-failure",
        "apps.restart_on_failure",
    ),
    (RestartPolicy::No, "no", "apps.restart_no"),
];

#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum SourceKind {
    #[default]
    Image,
    Git,
    Compose,
}

/// The raw values of the form. Every source kind keeps its own fields, so a change of the kind
/// loses no input.
#[derive(Clone, PartialEq, Debug)]
pub struct AppForm {
    pub name: String,
    pub target: String,
    pub kind: SourceKind,
    pub image: String,
    /// The id of the Git provider connection, or empty for none.
    pub connection: String,
    pub repository: String,
    pub branch: String,
    pub context: String,
    pub dockerfile: String,
    pub compose_file: String,
    pub service: String,
    pub port: String,
    /// One domain on each line.
    pub domains: String,
    /// The ids of the ports that serve the app, as the checkboxes select them.
    pub ports: Vec<String>,
    /// One extra port name or id on each line, beyond the port list.
    pub custom_ports: String,
    pub health_check_path: String,
    /// One mount on each line: `volume:/path` or `volume:/path:ro`.
    pub volumes: String,
    pub restart: RestartPolicy,
    pub memory_mb: String,
    pub swap_mb: String,
    pub cpus: String,
    /// The fixed host port and the engine of a database app, kept from the saved app. The
    /// app form shows neither, because the Databases page manages them.
    pub host_port: Option<u16>,
    pub database: Option<String>,
    /// The database that the app uses. The select of the form lists the databases.
    pub uses_database: Option<ShortId>,
    pub auto_deploy: bool,
    pub require_approval: bool,
    pub deploy_window: String,
    pub tag_trigger: String,
    pub store_app_id: Option<String>,
    pub targets: Vec<String>,
    pub load_balancing: String,
    pub alert_cpu_percent: String,
    pub alert_memory_mb: String,
    pub sticky_enabled: bool,
    pub sticky_name: String,
    pub sticky_max_age: String,
    pub git_config_path: String,
    pub git_config_repo: String,
    pub git_config_branch: String,
    pub env_groups: Vec<String>,
    pub preview_enabled: bool,
    pub preview_domain_template: String,
    pub buildkit_enabled: bool,
    pub buildkit_remote_addr: String,
    pub buildkit_cache_cargo: bool,
    pub buildkit_cache_npm: bool,
    pub buildkit_cache_go: bool,
    pub buildkit_cache_pip: bool,
    pub buildkit_custom_cache: String,
    pub buildkit_cache_from: String,
    pub buildkit_cache_to: String,
    pub bot_blocker: bool,
    pub grpc_web: bool,
    pub project_id: Option<String>,
    pub environment_id: Option<String>,
}

impl Default for AppForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            target: LOCAL_TARGET.to_string(),
            kind: SourceKind::default(),
            image: String::new(),
            connection: String::new(),
            repository: String::new(),
            branch: GitRef::main().to_string(),
            context: RelPath::root().to_string(),
            dockerfile: RelPath::dockerfile().to_string(),
            compose_file: String::new(),
            service: String::new(),
            port: String::new(),
            domains: String::new(),
            ports: Vec::new(),
            custom_ports: String::new(),
            health_check_path: String::new(),
            volumes: String::new(),
            restart: RestartPolicy::default(),
            memory_mb: String::new(),
            swap_mb: String::new(),
            cpus: String::new(),
            host_port: None,
            database: None,
            uses_database: None,
            auto_deploy: false,
            require_approval: false,
            deploy_window: String::new(),
            tag_trigger: String::new(),
            store_app_id: None,
            targets: Vec::new(),
            load_balancing: String::new(),
            alert_cpu_percent: String::new(),
            alert_memory_mb: String::new(),
            sticky_enabled: false,
            sticky_name: String::new(),
            sticky_max_age: "0".to_string(),
            git_config_path: String::new(),
            git_config_repo: String::new(),
            git_config_branch: String::new(),
            env_groups: Vec::new(),
            preview_enabled: false,
            preview_domain_template: String::new(),
            buildkit_enabled: false,
            buildkit_remote_addr: String::new(),
            buildkit_cache_cargo: false,
            buildkit_cache_npm: false,
            buildkit_cache_go: false,
            buildkit_cache_pip: false,
            buildkit_custom_cache: String::new(),
            buildkit_cache_from: String::new(),
            buildkit_cache_to: String::new(),
            bot_blocker: false,
            grpc_web: false,
            project_id: None,
            environment_id: None,
        }
    }
}

impl AppForm {
    /// A form filled from an app store entry. The name starts as the entry id; the user
    /// changes it before the first save.
    pub fn from_store(app: &StoreApp) -> Self {
        if app.is_compose() {
            return Self {
                name: app.id.clone(),
                kind: SourceKind::Compose,
                repository: app.repository.clone().unwrap_or_default(),
                branch: app
                    .branch
                    .clone()
                    .unwrap_or_else(|| GitRef::main().to_string()),
                compose_file: app.file.clone().unwrap_or_default(),
                service: app.service.clone().unwrap_or_default(),
                port: app.port.to_string(),
                store_app_id: Some(app.id.clone()),
                ..Self::default()
            };
        }
        Self {
            name: app.id.clone(),
            kind: SourceKind::Image,
            image: app.image.clone().unwrap_or_default(),
            port: app.port.to_string(),
            health_check_path: app.health_check_path.clone().unwrap_or_default(),
            store_app_id: Some(app.id.clone()),
            ..Self::default()
        }
    }

    pub fn edit(app: &AppEntry) -> Self {
        let spec = &app.spec;
        let form = Self {
            name: app.name.to_string(),
            target: app.target.to_string(),
            port: spec.port.to_string(),
            domains: lines(spec.domains.iter()),
            ports: spec.ports.clone(),
            custom_ports: String::new(),
            health_check_path: spec.health_check_path.clone().unwrap_or_default(),
            volumes: lines(spec.volumes.iter().map(mount_line)),
            restart: spec.restart,
            memory_mb: optional(spec.limits.memory_bytes.map(|bytes| bytes / BYTES_PER_MB)),
            swap_mb: optional(
                match (spec.limits.memory_bytes, spec.limits.memory_swap_bytes) {
                    (Some(mem), Some(swap)) if swap >= mem as i64 => {
                        Some((swap as u64 - mem) / BYTES_PER_MB)
                    }
                    _ => None,
                },
            ),
            cpus: optional(
                spec.limits
                    .nano_cpus
                    .map(|nano| nano as f64 / NANO_CPUS_PER_CPU),
            ),
            host_port: spec.host_port,
            database: spec.database.clone(),
            uses_database: spec.uses_database,
            auto_deploy: spec.auto_deploy,
            require_approval: spec.require_approval,
            deploy_window: spec.deploy_window.clone().unwrap_or_default(),
            tag_trigger: spec.tag_trigger.clone().unwrap_or_default(),
            store_app_id: spec.store_app_id.clone(),
            targets: spec.targets.iter().map(ToString::to_string).collect(),
            load_balancing: spec
                .load_balancing
                .map(|lb| lb.as_str().to_string())
                .unwrap_or_default(),
            alert_cpu_percent: spec
                .alert_rules
                .as_ref()
                .and_then(|r| r.cpu_percent)
                .map(|v| v.to_string())
                .unwrap_or_default(),
            alert_memory_mb: spec
                .alert_rules
                .as_ref()
                .and_then(|r| r.memory_bytes)
                .map(|b| (b / BYTES_PER_MB).to_string())
                .unwrap_or_default(),
            sticky_enabled: spec.sticky.as_ref().is_some_and(|s| s.enabled),
            sticky_name: spec
                .sticky
                .as_ref()
                .map(|s| s.name.clone())
                .unwrap_or_default(),
            sticky_max_age: spec
                .sticky
                .as_ref()
                .and_then(|s| s.max_age)
                .map(|d| d.as_secs().to_string())
                .unwrap_or_else(|| "0".into()),
            git_config_path: spec
                .git_config
                .as_ref()
                .map(|c| c.path.to_string())
                .unwrap_or_default(),
            git_config_repo: spec
                .git_config
                .as_ref()
                .and_then(|c| c.repository.as_ref().map(ToString::to_string))
                .unwrap_or_default(),
            git_config_branch: spec
                .git_config
                .as_ref()
                .and_then(|c| c.branch.as_ref().map(ToString::to_string))
                .unwrap_or_default(),
            env_groups: spec.env_groups.iter().map(ToString::to_string).collect(),
            preview_enabled: spec.preview.as_ref().is_some_and(|p| p.enabled),
            preview_domain_template: spec
                .preview
                .as_ref()
                .and_then(|p| p.domain_template.clone())
                .unwrap_or_default(),
            buildkit_enabled: spec.buildkit.as_ref().is_some_and(|b| b.enabled),
            buildkit_remote_addr: spec
                .buildkit
                .as_ref()
                .and_then(|b| b.remote_addr.clone())
                .unwrap_or_default(),
            buildkit_cache_cargo: spec
                .buildkit
                .as_ref()
                .is_some_and(|b| b.cache_mounts.contains(&CacheMountPreset::Cargo)),
            buildkit_cache_npm: spec
                .buildkit
                .as_ref()
                .is_some_and(|b| b.cache_mounts.contains(&CacheMountPreset::Npm)),
            buildkit_cache_go: spec
                .buildkit
                .as_ref()
                .is_some_and(|b| b.cache_mounts.contains(&CacheMountPreset::Go)),
            buildkit_cache_pip: spec
                .buildkit
                .as_ref()
                .is_some_and(|b| b.cache_mounts.contains(&CacheMountPreset::Pip)),
            buildkit_custom_cache: spec
                .buildkit
                .as_ref()
                .map(|b| b.custom_cache_paths.join("\n"))
                .unwrap_or_default(),
            buildkit_cache_from: spec
                .buildkit
                .as_ref()
                .and_then(|b| b.cache_from.clone())
                .unwrap_or_default(),
            buildkit_cache_to: spec
                .buildkit
                .as_ref()
                .and_then(|b| b.cache_to.clone())
                .unwrap_or_default(),
            bot_blocker: spec.bot_blocker,
            grpc_web: spec.grpc_web,
            project_id: spec.project_id.map(|id| id.to_string()),
            environment_id: spec.environment_id.map(|id| id.to_string()),
            ..Self::default()
        };
        form.with_source(&spec.source)
    }

    fn with_source(self, source: &AppSource) -> Self {
        match source {
            AppSource::Image { image } => Self {
                kind: SourceKind::Image,
                image: image.to_string(),
                ..self
            },
            AppSource::Git {
                repository,
                branch,
                context,
                dockerfile,
                connection,
            } => Self {
                kind: SourceKind::Git,
                connection: optional(connection.as_ref()),
                repository: repository.to_string(),
                branch: branch.to_string(),
                context: context.to_string(),
                dockerfile: dockerfile.to_string(),
                ..self
            },
            AppSource::Compose {
                repository,
                branch,
                file,
                service,
                connection,
            } => Self {
                kind: SourceKind::Compose,
                connection: optional(connection.as_ref()),
                repository: repository.to_string(),
                branch: branch.to_string(),
                compose_file: optional(file.as_ref()),
                service: service.to_string(),
                ..self
            },
        }
    }

    /// Whether the source has a Git repository, so the app can use a Git token.
    pub fn uses_git(&self) -> bool {
        self.kind != SourceKind::Image
    }

    /// Checks the form and returns the request. The error is a message in the selected language.
    pub fn parse(&self, locale: Locale) -> Result<AppRequest, String> {
        let spec = self.build_spec(locale)?;
        spec.validate().map_err(|err| locale.error_message(&err))?;
        Ok(AppRequest {
            name: parse(locale, self.name.trim())?,
            target: parse_target(locale, &self.target)?,
            spec,
        })
    }

    fn build_spec(&self, locale: Locale) -> Result<AppSpec, String> {
        let (volumes, restart, limits) = self.container_settings(locale)?;
        let (targets, load_balancing) = self.parse_replica_settings(locale)?;
        let alert_rules = self.parse_alert_rules()?;
        Ok(AppSpec {
            source: self.source(locale)?,
            port: self.port(locale)?,
            domains: parse_lines(locale, &self.domains, parse)?,
            ports: self.spec_ports(locale)?,
            health_check_path: non_empty(&self.health_check_path).map(str::to_string),
            volumes,
            restart,
            limits,
            host_port: self.host_port,
            database: self.database.clone(),
            uses_database: self.uses_database,
            replica_of: None,
            auto_deploy: self.auto_deploy,
            require_approval: self.require_approval,
            deploy_window: non_empty(&self.deploy_window).map(str::to_string),
            tag_trigger: non_empty(&self.tag_trigger).map(str::to_string),
            store_app_id: self.store_app_id.clone(),
            targets,
            load_balancing,
            alert_rules,
            sticky: self.parse_sticky(),
            git_config: self.parse_git_config(locale)?,
            env_groups: self
                .env_groups
                .iter()
                .filter_map(|id| id.parse().ok())
                .collect(),
            preview: self.parse_preview(),
            preview_of: None,
            buildkit: self.parse_buildkit(),
            bot_blocker: self.bot_blocker,
            grpc_web: self.grpc_web,
            project_id: self.project_id.as_deref().and_then(|id| id.parse().ok()),
            environment_id: self
                .environment_id
                .as_deref()
                .and_then(|id| id.parse().ok()),
        })
    }

    fn parse_buildkit(&self) -> Option<BuildKitConfig> {
        if !self.buildkit_enabled {
            return None;
        }
        let mut presets = Vec::new();
        if self.buildkit_cache_cargo {
            presets.push(CacheMountPreset::Cargo);
        }
        if self.buildkit_cache_npm {
            presets.push(CacheMountPreset::Npm);
        }
        if self.buildkit_cache_go {
            presets.push(CacheMountPreset::Go);
        }
        if self.buildkit_cache_pip {
            presets.push(CacheMountPreset::Pip);
        }
        let custom_cache_paths = self
            .buildkit_custom_cache
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        Some(BuildKitConfig {
            enabled: true,
            remote_addr: non_empty(&self.buildkit_remote_addr).map(str::to_string),
            cache_mounts: presets,
            custom_cache_paths,
            cache_from: non_empty(&self.buildkit_cache_from).map(str::to_string),
            cache_to: non_empty(&self.buildkit_cache_to).map(str::to_string),
        })
    }

    fn parse_preview(&self) -> Option<PreviewConfig> {
        if !self.preview_enabled {
            return None;
        }
        Some(PreviewConfig {
            enabled: true,
            domain_template: non_empty(&self.preview_domain_template).map(str::to_string),
        })
    }

    fn parse_git_config(&self, locale: Locale) -> Result<Option<GitConfigFile>, String> {
        let path_str = self.git_config_path.trim();
        if path_str.is_empty() {
            return Ok(None);
        }
        let path = path_str
            .parse::<RelPath>()
            .map_err(|err| locale.error_message(&err))?;
        let repository = if !self.git_config_repo.trim().is_empty() {
            Some(
                self.git_config_repo
                    .trim()
                    .parse::<RepoUrl>()
                    .map_err(|err| locale.error_message(&err))?,
            )
        } else {
            None
        };
        let branch = if !self.git_config_branch.trim().is_empty() {
            Some(
                self.git_config_branch
                    .trim()
                    .parse::<GitRef>()
                    .map_err(|err| locale.error_message(&err))?,
            )
        } else {
            None
        };
        Ok(Some(GitConfigFile {
            repository,
            branch,
            path,
        }))
    }

    fn parse_sticky(&self) -> Option<StickyCookie> {
        if !self.sticky_enabled {
            return None;
        }
        let max_age_secs = self.sticky_max_age.trim().parse::<u64>().unwrap_or(0);
        let max_age = if max_age_secs > 0 {
            Some(Duration::from_secs(max_age_secs))
        } else {
            None
        };
        Some(StickyCookie {
            enabled: true,
            name: if self.sticky_name.trim().is_empty() {
                DEFAULT_STICKY_COOKIE_NAME.to_string()
            } else {
                self.sticky_name.trim().to_string()
            },
            max_age,
        })
    }

    fn parse_alert_rules(&self) -> Result<Option<r3v3rs3_api::platform::AlertRules>, String> {
        let cpu = non_empty(&self.alert_cpu_percent)
            .map(|s| {
                s.trim()
                    .parse::<u32>()
                    .map_err(|_| "CPU threshold must be a number".to_string())
            })
            .transpose()?;
        let mem = non_empty(&self.alert_memory_mb)
            .map(|s| {
                s.trim()
                    .parse::<u64>()
                    .map(|mb| mb * BYTES_PER_MB)
                    .map_err(|_| "Memory threshold must be a number".to_string())
            })
            .transpose()?;
        if cpu.is_some() || mem.is_some() {
            Ok(Some(r3v3rs3_api::platform::AlertRules {
                cpu_percent: cpu,
                memory_bytes: mem,
                memory_percent: None,
            }))
        } else {
            Ok(None)
        }
    }

    fn parse_replica_settings(
        &self,
        locale: Locale,
    ) -> Result<(Vec<ShortId>, Option<r3v3rs3_api::upstream::LoadBalancing>), String> {
        let targets = self
            .targets
            .iter()
            .map(|t| parse_target(locale, t))
            .collect::<Result<Vec<_>, _>>()?;
        let load_balancing = match self.load_balancing.as_str() {
            "round_robin" => Some(r3v3rs3_api::upstream::LoadBalancing::RoundRobin),
            "least_conn" => Some(r3v3rs3_api::upstream::LoadBalancing::LeastConn),
            "random" => Some(r3v3rs3_api::upstream::LoadBalancing::Random),
            "first" => Some(r3v3rs3_api::upstream::LoadBalancing::First),
            "client_ip_hash" => Some(r3v3rs3_api::upstream::LoadBalancing::ClientIpHash),
            _ => None,
        };
        Ok((targets, load_balancing))
    }

    fn source(&self, locale: Locale) -> Result<AppSource, String> {
        match self.kind {
            SourceKind::Image => Ok(AppSource::Image {
                image: parse(locale, self.image.trim())?,
            }),
            SourceKind::Git => Ok(AppSource::Git {
                repository: parse(locale, self.repository.trim())?,
                branch: parse(locale, self.branch.trim())?,
                context: parse(locale, self.context.trim())?,
                dockerfile: parse(locale, self.dockerfile.trim())?,
                connection: self.connection(),
            }),
            SourceKind::Compose => self.compose_source(locale),
        }
    }

    fn compose_source(&self, locale: Locale) -> Result<AppSource, String> {
        let file = non_empty(&self.compose_file)
            .map(|file| parse(locale, file))
            .transpose()?;
        Ok(AppSource::Compose {
            repository: parse(locale, self.repository.trim())?,
            branch: parse(locale, self.branch.trim())?,
            file,
            service: parse(locale, self.service.trim())?,
            connection: self.connection(),
        })
    }

    /// The selected connection. An empty value parses as the zero id, so it means none here.
    fn connection(&self) -> Option<ShortId> {
        non_empty(&self.connection).and_then(|id| id.parse().ok())
    }

    fn port(&self, locale: Locale) -> Result<u16, String> {
        match self.port.trim().parse::<u16>() {
            Ok(port) if port > 0 => Ok(port),
            _ => Err(locale.t("apps.invalid_port").to_string()),
        }
    }

    /// The checked ports plus the lines of the free text field, without repeats. An empty list
    /// means the automatic choice of the platform.
    fn spec_ports(&self, locale: Locale) -> Result<Vec<String>, String> {
        let mut ports = self.ports.clone();
        for line in parse_lines(locale, &self.custom_ports, |_, value| Ok(value.to_string()))? {
            if !ports.contains(&line) {
                ports.push(line);
            }
        }
        Ok(ports)
    }

    /// The volumes, the restart policy and the limits. A Compose file sets them itself, so a
    /// Compose app sends the defaults.
    fn container_settings(
        &self,
        locale: Locale,
    ) -> Result<(Vec<VolumeMount>, RestartPolicy, ResourceLimits), String> {
        if self.kind == SourceKind::Compose {
            return Ok(Default::default());
        }
        let volumes = parse_lines(locale, &self.volumes, parse_mount)?;
        Ok((volumes, self.restart, self.limits(locale)?))
    }

    fn limits(&self, locale: Locale) -> Result<ResourceLimits, String> {
        let memory_bytes = non_empty(&self.memory_mb)
            .map(|value| parse_memory(locale, value))
            .transpose()?;
        let nano_cpus = non_empty(&self.cpus)
            .map(|value| parse_cpus(locale, value))
            .transpose()?;
        let memory_swap_bytes = parse_swap(locale, memory_bytes, &self.swap_mb)?;
        Ok(ResourceLimits {
            memory_bytes,
            nano_cpus,
            memory_swap_bytes,
        })
    }
}

fn parse_swap(
    locale: Locale,
    memory_bytes: Option<u64>,
    swap_mb: &str,
) -> Result<Option<i64>, String> {
    let Some(swap_str) = non_empty(swap_mb) else {
        return Ok(None);
    };
    let Some(mem) = memory_bytes else {
        return Err(locale.t("apps.swap_requires_memory").to_string());
    };
    let mb: u64 = swap_str
        .parse()
        .map_err(|_| locale.t("apps.invalid_swap").to_string())?;
    Ok(Some(mem as i64 + (mb * BYTES_PER_MB) as i64))
}

fn parse<T: FromStr<Err = Error>>(locale: Locale, value: &str) -> Result<T, String> {
    value.parse().map_err(|err| locale.error_message(&err))
}

fn parse_target(locale: Locale, value: &str) -> Result<ShortId, String> {
    // An empty value parses as the zero id, so it is an error here.
    match value.parse::<ShortId>() {
        Ok(id) if !value.is_empty() => Ok(id),
        _ => Err(locale.t("apps.invalid_target").to_string()),
    }
}

fn parse_mount(locale: Locale, line: &str) -> Result<VolumeMount, String> {
    let invalid = || locale.tf("apps.invalid_volume", &[("volume", line)]);
    let mut parts = line.split(':');
    let (Some(volume), Some(target)) = (parts.next(), parts.next()) else {
        return Err(invalid());
    };
    let read_only = match parts.next() {
        None => false,
        Some("ro") => true,
        Some(_) => return Err(invalid()),
    };
    if parts.next().is_some() {
        return Err(invalid());
    }
    Ok(VolumeMount {
        volume: parse(locale, volume)?,
        target: parse(locale, target)?,
        read_only,
    })
}

fn parse_memory(locale: Locale, value: &str) -> Result<u64, String> {
    match value.parse::<u64>() {
        Ok(mb) if mb > 0 => mb
            .checked_mul(BYTES_PER_MB)
            .ok_or_else(|| locale.t("apps.invalid_memory").to_string()),
        _ => Err(locale.t("apps.invalid_memory").to_string()),
    }
}

fn parse_cpus(locale: Locale, value: &str) -> Result<u64, String> {
    match value.parse::<f64>() {
        Ok(cpus) if cpus.is_finite() && cpus > 0.0 && cpus <= 1024.0 => {
            Ok((cpus * NANO_CPUS_PER_CPU).round() as u64)
        }
        _ => Err(locale.t("apps.invalid_cpus").to_string()),
    }
}

/// Parses every line that is not blank.
fn parse_lines<T>(
    locale: Locale,
    text: &str,
    parse_line: fn(Locale, &str) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| parse_line(locale, line))
        .collect()
}

fn non_empty(value: &str) -> Option<&str> {
    Some(value.trim()).filter(|value| !value.is_empty())
}

fn optional(value: Option<impl ToString>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn lines<T: ToString>(items: impl Iterator<Item = T>) -> String {
    items
        .map(|item| item.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn mount_line(mount: &VolumeMount) -> String {
    let suffix = if mount.read_only { ":ro" } else { "" };
    format!("{}:{}{suffix}", mount.volume, mount.target.as_str())
}

/// The options and lists available for selection in the app form.
pub struct FormOptions<'a> {
    pub targets: &'a [TargetEntry],
    pub connections: &'a [GitConnectionEntry],
    pub ports: &'a [PortEntry],
    pub databases: &'a [(ShortId, String)],
    pub env_groups: &'a [(ShortId, String)],
}

/// The fields of the form. `editing` locks the name of an existing app.
pub fn fields_view(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    options: FormOptions<'_>,
    editing: bool,
) -> Html {
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("common.name")}</label>
            <input type="text" autocapitalize="off" autocomplete="off" value={form.name.clone()} oninput={text_input(form, |f| &mut f.name)} disabled={editing} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t("apps.name_hint")}</p>
            { target_select(locale, form, options.targets) }
            { kind_select(locale, form) }
            { source_view(locale, form, options.connections) }
            { text_field(locale, form, "apps.port", Some("apps.port_hint"), |f| &mut f.port) }
            { text_area(locale, form, "apps.domains", "apps.domains_hint", |f| &mut f.domains) }
            { ports_view(locale, form, options.ports) }
            { text_field(locale, form, "apps.health_check_path", Some("apps.health_check_path_hint"), |f| &mut f.health_check_path) }
            { database_select(locale, form, options.databases) }
            { env_groups_view(locale, form, options.env_groups) }
            if form.kind != SourceKind::Compose {
                { container_view(locale, form) }
            }
            { git_config_view(locale, form) }
            { approval_view(locale, form) }
            { bot_blocker_view(locale, form) }
            { grpc_web_view(locale, form) }
        </>
    }
}

fn env_groups_view(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    env_groups: &[(ShortId, String)],
) -> Html {
    if env_groups.is_empty() {
        return html! {};
    }
    let checkboxes = env_groups
        .iter()
        .map(|(id, name)| {
            let id_str = id.to_string();
            let checked = form.env_groups.contains(&id_str);
            let state = form.clone();
            let id_val = id_str.clone();
            let onchange = Callback::from(move |event: Event| {
                let input: HtmlInputElement = event.target_unchecked_into();
                let mut updated = (*state).clone();
                if input.checked() {
                    if !updated.env_groups.contains(&id_val) {
                        updated.env_groups.push(id_val.clone());
                    }
                } else {
                    updated.env_groups.retain(|g| g != &id_val);
                }
                state.set(updated);
            });
            html! {
                <label class="flex items-center gap-2 text-sm text-neutral-800 dark:text-neutral-200 cursor-pointer">
                    <input type="checkbox" value={id_str} checked={checked} {onchange} class="w-4 h-4 rounded text-blue-600" />
                    <span>{name.clone()}</span>
                </label>
            }
        })
        .collect::<Html>();
    html! {
        <div class="mt-4 pt-4 border-t border-neutral-200 dark:border-neutral-700">
            <label class={LABEL_CLASS}>{locale.t("apps.env_groups")}</label>
            <p class={HINT_CLASS}>{locale.t("apps.env_groups_hint")}</p>
            <div class="mt-2 grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 gap-2">
                { checkboxes }
            </div>
        </div>
    }
}

fn git_config_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let path_input = text_input(form, |f| &mut f.git_config_path);
    let repo_input = text_input(form, |f| &mut f.git_config_repo);
    let branch_input = text_input(form, |f| &mut f.git_config_branch);

    html! {
        <div class="mt-4 pt-4 border-t border-neutral-200 dark:border-neutral-700">
            <h4 class="text-sm font-semibold text-neutral-800 dark:text-neutral-200 mb-2">
                {locale.t("apps.git_config_title")}
            </h4>
            <div class="space-y-3">
                <div>
                    <label class={LABEL_CLASS}>{locale.t("apps.git_config_path")}</label>
                    <input type="text" autocapitalize="off" autocomplete="off" value={form.git_config_path.clone()} oninput={path_input} placeholder=".env" class={INPUT_CLASS} />
                    <p class={HINT_CLASS}>{locale.t("apps.git_config_path_hint")}</p>
                </div>
                if !form.git_config_path.is_empty() || form.kind == SourceKind::Image {
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-3">
                        <div>
                            <label class={LABEL_CLASS}>{locale.t("apps.git_config_repo")}</label>
                            <input type="text" autocapitalize="off" autocomplete="off" value={form.git_config_repo.clone()} oninput={repo_input} placeholder="https://github.com/org/config-repo.git" class={INPUT_CLASS} />
                            <p class={HINT_CLASS}>{locale.t("apps.git_config_repo_hint")}</p>
                        </div>
                        <div>
                            <label class={LABEL_CLASS}>{locale.t("apps.git_config_branch")}</label>
                            <input type="text" autocapitalize="off" autocomplete="off" value={form.git_config_branch.clone()} oninput={branch_input} placeholder="main" class={INPUT_CLASS} />
                            <p class={HINT_CLASS}>{locale.t("apps.git_config_branch_hint")}</p>
                        </div>
                    </div>
                }
            </div>
        </div>
    }
}

fn approval_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let form_handle = form.clone();
    let checked = form.require_approval;
    let onchange = Callback::from(move |e: Event| {
        if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
            let mut updated = (*form_handle).clone();
            updated.require_approval = target.checked();
            form_handle.set(updated);
        }
    });
    html! {
        <div class="mt-4">
            <label class="flex items-center gap-2 cursor-pointer">
                <input type="checkbox" checked={checked} {onchange} class="w-4 h-4 text-blue-600 bg-neutral-100 border-neutral-300 rounded focus:ring-blue-500 dark:focus:ring-blue-600 dark:ring-offset-neutral-800 focus:ring-2 dark:bg-neutral-700 dark:border-neutral-600" />
                <span class={LABEL_CLASS}>{locale.t("apps.require_approval")}</span>
            </label>
            <p class={HINT_CLASS}>{locale.t("apps.require_approval_hint")}</p>
            { text_field(locale, form, "apps.deploy_window", Some("apps.deploy_window_hint"), |f| &mut f.deploy_window) }
        </div>
    }
}

fn bot_blocker_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let form_handle = form.clone();
    let checked = form.bot_blocker;
    let onchange = Callback::from(move |e: Event| {
        if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
            let mut updated = (*form_handle).clone();
            updated.bot_blocker = target.checked();
            form_handle.set(updated);
        }
    });
    html! {
        <div class="mt-4">
            <label class="flex items-center gap-2 cursor-pointer">
                <input type="checkbox" checked={checked} {onchange} class="w-4 h-4 text-blue-600 bg-neutral-100 border-neutral-300 rounded focus:ring-blue-500 dark:focus:ring-blue-600 dark:ring-offset-neutral-800 focus:ring-2 dark:bg-neutral-700 dark:border-neutral-600" />
                <span class={LABEL_CLASS}>{locale.t("apps.bot_blocker")}</span>
            </label>
            <p class={HINT_CLASS}>{locale.t("apps.bot_blocker_hint")}</p>
        </div>
    }
}

fn grpc_web_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let form_handle = form.clone();
    let checked = form.grpc_web;
    let onchange = Callback::from(move |e: Event| {
        if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
            let mut updated = (*form_handle).clone();
            updated.grpc_web = target.checked();
            form_handle.set(updated);
        }
    });
    html! {
        <div class="mt-4">
            <label class="flex items-center gap-2 cursor-pointer">
                <input type="checkbox" checked={checked} {onchange} class="w-4 h-4 text-blue-600 bg-neutral-100 border-neutral-300 rounded focus:ring-blue-500 dark:focus:ring-blue-600 dark:ring-offset-neutral-800 focus:ring-2 dark:bg-neutral-700 dark:border-neutral-600" />
                <span class={LABEL_CLASS}>{locale.t("apps.grpc_web")}</span>
            </label>
            <p class={HINT_CLASS}>{locale.t("apps.grpc_web_hint")}</p>
        </div>
    }
}

/// The select of the database that the app uses. The deployment injects its connection.
fn database_select(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    databases: &[(ShortId, String)],
) -> Html {
    let options = databases
        .iter()
        .map(|(id, name)| {
            let value = id.to_string();
            let selected = form.uses_database == Some(*id);
            html! { <option {value} {selected}>{name.clone()}</option> }
        })
        .collect::<Html>();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.database")}</label>
            <select
                class={INPUT_CLASS}
                value={form.uses_database.map(|id| id.to_string()).unwrap_or_default()}
                onchange={{
                    let form = form.clone();
                    Callback::from(move |event: Event| {
                        let select: HtmlSelectElement = event.target_unchecked_into();
                        let mut updated = (*form).clone();
                        updated.uses_database = select.value().parse().ok();
                        form.set(updated);
                    })
                }}
            >
                <option value="" selected={form.uses_database.is_none()}>
                    {locale.t("apps.database_none")}
                </option>
                { options }
            </select>
            <p class={HINT_CLASS}>{locale.t("apps.database_hint")}</p>
        </>
    }
}

/// The checkbox list of the ports and the free text field for the ones beyond the list. An
/// unchecked list means the automatic choice of the platform.
fn ports_view(locale: Locale, form: &UseStateHandle<AppForm>, ports: &[PortEntry]) -> Html {
    let checkboxes = ports
        .iter()
        .map(|entry| {
            let id = entry.id.to_string();
            let checked = form.ports.contains(&id) || form.ports.contains(&entry.port.name);
            let state = form.clone();
            let id_value = id.clone();
            let onchange = Callback::from(move |event: Event| {
                let input: HtmlInputElement = event.target_unchecked_into();
                let mut updated = (*state).clone();
                if input.checked() {
                    if !updated.ports.contains(&id_value) {
                        updated.ports.push(id_value.clone());
                    }
                } else {
                    updated.ports.retain(|port| port != &id_value);
                }
                state.set(updated);
            });
            html! {
                <label class="flex items-center gap-2">
                    <input type="checkbox" value={id} checked={checked} {onchange} />
                    <span>{port_label(entry)}</span>
                </label>
            }
        })
        .collect::<Html>();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.ports")}</label>
            if ports.is_empty() {
                <p class={HINT_CLASS}>{locale.t("apps.ports_empty")}</p>
            } else {
                <div class="flex flex-col gap-1">{checkboxes}</div>
            }
            <p class={HINT_CLASS}>{locale.t("apps.ports_hint")}</p>
            { text_area(locale, form, "apps.custom_ports", "apps.custom_ports_hint", |f| &mut f.custom_ports) }
        </>
    }
}

fn port_label(entry: &PortEntry) -> String {
    let protocol = entry
        .port
        .listen
        .to_string()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string();
    if entry.port.name.is_empty() {
        protocol
    } else {
        format!("{} ({protocol})", entry.port.name)
    }
}

fn source_view(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    connections: &[GitConnectionEntry],
) -> Html {
    match form.kind {
        SourceKind::Image => html! {
            <>
                { text_field(locale, form, "apps.image", Some("apps.image_hint"), |f| {
                    &mut f.image
                }) }
                { auto_deploy_toggle(locale, form) }
            </>
        },
        SourceKind::Git => html! {
            <>
                { repository_view(locale, form, connections) }
                { text_field(locale, form, "apps.tag_trigger", Some("apps.tag_trigger_hint"), |f| &mut f.tag_trigger) }
                { preview_view(locale, form) }
                { buildkit_view(locale, form) }
                { text_field(locale, form, "apps.context", Some("apps.context_hint"), |f| &mut f.context) }
                { text_field(locale, form, "apps.dockerfile", Some("apps.dockerfile_hint"), |f| &mut f.dockerfile) }
            </>
        },
        SourceKind::Compose => html! {
            <>
                { repository_view(locale, form, connections) }
                { text_field(locale, form, "apps.tag_trigger", Some("apps.tag_trigger_hint"), |f| &mut f.tag_trigger) }
                { preview_view(locale, form) }
                { text_field(locale, form, "apps.compose_file", Some("apps.compose_file_hint"), |f| &mut f.compose_file) }
                { text_field(locale, form, "apps.service", Some("apps.service_hint"), |f| &mut f.service) }
            </>
        },
    }
}

fn preview_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let form_handle = form.clone();
    let checked = form.preview_enabled;
    let onchange = Callback::from(move |e: Event| {
        if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
            let mut updated = (*form_handle).clone();
            updated.preview_enabled = target.checked();
            form_handle.set(updated);
        }
    });
    html! {
        <div class="mt-4 p-3 bg-neutral-50 dark:bg-neutral-800/40 rounded-lg border border-neutral-200 dark:border-neutral-700">
            <label class="flex items-center gap-2 cursor-pointer">
                <input type="checkbox" checked={checked} {onchange} class="w-4 h-4 text-blue-600 bg-neutral-100 border-neutral-300 rounded focus:ring-blue-500 dark:focus:ring-blue-600 dark:ring-offset-neutral-800 focus:ring-2 dark:bg-neutral-700 dark:border-neutral-600" />
                <span class={LABEL_CLASS}>{locale.t("apps.preview_enabled")}</span>
            </label>
            <p class={HINT_CLASS}>{locale.t("apps.preview_enabled_hint")}</p>
            if checked {
                <div class="mt-3">
                    { text_field(locale, form, "apps.preview_domain_template", Some("apps.preview_domain_template_hint"), |f| &mut f.preview_domain_template) }
                </div>
            }
        </div>
    }
}

fn buildkit_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let form_handle = form.clone();
    let checked = form.buildkit_enabled;
    let onchange = Callback::from(move |e: Event| {
        if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
            let mut updated = (*form_handle).clone();
            updated.buildkit_enabled = target.checked();
            form_handle.set(updated);
        }
    });
    let on_cargo = {
        let form = form.clone();
        Callback::from(move |e: Event| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut u = (*form).clone();
                u.buildkit_cache_cargo = target.checked();
                form.set(u);
            }
        })
    };
    let on_npm = {
        let form = form.clone();
        Callback::from(move |e: Event| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut u = (*form).clone();
                u.buildkit_cache_npm = target.checked();
                form.set(u);
            }
        })
    };
    let on_go = {
        let form = form.clone();
        Callback::from(move |e: Event| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut u = (*form).clone();
                u.buildkit_cache_go = target.checked();
                form.set(u);
            }
        })
    };
    let on_pip = {
        let form = form.clone();
        Callback::from(move |e: Event| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                let mut u = (*form).clone();
                u.buildkit_cache_pip = target.checked();
                form.set(u);
            }
        })
    };
    html! {
        <div class="mt-4 rounded border border-neutral-200 dark:border-neutral-700 p-4">
            <label class="flex items-center gap-2 cursor-pointer">
                <input type="checkbox" {checked} {onchange} class="rounded border-neutral-300 text-neutral-900 focus:ring-0 dark:border-neutral-600 dark:bg-neutral-800" />
                <span class={LABEL_CLASS}>{locale.t("apps.buildkit_enabled")}</span>
            </label>
            <p class={HINT_CLASS}>{locale.t("apps.buildkit_enabled_hint")}</p>
            if checked {
                <div class="mt-3 space-y-3">
                    { text_field(locale, form, "apps.buildkit_remote_addr", Some("apps.buildkit_remote_addr_hint"), |f| &mut f.buildkit_remote_addr) }
                    <div>
                        <span class={LABEL_CLASS}>{locale.t("apps.buildkit_cache_presets")}</span>
                        <div class="mt-2 flex flex-wrap gap-4 text-sm">
                            <label class="flex items-center gap-1.5 cursor-pointer">
                                <input type="checkbox" checked={form.buildkit_cache_cargo} onchange={on_cargo} />
                                <span>{"Rust (Cargo)"}</span>
                            </label>
                            <label class="flex items-center gap-1.5 cursor-pointer">
                                <input type="checkbox" checked={form.buildkit_cache_npm} onchange={on_npm} />
                                <span>{"Node.js (NPM)"}</span>
                            </label>
                            <label class="flex items-center gap-1.5 cursor-pointer">
                                <input type="checkbox" checked={form.buildkit_cache_go} onchange={on_go} />
                                <span>{"Go"}</span>
                            </label>
                            <label class="flex items-center gap-1.5 cursor-pointer">
                                <input type="checkbox" checked={form.buildkit_cache_pip} onchange={on_pip} />
                                <span>{"Python (Pip)"}</span>
                            </label>
                        </div>
                    </div>
                    { text_area(locale, form, "apps.buildkit_custom_cache", "apps.buildkit_custom_cache_hint", |f| &mut f.buildkit_custom_cache) }
                    { text_field(locale, form, "apps.buildkit_cache_from", Some("apps.buildkit_cache_from_hint"), |f| &mut f.buildkit_cache_from) }
                    { text_field(locale, form, "apps.buildkit_cache_to", Some("apps.buildkit_cache_to_hint"), |f| &mut f.buildkit_cache_to) }
                </div>
            }
        </div>
    }
}

fn auto_deploy_toggle(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let form_handle = form.clone();
    let checked = form.auto_deploy;
    let onchange = Callback::from(move |e: Event| {
        if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
            let mut updated = (*form_handle).clone();
            updated.auto_deploy = target.checked();
            form_handle.set(updated);
        }
    });
    html! {
        <div class="mt-4">
            <label class="flex items-center gap-2 cursor-pointer">
                <input type="checkbox" checked={checked} {onchange} class="w-4 h-4 text-blue-600 bg-neutral-100 border-neutral-300 rounded focus:ring-blue-500 dark:focus:ring-blue-600 dark:ring-offset-neutral-800 focus:ring-2 dark:bg-neutral-700 dark:border-neutral-600" />
                <span class={LABEL_CLASS}>{locale.t("apps.auto_deploy")}</span>
            </label>
            <p class={HINT_CLASS}>{locale.t("apps.auto_deploy_hint")}</p>
        </div>
    }
}

fn repository_view(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    connections: &[GitConnectionEntry],
) -> Html {
    html! { <RepositoryPicker form={form.clone()} connections={connections.to_vec()} {locale} /> }
}

/// The volumes, the restart policy and the limits. A Compose app sets them in its Compose file.
fn container_view(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    html! {
        <>
            { text_area(locale, form, "apps.volumes", "apps.volumes_hint", |f| &mut f.volumes) }
            { restart_select(locale, form) }
            { text_field(locale, form, "apps.memory", Some("apps.memory_hint"), |f| &mut f.memory_mb) }
            { text_field(locale, form, "apps.swap", Some("apps.swap_hint"), |f| &mut f.swap_mb) }
            { text_field(locale, form, "apps.cpus", Some("apps.cpus_hint"), |f| &mut f.cpus) }
            { text_field(locale, form, "apps.alert_cpu", Some("apps.alert_cpu_hint"), |f| &mut f.alert_cpu_percent) }
            { text_field(locale, form, "apps.alert_memory", Some("apps.alert_memory_hint"), |f| &mut f.alert_memory_mb) }
        </>
    }
}

pub(super) fn text_field(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    label: &'static str,
    hint: Option<&'static str>,
    select: fn(&mut AppForm) -> &mut String,
) -> Html {
    let mut current = (**form).clone();
    let value = select(&mut current).clone();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t(label)}</label>
            <input type="text" autocapitalize="off" autocomplete="off" {value} oninput={text_input(form, select)} class={INPUT_CLASS} />
            if let Some(hint) = hint {
                <p class={HINT_CLASS}>{locale.t(hint)}</p>
            }
        </>
    }
}

fn text_area(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    label: &'static str,
    hint: &'static str,
    select: fn(&mut AppForm) -> &mut String,
) -> Html {
    let mut current = (**form).clone();
    let value = select(&mut current).clone();
    let state = form.clone();
    let oninput = Callback::from(move |event: InputEvent| {
        let area: HtmlTextAreaElement = event.target_unchecked_into();
        let mut updated = (*state).clone();
        *select(&mut updated) = area.value();
        state.set(updated);
    });
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t(label)}</label>
            <textarea rows="3" autocapitalize="off" {value} {oninput} class={INPUT_CLASS} />
            <p class={HINT_CLASS}>{locale.t(hint)}</p>
        </>
    }
}

/// A select whose change writes the chosen option into the form.
fn select_view(
    form: &UseStateHandle<AppForm>,
    options: Html,
    apply: fn(&mut AppForm, &str),
) -> Html {
    let state = form.clone();
    let onchange = Callback::from(move |event: Event| {
        let select: HtmlSelectElement = event.target_unchecked_into();
        let mut updated = (*state).clone();
        apply(&mut updated, &select.value());
        state.set(updated);
    });
    html! { <select {onchange} class={INPUT_CLASS}>{options}</select> }
}

fn kind_select(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let options = KINDS
        .iter()
        .map(|(kind, value, key)| {
            html! { <option value={*value} selected={*kind == form.kind}>{locale.t(key)}</option> }
        })
        .collect::<Html>();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.source")}</label>
            { select_view(form, options, |f, value| f.kind = parse_kind(value)) }
        </>
    }
}

fn restart_select(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let options = RESTARTS
        .iter()
        .map(|(restart, value, key)| {
            html! { <option value={*value} selected={*restart == form.restart}>{locale.t(key)}</option> }
        })
        .collect::<Html>();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.restart")}</label>
            { select_view(form, options, |f, value| f.restart = parse_restart(value)) }
        </>
    }
}

fn target_select(locale: Locale, form: &UseStateHandle<AppForm>, targets: &[TargetEntry]) -> Html {
    let options = targets
        .iter()
        .map(|target| {
            let id = target.id.to_string();
            let (status, _) = super::targets::target_status(target);
            let label = format!("{} ({})", target.name, locale.t(status));
            html! { <option value={id.clone()} selected={id == form.target}>{label}</option> }
        })
        .collect::<Html>();
    html! {
        <>
            <label class={LABEL_CLASS}>{locale.t("apps.target")}</label>
            { select_view(form, options, |f, value| f.target = value.to_string()) }
            { replica_targets_view(locale, form, targets) }
        </>
    }
}

fn replica_targets_view(
    locale: Locale,
    form: &UseStateHandle<AppForm>,
    targets: &[TargetEntry],
) -> Html {
    if targets.len() <= 1 {
        return html! {};
    }
    let checkboxes = targets
        .iter()
        .map(|target| {
            let id = target.id.to_string();
            let checked = form.targets.contains(&id);
            let state = form.clone();
            let id_value = id.clone();
            let (status, _) = super::targets::target_status(target);
            let label = format!("{} ({})", target.name, locale.t(status));
            let onchange = Callback::from(move |event: Event| {
                let input: HtmlInputElement = event.target_unchecked_into();
                let mut updated = (*state).clone();
                if input.checked() {
                    if !updated.targets.contains(&id_value) {
                        updated.targets.push(id_value.clone());
                    }
                } else {
                    updated.targets.retain(|t| t != &id_value);
                }
                state.set(updated);
            });
            html! {
                <label class="flex items-center gap-2">
                    <input type="checkbox" value={id} checked={checked} {onchange} />
                    <span>{label}</span>
                </label>
            }
        })
        .collect::<Html>();
    html! {
        <div class="mt-4">
            <label class={LABEL_CLASS}>{locale.t("apps.replica_targets")}</label>
            <div class="flex flex-col gap-1">{checkboxes}</div>
            <p class={HINT_CLASS}>{locale.t("apps.replica_targets_hint")}</p>
            if form.targets.len() > 1 {
                { load_balancing_select(locale, form) }
                { sticky_session_select(locale, form) }
            }
        </div>
    }
}

fn sticky_session_select(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let form_clone = form.clone();
    let toggle_cb = Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let mut next = (*form_clone).clone();
        next.sticky_enabled = input.checked();
        form_clone.set(next);
    });
    let form_name = form.clone();
    let name_cb = Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let mut next = (*form_name).clone();
        next.sticky_name = input.value();
        form_name.set(next);
    });
    let form_age = form.clone();
    let max_age_cb = Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let mut next = (*form_age).clone();
        next.sticky_max_age = input.value();
        form_age.set(next);
    });
    html! {
        <div class="mt-3">
            <label class="flex items-center gap-2 cursor-pointer text-sm font-medium text-neutral-800 dark:text-neutral-200">
                <input type="checkbox" checked={form.sticky_enabled} onchange={toggle_cb} />
                <span>{locale.t("http_form.enable_sticky_cookie")}</span>
            </label>
            if form.sticky_enabled {
                <div class="grid grid-cols-1 sm:grid-cols-2 gap-x-4 mt-2">
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.sticky_cookie_name")}</label>
                        <input type="text" autocapitalize="off" spellcheck="false" placeholder="r3v3rs3_affinity" value={form.sticky_name.clone()} onchange={name_cb} class={INPUT_CLASS} />
                    </div>
                    <div>
                        <label class={LABEL_CLASS}>{locale.t("http_form.sticky_max_age")}</label>
                        <input type="number" min="0" value={form.sticky_max_age.clone()} onchange={max_age_cb} class={INPUT_CLASS} />
                    </div>
                </div>
            }
        </div>
    }
}

fn load_balancing_select(locale: Locale, form: &UseStateHandle<AppForm>) -> Html {
    let options = r3v3rs3_api::upstream::LoadBalancing::ALL
        .iter()
        .map(|lb| {
            let val = lb.as_str();
            let key = format!("load_balancing.{val}");
            let label = locale.t(&key);
            let selected = form.load_balancing == val
                || (form.load_balancing.is_empty()
                    && *lb == r3v3rs3_api::upstream::LoadBalancing::RoundRobin);
            html! { <option value={val} {selected}>{label}</option> }
        })
        .collect::<Html>();
    html! {
        <div class="mt-2">
            <label class={LABEL_CLASS}>{locale.t("apps.load_balancing")}</label>
            { select_view(form, options, |f, value| f.load_balancing = value.to_string()) }
            <p class={HINT_CLASS}>{locale.t("apps.load_balancing_hint")}</p>
        </div>
    }
}

fn parse_kind(value: &str) -> SourceKind {
    KINDS
        .iter()
        .find(|(_, item, _)| *item == value)
        .map_or_else(SourceKind::default, |(kind, _, _)| *kind)
}

fn parse_restart(value: &str) -> RestartPolicy {
    RESTARTS
        .iter()
        .find(|(_, item, _)| *item == value)
        .map_or_else(RestartPolicy::default, |(restart, _, _)| *restart)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    fn image_form() -> AppForm {
        AppForm {
            name: " shop ".into(),
            image: "nginx:1.27".into(),
            port: "8080".into(),
            domains: "shop.example.com\n\n www.example.com ".into(),
            volumes: "shop-data:/data\nshop-cache:/cache:ro".into(),
            restart: RestartPolicy::Always,
            memory_mb: "512".into(),
            cpus: "1.5".into(),
            ..AppForm::default()
        }
    }

    #[test]
    fn an_image_form_makes_the_request_of_the_admin_api() {
        let request = image_form().parse(Locale::En).unwrap();
        assert_eq!(
            to_value(&request).unwrap(),
            json!({
                "name": "shop",
                "target": "local",
                "spec": {
                    "source": {"type": "image", "image": "nginx:1.27"},
                    "port": 8080,
                    "domains": ["shop.example.com", "www.example.com"],
                    "volumes": [
                        {"volume": "shop-data", "target": "/data", "read_only": false},
                        {"volume": "shop-cache", "target": "/cache", "read_only": true}
                    ],
                    "restart": "always",
                    "limits": {"memory_bytes": 536_870_912_u64, "nano_cpus": 1_500_000_000_u64},
                    "auto_deploy": false,
                    "require_approval": false
                }
            })
        );
    }

    #[test]
    fn the_port_selection_joins_the_extra_port_lines_without_repeats() {
        let form = AppForm {
            ports: vec!["p1".into(), "p2".into()],
            custom_ports: "https\np2\n\n p1 ".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        assert_eq!(request.spec.ports, ["p1", "p2", "https"]);
        let empty = AppForm {
            ports: Vec::new(),
            ..image_form()
        };
        assert!(empty.parse(Locale::En).unwrap().spec.ports.is_empty());
    }

    #[test]
    fn a_compose_form_hides_the_container_settings() {
        let form = AppForm {
            kind: SourceKind::Compose,
            repository: "https://github.com/owner/shop.git".into(),
            service: "web".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        assert!(request.spec.volumes.is_empty());
        assert_eq!(request.spec.restart, RestartPolicy::default());
        assert_eq!(request.spec.limits, ResourceLimits::default());
        let AppSource::Compose { file, branch, .. } = &request.spec.source else {
            panic!("expected a Compose source: {:?}", request.spec.source);
        };
        assert_eq!((file, branch.as_str()), (&None, "main"));
    }

    #[test]
    fn an_existing_app_fills_the_form_again() {
        let request = image_form().parse(Locale::En).unwrap();
        let app = AppEntry {
            id: "shop".parse().unwrap(),
            name: request.name.clone(),
            target: request.target,
            spec: request.spec.clone(),
            git_token_set: false,
            webhook_secret_set: false,
            hook: None,
            update: None,
            catalog_update: None,
            created_at: 0,
            updated_at: 0,
        };
        let form = AppForm::edit(&app);
        assert_eq!(form.parse(Locale::En).unwrap(), request);
        assert_eq!(form.cpus, "1.5");
        assert!(!form.uses_git());
    }

    #[test]
    fn a_store_entry_fills_the_image_form() {
        let entry = StoreApp {
            id: "uptime-kuma".into(),
            name: "Uptime Kuma".into(),
            description: "test".into(),
            description_tr: String::new(),
            category: None,
            category_tr: None,
            screenshot: None,
            image: Some("louislam/uptime-kuma:1".into()),
            port: 3001,
            health_check_path: Some("/".into()),
            website: None,
            repository: None,
            branch: None,
            file: None,
            service: None,
            env: Vec::new(),
        };
        let form = AppForm::from_store(&entry);
        let request = form.parse(Locale::En).unwrap();
        assert_eq!(request.name.as_str(), "uptime-kuma");
        assert_eq!(request.target.to_string(), "local");
        assert_eq!(
            to_value(&request.spec.source).unwrap(),
            json!({"type": "image", "image": "louislam/uptime-kuma:1"})
        );
        assert_eq!(request.spec.port, 3001);
        assert_eq!(request.spec.health_check_path.as_deref(), Some("/"));
        assert!(request.spec.domains.is_empty());
    }

    #[test]
    fn a_compose_store_entry_fills_the_compose_form() {
        let entry = StoreApp {
            id: "umami".into(),
            name: "Umami".into(),
            description: "test".into(),
            description_tr: String::new(),
            category: None,
            category_tr: None,
            screenshot: None,
            image: None,
            port: 3000,
            health_check_path: None,
            website: None,
            repository: Some("https://github.com/owner/shop.git".into()),
            branch: None,
            file: Some("app-store-compose/umami/compose.yaml".into()),
            service: Some("umami".into()),
            env: Vec::new(),
        };
        let form = AppForm::from_store(&entry);
        let request = form.parse(Locale::En).unwrap();
        assert_eq!(request.name.as_str(), "umami");
        assert_eq!(
            to_value(&request.spec.source).unwrap(),
            json!({
                "type": "compose",
                "repository": "https://github.com/owner/shop.git",
                "branch": "main",
                "file": "app-store-compose/umami/compose.yaml",
                "service": "umami"
            })
        );
        assert_eq!(request.spec.port, 3000);
    }

    #[test]
    fn invalid_values_have_translated_errors() {
        let cases = [
            (
                AppForm {
                    port: "0".into(),
                    ..image_form()
                },
                "apps.invalid_port",
            ),
            (
                AppForm {
                    memory_mb: "-1".into(),
                    ..image_form()
                },
                "apps.invalid_memory",
            ),
            (
                AppForm {
                    cpus: "0".into(),
                    ..image_form()
                },
                "apps.invalid_cpus",
            ),
            (
                AppForm {
                    target: String::new(),
                    ..image_form()
                },
                "apps.invalid_target",
            ),
        ];
        for (form, key) in cases {
            assert_eq!(form.parse(Locale::Tr), Err(Locale::Tr.t(key).to_string()));
        }
        let volume = AppForm {
            volumes: "data".into(),
            ..image_form()
        };
        assert_eq!(
            volume.parse(Locale::En),
            Err(Locale::En.tf("apps.invalid_volume", &[("volume", "data")]))
        );
        let name = AppForm {
            name: "Shop".into(),
            ..image_form()
        };
        assert!(name.parse(Locale::En).is_err());
    }

    #[test]
    fn the_selects_read_their_own_values() {
        for (kind, value, _) in KINDS {
            assert_eq!(parse_kind(value), kind);
        }
        for (restart, value, _) in RESTARTS {
            assert_eq!(parse_restart(value), restart);
        }
        assert_eq!(parse_kind("other"), SourceKind::Image);
    }

    #[test]
    fn tag_trigger_in_form() {
        let form = AppForm {
            kind: SourceKind::Git,
            repository: "https://github.com/owner/shop.git".into(),
            tag_trigger: "v*".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        assert_eq!(request.spec.tag_trigger.as_deref(), Some("v*"));

        let invalid_pattern = AppForm {
            kind: SourceKind::Git,
            repository: "https://github.com/owner/shop.git".into(),
            tag_trigger: "v* invalid".into(),
            ..image_form()
        };
        assert!(invalid_pattern.parse(Locale::En).is_err());
    }

    #[test]
    fn alert_rules_in_form() {
        let form = AppForm {
            alert_cpu_percent: "80".into(),
            alert_memory_mb: "256".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        let rules = request.spec.alert_rules.unwrap();
        assert_eq!(rules.cpu_percent, Some(80));
        assert_eq!(rules.memory_bytes, Some(256 * BYTES_PER_MB));

        let invalid_cpu = AppForm {
            alert_cpu_percent: "not-a-number".into(),
            ..image_form()
        };
        assert!(invalid_cpu.parse(Locale::En).is_err());
    }

    #[test]
    fn git_config_in_form() {
        let form = AppForm {
            git_config_path: "config/prod.env".into(),
            git_config_repo: "https://github.com/owner/config-repo.git".into(),
            git_config_branch: "production".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        let cfg = request.spec.git_config.unwrap();
        assert_eq!(cfg.path.as_str(), "config/prod.env");
        assert_eq!(
            cfg.repository.as_ref().map(|r| r.as_str()),
            Some("https://github.com/owner/config-repo.git")
        );
        assert_eq!(cfg.branch.as_ref().map(|b| b.as_str()), Some("production"));
    }

    #[test]
    fn swap_in_form() {
        let form = AppForm {
            memory_mb: "512".into(),
            swap_mb: "256".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        assert_eq!(request.spec.limits.memory_bytes, Some(512 * BYTES_PER_MB));
        assert_eq!(
            request.spec.limits.memory_swap_bytes,
            Some((512 + 256) * BYTES_PER_MB as i64)
        );

        let swap_without_mem = AppForm {
            memory_mb: String::new(),
            swap_mb: "256".into(),
            ..image_form()
        };
        assert!(swap_without_mem.parse(Locale::En).is_err());
    }

    #[test]
    fn preview_in_form() {
        let form = AppForm {
            preview_enabled: true,
            preview_domain_template: "pr-{pr}.app.example.com".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        let preview = request.spec.preview.unwrap();
        assert!(preview.enabled);
        assert_eq!(
            preview.domain_template.as_deref(),
            Some("pr-{pr}.app.example.com")
        );
    }

    #[test]
    fn buildkit_in_form() {
        let form = AppForm {
            kind: SourceKind::Git,
            repository: "https://github.com/owner/shop.git".into(),
            buildkit_enabled: true,
            buildkit_remote_addr: "tcp://10.0.0.5:1234".into(),
            buildkit_cache_cargo: true,
            buildkit_cache_npm: true,
            buildkit_custom_cache: "/root/.cache/custom\n/tmp/build-cache".into(),
            buildkit_cache_from: "type=registry,ref=test/app:cache".into(),
            buildkit_cache_to: "type=registry,ref=test/app:cache,mode=max".into(),
            ..image_form()
        };
        let request = form.parse(Locale::En).unwrap();
        let bk = request.spec.buildkit.unwrap();
        assert!(bk.enabled);
        assert_eq!(bk.remote_addr.as_deref(), Some("tcp://10.0.0.5:1234"));
        assert!(bk.cache_mounts.contains(&CacheMountPreset::Cargo));
        assert!(bk.cache_mounts.contains(&CacheMountPreset::Npm));
        assert!(!bk.cache_mounts.contains(&CacheMountPreset::Go));
        assert_eq!(
            bk.custom_cache_paths,
            vec!["/root/.cache/custom", "/tmp/build-cache"]
        );
        assert_eq!(
            bk.cache_from.as_deref(),
            Some("type=registry,ref=test/app:cache")
        );
        assert_eq!(
            bk.cache_to.as_deref(),
            Some("type=registry,ref=test/app:cache,mode=max")
        );
    }
}
