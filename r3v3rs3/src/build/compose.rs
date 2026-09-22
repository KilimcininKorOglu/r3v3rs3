//! Runs `docker compose` for the Compose apps of the platform. The process gets only the
//! variables that the `docker` binary needs and the variables of the app, so the environment of
//! r3v3rs3 does not reach the interpolation of a Compose file.

use super::process::run;
use anyhow::bail;
use r3v3rs3_api::container::{EnvVar, ProjectName};
use serde_derive::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// The longest time of `up`, which also builds the images of the project.
const UP_TIMEOUT: Duration = Duration::from_secs(1800);

/// The longest time of `config` and `down`.
const TIMEOUT: Duration = Duration::from_secs(300);

/// The variables of the `docker` process itself. An app variable with one of these names is
/// refused, because it would change the Docker Engine or the plugins that `docker` uses.
pub const RESERVED_ENV: [&str; 4] = ["PATH", "HOME", "DOCKER_CONFIG", "DOCKER_HOST"];

/// The files of one Compose project.
pub struct ComposeProject<'a> {
    pub name: &'a ProjectName,
    /// The working directory. The relative paths of a file resolve against the directory of the
    /// first file.
    pub dir: &'a Path,
    pub files: &'a [PathBuf],
    /// The variables that the files interpolate.
    pub env: &'a [EnvVar],
}

/// The part of the resolved Compose model that the platform reads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ComposeModel {
    #[serde(default)]
    pub services: BTreeMap<String, ComposeService>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ComposeService {
    #[serde(default)]
    pub ports: Vec<ComposePort>,
}

/// One port of a service. A port without `published` gets a free host port from Docker.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ComposePort {
    pub target: u16,
    #[serde(default)]
    pub published: Option<String>,
    #[serde(default)]
    pub host_ip: Option<String>,
}

/// Runs the Compose commands of a project.
#[async_trait::async_trait]
pub trait ComposeRunner: Send + Sync {
    /// Validates the files and returns the resolved model.
    async fn config(&self, project: &ComposeProject<'_>) -> anyhow::Result<ComposeModel>;

    /// Builds the images and starts the services. A service whose configuration changed is
    /// recreated.
    async fn up(&self, project: &ComposeProject<'_>) -> anyhow::Result<()>;

    /// Stops and removes the containers, the networks and the built images of the project.
    /// Named volumes stay.
    async fn down(&self, name: &ProjectName) -> anyhow::Result<()>;
}

/// The runner that calls the `docker` binary with the Compose plugin.
pub struct DockerCompose {
    docker_host: String,
}

impl DockerCompose {
    /// A runner for the Docker Engine at `docker_host`, in the format of `DOCKER_HOST`.
    pub fn new(docker_host: impl Into<String>) -> Self {
        Self {
            docker_host: docker_host.into(),
        }
    }

    fn command(&self, name: &ProjectName, dir: &Path) -> Command {
        let mut command = Command::new("docker");
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("DOCKER_HOST", &self.docker_host);
        // The plugin directory and the registry logins of the host.
        for key in ["HOME", "DOCKER_CONFIG"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
            .args([
                "compose",
                "--ansi",
                "never",
                "--project-name",
                name.as_str(),
            ])
            .current_dir(dir);
        command
    }

    fn project_command(&self, project: &ComposeProject<'_>) -> anyhow::Result<Command> {
        let mut command = self.command(project.name, project.dir);
        for var in project.env {
            if RESERVED_ENV.contains(&var.key.as_str()) {
                let key = var.key.as_str();
                bail!("the variable {key} is reserved for the docker binary");
            }
            command.env(var.key.as_str(), &var.value);
        }
        for file in project.files {
            command.arg("--file").arg(file);
        }
        Ok(command)
    }
}

#[async_trait::async_trait]
impl ComposeRunner for DockerCompose {
    async fn config(&self, project: &ComposeProject<'_>) -> anyhow::Result<ComposeModel> {
        let mut command = self.project_command(project)?;
        command.args(["config", "--format", "json"]);
        let output = run(command, TIMEOUT).await?;
        Ok(serde_json::from_str(&output)?)
    }

    async fn up(&self, project: &ComposeProject<'_>) -> anyhow::Result<()> {
        let mut command = self.project_command(project)?;
        command.args([
            "up",
            "--detach",
            "--build",
            "--remove-orphans",
            "--quiet-pull",
        ]);
        run(command, UP_TIMEOUT).await?;
        Ok(())
    }

    async fn down(&self, name: &ProjectName) -> anyhow::Result<()> {
        // Without a Compose file, `down` finds the containers by the project name.
        let mut command = self.command(name, Path::new("/"));
        command.args(["down", "--remove-orphans", "--rmi", "local"]);
        run(command, TIMEOUT).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_env(env: &[EnvVar]) -> anyhow::Result<Vec<(String, String)>> {
        let compose = DockerCompose::new("unix:///var/run/docker.sock");
        let name = "r3v3rs3-abc-def".parse()?;
        let files = [PathBuf::from("compose.yaml")];
        let project = ComposeProject {
            name: &name,
            dir: Path::new("/tmp"),
            files: &files,
            env,
        };
        let command = compose.project_command(&project)?;
        Ok(command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                let value = value?.to_string_lossy().into_owned();
                Some((key.to_string_lossy().into_owned(), value))
            })
            .collect())
    }

    #[test]
    fn the_process_gets_the_app_variables_and_the_docker_host() -> anyhow::Result<()> {
        let env = [EnvVar {
            key: "DATABASE_URL".parse()?,
            value: "postgres://db".into(),
        }];
        let vars = project_env(&env)?;
        assert!(vars.contains(&("DATABASE_URL".into(), "postgres://db".into())));
        assert!(vars.contains(&("DOCKER_HOST".into(), "unix:///var/run/docker.sock".into())));
        let keys = vars.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>();
        assert!(
            keys.iter()
                .all(|key| env.iter().any(|var| var.key.as_str() == *key)
                    || RESERVED_ENV.contains(key)),
            "{keys:?}"
        );
        Ok(())
    }

    #[test]
    fn an_app_cannot_set_a_variable_of_the_docker_binary() -> anyhow::Result<()> {
        for key in RESERVED_ENV {
            let env = [EnvVar {
                key: key.parse()?,
                value: "x".into(),
            }];
            assert!(project_env(&env).is_err(), "{key}");
        }
        Ok(())
    }

    #[test]
    fn the_model_reads_the_ports_of_each_service() -> anyhow::Result<()> {
        let model: ComposeModel = serde_json::from_str(
            r#"{"name": "p", "services": {
                "web": {"image": "a", "ports": [{"mode": "ingress", "host_ip": "127.0.0.1",
                    "target": 80, "protocol": "tcp"}]},
                "db": {"image": "b", "environment": {"SECRET": "x"}}
            }}"#,
        )?;
        let web = &model.services["web"].ports;
        assert_eq!(
            web,
            &[ComposePort {
                target: 80,
                published: None,
                host_ip: Some("127.0.0.1".into()),
            }]
        );
        assert!(model.services["db"].ports.is_empty());
        Ok(())
    }
}
