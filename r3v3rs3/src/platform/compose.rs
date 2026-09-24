//! The deployment of a Compose app: check out the source, add a file that publishes the traffic
//! service on 127.0.0.1, and start the services with `docker compose up`. Compose recreates the
//! changed services, so a Compose app deploys by recreate, not blue-green.

use super::build::remove_dir;
use super::deploy::DEPLOYMENT_LABEL;
use super::{Backend, Platform};
use crate::agent::executor::RESOURCE_PREFIX;
use crate::build::Revision;
use crate::build::compose::{ComposeModel, ComposePort, ComposeProject, ComposeRunner};
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, bail};
use r3v3rs3_api::container::{APP_LABEL, ContainerName, EnvVar, ProjectName, ServiceName};
use r3v3rs3_api::git::{CommitSha, GitRef, RelPath, RepoUrl};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::DeploymentStatus;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use tracing::error;

/// The directory of the Compose checkouts in the config directory. The checkout of the running
/// deployment stays, because the services can mount its files.
pub const COMPOSE_DIR: &str = "compose";

/// The file next to the checkout that publishes the traffic service.
pub const OVERRIDE_FILE: &str = "r3v3rs3.override.yaml";

/// The files that `docker compose` looks for, in its order.
const DEFAULT_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// The version of the repository that a deployment checks out.
#[derive(Debug, Clone)]
pub(super) enum SourceRevision {
    Branch(GitRef),
    /// The commit of an earlier deployment, which a rollback repeats.
    Commit(CommitSha),
}

impl SourceRevision {
    fn as_revision(&self) -> Revision<'_> {
        match self {
            Self::Branch(branch) => Revision::Branch(branch),
            Self::Commit(commit) => Revision::Commit(commit),
        }
    }
}

/// The Compose source of one deployment.
#[derive(Debug, Clone)]
pub(super) struct ComposeBuild {
    pub repository: RepoUrl,
    pub revision: SourceRevision,
    pub file: Option<RelPath>,
    pub service: ServiceName,
    /// The Git provider connection that clones the repository.
    pub connection: Option<ShortId>,
}

/// One deployment of a Compose app.
pub(super) struct ComposeJob<'a> {
    /// The runner of the target of the app.
    pub runner: &'a dyn ComposeRunner,
    pub app: ShortId,
    pub deployment: ShortId,
    /// The port of the traffic service.
    pub port: u16,
    pub source: &'a ComposeBuild,
    pub env: &'a [EnvVar],
    /// The name of the container of the traffic service.
    pub container: &'a ContainerName,
}

impl Platform {
    /// Checks out the source and starts the services. The checkouts of the older deployments of
    /// the app are removed once the services use the new one.
    pub(super) async fn compose_up(&self, job: &ComposeJob<'_>) -> anyhow::Result<()> {
        let _permit = self
            .builds
            .acquire()
            .await
            .context("the build queue is closed")?;
        let app_dir = self.compose_dir.join(job.app.to_string());
        let dir = app_dir.join(job.deployment.to_string());
        let files = match self.prepare_compose(job, &dir).await {
            Ok(files) => files,
            Err(err) => {
                if let Err(err) = remove_dir(&dir).await {
                    error!(dir = %dir.display(), "failed to remove a checkout: {err:#}");
                }
                return Err(err);
            }
        };
        self.store
            .set_status(job.deployment, DeploymentStatus::Deploying)
            .await?;
        let name = project_name(job.app)?;
        let result = job.runner.up(&project(&name, &dir, &files, job.env)).await;
        // The services can use the new checkout now, even when `up` failed halfway.
        if let Err(err) = job.runner.retain(&app_dir, &dir).await {
            error!(app = %job.app, "failed to remove the old checkouts: {err:#}");
        }
        result
    }

    /// Checks out the source, writes the override file and validates the files. Returns the files
    /// of the project.
    async fn prepare_compose(
        &self,
        job: &ComposeJob<'_>,
        dir: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.store
            .set_status(job.deployment, DeploymentStatus::Building)
            .await?;
        tokio::fs::create_dir_all(dir).await?;
        let checkout = dir.join("src");
        self.check_out(job, &checkout).await?;
        let file = compose_file(&checkout, job.source.file.as_ref())?;
        let override_file = dir.join(OVERRIDE_FILE);
        tokio::fs::write(&override_file, override_text(job)?).await?;
        let files = vec![file, override_file];
        let name = project_name(job.app)?;
        let model = job
            .runner
            .config(&project(&name, dir, &files, job.env))
            .await?;
        check_ports(&model, &job.source.service, job.port)?;
        Ok(files)
    }

    /// Checks out the revision of the deployment and records its commit.
    async fn check_out(&self, job: &ComposeJob<'_>, checkout: &Path) -> anyhow::Result<()> {
        let credential = self.git_credential(job.app, job.source.connection).await?;
        let sha = self
            .fetcher
            .fetch(
                &job.source.repository,
                job.source.revision.as_revision(),
                credential.as_ref(),
                checkout,
            )
            .await?;
        self.store.set_commit_sha(job.deployment, &sha).await
    }

    /// Removes the unused built images of the Compose project of an app, after a new build left
    /// the old ones without a tag.
    pub(super) async fn prune_compose_images(
        &self,
        runtime: &dyn ContainerRuntime,
        app: ShortId,
    ) -> anyhow::Result<()> {
        runtime
            .prune_project_images(&project_name(app)?, false)
            .await
    }

    /// Stops the Compose project of an app and removes its containers, its networks, its built
    /// images and its checkouts. An app that never ran as a Compose app has none.
    pub(super) async fn remove_compose_project(
        &self,
        backend: &Backend,
        app: ShortId,
    ) -> anyhow::Result<()> {
        let app_dir = self.compose_dir.join(app.to_string());
        if !tokio::fs::try_exists(&app_dir).await? {
            return Ok(());
        }
        let name = project_name(app)?;
        backend.compose.down(&name).await?;
        backend.runtime.prune_project_images(&name, true).await?;
        backend.compose.remove(&app_dir).await
    }
}

/// The Compose project of an app.
pub(super) fn project_name(app: ShortId) -> anyhow::Result<ProjectName> {
    Ok(format!("{RESOURCE_PREFIX}{app}").parse()?)
}

fn project<'a>(
    name: &'a ProjectName,
    dir: &'a Path,
    files: &'a [PathBuf],
    env: &'a [EnvVar],
) -> ComposeProject<'a> {
    ComposeProject {
        name,
        dir,
        files,
        env,
    }
}

/// The Compose file of the checkout, which must stay inside the checkout after every link is
/// resolved.
fn compose_file(checkout: &Path, file: Option<&RelPath>) -> anyhow::Result<PathBuf> {
    let root = checkout.canonicalize()?;
    let candidates = match file {
        Some(file) => vec![file.as_str()],
        None => DEFAULT_FILES.to_vec(),
    };
    for candidate in &candidates {
        let Ok(path) = root.join(candidate).canonicalize() else {
            continue;
        };
        if !path.starts_with(&root) || !path.is_file() {
            bail!("the Compose file {candidate} is not a file of the repository");
        }
        return Ok(path);
    }
    bail!(
        "the repository has no Compose file: {}",
        candidates.join(", ")
    )
}

/// The file that gives the traffic service its container name, its labels, its port on
/// 127.0.0.1 and the variables of the app. It names the variables only, so their values stay in
/// the environment of the `docker` process and never reach the disk.
fn override_document(job: &ComposeJob<'_>) -> Value {
    let env = job
        .env
        .iter()
        .map(|var| Value::from(var.key.as_str()))
        .collect::<Vec<_>>();
    let service = json!({
        "container_name": job.container.as_str(),
        "labels": {
            APP_LABEL: job.app.to_string(),
            DEPLOYMENT_LABEL: job.deployment.to_string(),
        },
        "ports": [format!("127.0.0.1::{}", job.port)],
        "environment": env,
    });
    let mut services = Map::new();
    services.insert(job.source.service.to_string(), service);
    json!({ "services": services })
}

/// The override document as YAML in the flow style. The `!override` tag replaces the ports of the
/// traffic service instead of adding to them, which JSON cannot express. The other values are
/// validated names, so the only `"ports":[` of the text is the key of that list.
fn override_text(job: &ComposeJob<'_>) -> anyhow::Result<String> {
    let json = serde_json::to_string(&override_document(job))?;
    Ok(json.replacen("\"ports\":[", "\"ports\":!override [", 1))
}

/// Only the port that the override file sets may reach the host, because r3v3rs3 routes the
/// requests to the app and a published port of another service would bypass it.
fn check_ports(model: &ComposeModel, service: &ServiceName, port: u16) -> anyhow::Result<()> {
    if !model.services.contains_key(service.as_str()) {
        bail!("the Compose file has no service {service}");
    }
    let expected = ComposePort {
        target: port,
        published: None,
        host_ip: Some("127.0.0.1".into()),
    };
    for (name, entry) in &model.services {
        let own = name == service.as_str();
        if let Some(published) = entry.ports.iter().find(|p| !own || **p != expected) {
            bail!(
                "the service {name} publishes its port {}; remove the ports of the Compose file, \
                 because r3v3rs3 publishes the port {port} of the service {service} itself",
                published.target
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn model(services: &[(&str, Vec<ComposePort>)]) -> ComposeModel {
        let services = services
            .iter()
            .map(|(name, ports)| {
                let service = crate::build::compose::ComposeService {
                    ports: ports.clone(),
                };
                (name.to_string(), service)
            })
            .collect::<BTreeMap<_, _>>();
        ComposeModel { services }
    }

    fn port(target: u16, published: Option<&str>, host_ip: Option<&str>) -> ComposePort {
        ComposePort {
            target,
            published: published.map(str::to_string),
            host_ip: host_ip.map(str::to_string),
        }
    }

    #[test]
    fn only_the_port_of_the_override_reaches_the_host() -> anyhow::Result<()> {
        let web: ServiceName = "web".parse()?;
        let own = port(80, None, Some("127.0.0.1"));
        let valid = model(&[("web", vec![own.clone()]), ("db", Vec::new())]);
        check_ports(&valid, &web, 80)?;

        let cases = [
            model(&[("web", vec![own.clone(), port(80, Some("8080"), None)])]),
            model(&[
                ("web", vec![own.clone()]),
                ("db", vec![port(5432, None, None)]),
            ]),
            model(&[("web", vec![port(80, None, None)])]),
            model(&[("api", vec![own])]),
        ];
        for case in cases {
            assert!(check_ports(&case, &web, 80).is_err(), "{case:?}");
        }
        Ok(())
    }

    #[test]
    fn the_override_names_the_variables_without_their_values() -> anyhow::Result<()> {
        let source = ComposeBuild {
            connection: None,
            repository: "https://git.example.com/team/shop.git".parse()?,
            revision: SourceRevision::Branch(GitRef::main()),
            file: None,
            service: "web".parse()?,
        };
        let env = [EnvVar {
            key: "DATABASE_URL".parse()?,
            value: "postgres://secret".into(),
        }];
        let container = "r3v3rs3-bcd-fgh-jkl-mnp".parse()?;
        let runner = crate::build::compose::DockerCompose::new("unix:///var/run/docker.sock");
        let job = ComposeJob {
            runner: &runner,
            app: "bcd-fgh".parse()?,
            deployment: "jkl-mnp".parse()?,
            port: 8080,
            source: &source,
            env: &env,
            container: &container,
        };
        let document = override_document(&job);
        let web = &document["services"]["web"];
        assert_eq!(web["container_name"], "r3v3rs3-bcd-fgh-jkl-mnp");
        assert_eq!(web["ports"], json!(["127.0.0.1::8080"]));
        assert_eq!(web["environment"], json!(["DATABASE_URL"]));
        assert_eq!(web["labels"][APP_LABEL], "bcd-fgh");
        let text = override_text(&job)?;
        assert!(
            text.contains(r#""ports":!override ["127.0.0.1::8080"]"#),
            "{text}"
        );
        assert!(!text.contains("secret"), "{text}");
        Ok(())
    }

    #[test]
    fn the_compose_file_is_the_named_one_or_the_first_default() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "r3v3rs3-compose-file-{}",
            hex::encode(rand::random::<[u8; 8]>())
        ));
        std::fs::create_dir_all(dir.join("deploy"))?;
        let result = check_compose_files(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    fn check_compose_files(dir: &Path) -> anyhow::Result<()> {
        assert!(compose_file(dir, None).is_err());
        std::fs::write(dir.join("docker-compose.yml"), "services: {}\n")?;
        std::fs::write(dir.join("deploy/prod.yaml"), "services: {}\n")?;
        std::os::unix::fs::symlink("/etc/hosts", dir.join("outside.yaml"))?;
        let found = compose_file(dir, None)?;
        assert!(found.ends_with("docker-compose.yml"), "{found:?}");
        let named = compose_file(dir, Some(&"deploy/prod.yaml".parse()?))?;
        assert!(named.ends_with("deploy/prod.yaml"), "{named:?}");
        assert!(compose_file(dir, Some(&"outside.yaml".parse()?)).is_err());
        assert!(compose_file(dir, Some(&"deploy".parse()?)).is_err());
        Ok(())
    }
}
