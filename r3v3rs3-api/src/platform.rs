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
    /// repository needs the Git token of the app.
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
    },
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
}

impl AppSpec {
    pub fn validate(&self) -> Result<(), Error> {
        if self.port == 0 {
            return Err(invalid("the port must be between 1 and 65535"));
        }
        self.validate_domains()?;
        self.validate_compose()?;
        if let Some(path) = &self.health_check_path {
            validate_health_path(path)?;
        }
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

fn invalid(reason: &str) -> Error {
    Error::InvalidContainerSpec {
        reason: reason.to_string(),
    }
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
    /// The Unix time in milliseconds.
    pub created_at: u64,
    /// The Unix time in milliseconds.
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
}

impl DeploymentTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Webhook => "webhook",
            Self::Rollback => "rollback",
            Self::Api => "api",
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(value: serde_json::Value) -> Result<AppSpec, String> {
        let spec: AppSpec = serde_json::from_value(value).map_err(|err| err.to_string())?;
        spec.validate().map_err(|err| err.to_string())?;
        Ok(spec)
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
}
