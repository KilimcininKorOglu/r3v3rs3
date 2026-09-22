//! A container runtime, a source fetcher and a Compose runner in memory for the unit tests of the
//! platform.

use crate::build::compose::{ComposeModel, ComposePort, ComposeProject, ComposeRunner};
use crate::build::{Revision, SourceFetcher};
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, bail};
use r3v3rs3_api::container::{
    APP_LABEL, AppName, COMPOSE_PROJECT_LABEL, ContainerInfo, ContainerName, ContainerSpec,
    ContainerSummary, ImageInfo, ImageRef, NetworkName, ProjectName,
};
use r3v3rs3_api::git::{RelPath, RepoUrl};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

/// The image digest of every pulled image.
pub const DIGEST: &str = "sha256:0123456789abcdef";

#[derive(Default)]
pub struct FakeState {
    pub containers: BTreeMap<String, ContainerInfo>,
    pub images: BTreeMap<String, ImageInfo>,
    pub networks: BTreeSet<String>,
    /// Every call fails.
    pub broken: bool,
    /// A pull of an image whose reference contains this text fails.
    pub missing_image: Option<String>,
    /// A started container exits at once with code 1.
    pub crash: bool,
    /// The port of 127.0.0.1 that a started container publishes.
    pub host_port: u16,
    /// Every build fails with this error.
    pub build_error: Option<String>,
    /// The builds in their order.
    pub builds: Vec<Build>,
    /// The Compose projects whose images were pruned, and whether every unused image went.
    pub pruned: Vec<(String, bool)>,
}

/// One build of the fake runtime.
pub struct Build {
    pub tag: String,
    pub dockerfile: String,
    pub context: Vec<u8>,
}

#[derive(Default)]
pub struct FakeRuntime(Mutex<FakeState>);

/// The commit of every fake checkout.
pub const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

#[derive(Default)]
pub struct FetchState {
    /// Every fetch fails with this error.
    pub error: Option<String>,
    /// The repository, the branch and the token of every fetch.
    pub fetched: Vec<(String, String, Option<String>)>,
    /// The `compose.yaml` of the checkout. Without it the file has no services.
    pub compose_file: Option<String>,
}

/// A source fetcher that writes a checkout with `.git` and `app/Dockerfile`.
#[derive(Default)]
pub struct FakeFetcher(Mutex<FetchState>);

impl FakeFetcher {
    pub fn state(&self) -> MutexGuard<'_, FetchState> {
        match self.0.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[async_trait::async_trait]
impl SourceFetcher for FakeFetcher {
    async fn fetch(
        &self,
        repository: &RepoUrl,
        revision: Revision<'_>,
        token: Option<&str>,
        dest: &Path,
    ) -> anyhow::Result<String> {
        let compose = {
            let mut state = self.state();
            if let Some(error) = &state.error {
                bail!("git failed: {error}");
            }
            let fetch = (
                repository.to_string(),
                revision.as_str().to_string(),
                token.map(str::to_string),
            );
            state.fetched.push(fetch);
            state.compose_file.clone()
        };
        std::fs::create_dir_all(dest.join(".git"))?;
        std::fs::create_dir_all(dest.join("app"))?;
        std::fs::write(dest.join(".git/config"), "[core]\n")?;
        std::fs::write(dest.join("app/Dockerfile"), "FROM scratch\n")?;
        let compose = compose.unwrap_or_else(|| "services: {}\n".into());
        std::fs::write(dest.join("compose.yaml"), compose)?;
        Ok(match revision {
            Revision::Branch(_) => COMMIT.into(),
            Revision::Commit(commit) => commit.to_string(),
        })
    }
}

#[derive(Default)]
pub struct ComposeState {
    /// Ports that the resolved model has besides the ports of the override file, as
    /// `(service, port)`.
    pub extra_ports: Vec<(String, ComposePort)>,
    /// Every `up` fails with this error.
    pub up_error: Option<String>,
    /// The commands in their order, as `<command> <project>`.
    pub calls: Vec<String>,
    /// The variables of the last command.
    pub env: Vec<(String, String)>,
}

/// A Compose runner that reads the override file of the platform and starts the traffic service
/// as a container of the fake runtime.
pub struct FakeCompose {
    state: Mutex<ComposeState>,
    runtime: Arc<FakeRuntime>,
}

impl FakeCompose {
    pub fn new(runtime: Arc<FakeRuntime>) -> Self {
        Self {
            state: Mutex::new(ComposeState::default()),
            runtime,
        }
    }

    pub fn state(&self) -> MutexGuard<'_, ComposeState> {
        match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn record(&self, command: &str, project: &ComposeProject<'_>) {
        let mut state = self.state();
        state.calls.push(format!("{command} {}", project.name));
        state.env = project
            .env
            .iter()
            .map(|var| (var.key.as_str().to_string(), var.value.clone()))
            .collect();
    }
}

/// The traffic service of the override file: its name and its settings.
fn override_service(project: &ComposeProject<'_>) -> anyhow::Result<(String, Value)> {
    let path = project.files.last().context("no override file")?;
    // The tag is YAML, and the rest of the file is JSON.
    let text = std::fs::read_to_string(path)?.replacen("!override ", "", 1);
    let document: Value = serde_json::from_str(&text)?;
    let services = document["services"].as_object().context("no services")?;
    let (name, service) = services.iter().next().context("no service")?;
    Ok((name.clone(), service.clone()))
}

/// The container port of a `127.0.0.1::<port>` entry.
fn override_port(service: &Value) -> anyhow::Result<u16> {
    let entry = service["ports"][0].as_str().context("no port")?;
    Ok(entry.trim_start_matches("127.0.0.1::").parse()?)
}

#[async_trait::async_trait]
impl ComposeRunner for FakeCompose {
    async fn config(&self, project: &ComposeProject<'_>) -> anyhow::Result<ComposeModel> {
        self.record("config", project);
        if !project.files[0].is_file() {
            bail!("no such Compose file");
        }
        let (name, service) = override_service(project)?;
        let own = ComposePort {
            target: override_port(&service)?,
            published: None,
            host_ip: Some("127.0.0.1".into()),
        };
        let mut model = ComposeModel::default();
        model.services.entry(name).or_default().ports.push(own);
        for (name, port) in &self.state().extra_ports {
            model
                .services
                .entry(name.clone())
                .or_default()
                .ports
                .push(port.clone());
        }
        Ok(model)
    }

    /// Recreates the traffic service: the container of the earlier deployment goes, and the
    /// container of the override file starts.
    async fn up(&self, project: &ComposeProject<'_>) -> anyhow::Result<()> {
        self.record("up", project);
        if let Some(error) = &self.state().up_error {
            bail!("docker failed with exit status: 1: {error}");
        }
        let (_, service) = override_service(project)?;
        let name: ContainerName = service["container_name"]
            .as_str()
            .context("name")?
            .parse()?;
        let mut labels: BTreeMap<String, String> =
            serde_json::from_value(service["labels"].clone())?;
        labels.insert(COMPOSE_PROJECT_LABEL.into(), project.name.to_string());
        let mut state = self.runtime.check()?;
        remove_project(&mut state, project.name);
        let published = [(override_port(&service)?, state.host_port)];
        let mut info = container(&name, labels, !state.crash, &published);
        info.exit_code = i64::from(state.crash);
        state.containers.insert(name.to_string(), info);
        Ok(())
    }

    async fn down(&self, name: &ProjectName) -> anyhow::Result<()> {
        self.state().calls.push(format!("down {name}"));
        let mut state = self.runtime.check()?;
        remove_project(&mut state, name);
        Ok(())
    }
}

fn remove_project(state: &mut FakeState, project: &ProjectName) {
    state.containers.retain(|_, info| {
        info.labels.get(COMPOSE_PROJECT_LABEL).map(String::as_str) != Some(project.as_str())
    });
}

impl FakeRuntime {
    pub fn state(&self) -> MutexGuard<'_, FakeState> {
        match self.0.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Adds a container in the state that a deployment leaves.
    pub fn set(&self, name: &ContainerName, running: bool, published: &[(u16, u16)]) {
        let info = container(name, BTreeMap::new(), running, published);
        self.state().containers.insert(name.to_string(), info);
    }

    /// The names of the containers.
    pub fn names(&self) -> Vec<String> {
        self.state().containers.keys().cloned().collect()
    }

    fn check(&self) -> anyhow::Result<MutexGuard<'_, FakeState>> {
        let state = self.state();
        if state.broken {
            bail!("the Docker Engine does not answer");
        }
        Ok(state)
    }
}

fn container(
    name: &ContainerName,
    labels: BTreeMap<String, String>,
    running: bool,
    published: &[(u16, u16)],
) -> ContainerInfo {
    ContainerInfo {
        id: format!("id-{name}"),
        name: name.to_string(),
        image_id: DIGEST.into(),
        running,
        exit_code: 0,
        health: None,
        addresses: BTreeMap::new(),
        labels,
        published: published.iter().copied().collect(),
    }
}

#[async_trait::async_trait]
impl ContainerRuntime for FakeRuntime {
    async fn version(&self) -> anyhow::Result<String> {
        let _state = self.check()?;
        Ok("fake".into())
    }

    async fn pull_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        let mut state = self.check()?;
        if let Some(missing) = &state.missing_image
            && image.as_str().contains(missing.as_str())
        {
            bail!("manifest unknown: {image}");
        }
        let digest = format!("{}@{DIGEST}", image.repository());
        let info = ImageInfo {
            id: DIGEST.into(),
            repo_digests: vec![digest.clone()],
        };
        state.images.insert(image.to_string(), info.clone());
        state.images.insert(digest, info);
        Ok(())
    }

    async fn inspect_image(&self, image: &ImageRef) -> anyhow::Result<Option<ImageInfo>> {
        Ok(self.check()?.images.get(image.as_str()).cloned())
    }

    async fn build_image(
        &self,
        context: Vec<u8>,
        dockerfile: &RelPath,
        tag: &ImageRef,
    ) -> anyhow::Result<String> {
        let mut state = self.check()?;
        if let Some(error) = &state.build_error {
            bail!("failed to build {tag}: {error}");
        }
        state.builds.push(Build {
            tag: tag.to_string(),
            dockerfile: dockerfile.to_string(),
            context,
        });
        let id = format!("sha256:{:064x}", state.builds.len());
        let info = ImageInfo {
            id: id.clone(),
            repo_digests: Vec::new(),
        };
        state.images.insert(tag.to_string(), info.clone());
        state.images.insert(id.clone(), info);
        Ok(id)
    }

    /// Removes the reference, and like Docker also the image when no other tag names it.
    async fn remove_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        let mut state = self.check()?;
        let Some(info) = state.images.remove(image.as_str()) else {
            return Ok(());
        };
        let tagged = state
            .images
            .iter()
            .any(|(name, other)| *name != info.id && other.id == info.id);
        if !tagged {
            state.images.remove(&info.id);
        }
        Ok(())
    }

    async fn prune_project_images(&self, project: &ProjectName, all: bool) -> anyhow::Result<()> {
        self.check()?.pruned.push((project.to_string(), all));
        Ok(())
    }

    async fn ensure_network(&self, network: &NetworkName, _: &AppName) -> anyhow::Result<()> {
        self.check()?.networks.insert(network.to_string());
        Ok(())
    }

    async fn remove_network(&self, network: &NetworkName) -> anyhow::Result<()> {
        self.check()?.networks.remove(network.as_str());
        Ok(())
    }

    async fn create_container(&self, spec: &ContainerSpec) -> anyhow::Result<String> {
        spec.validate()?;
        let mut state = self.check()?;
        if !state.images.contains_key(spec.image.as_str()) {
            bail!("no such image: {}", spec.image);
        }
        let published = spec
            .publish
            .map(|port| vec![(port, state.host_port)])
            .unwrap_or_default();
        let info = container(&spec.name, spec.labels.clone(), false, &published);
        let id = info.id.clone();
        state.containers.insert(spec.name.to_string(), info);
        Ok(id)
    }

    async fn start_container(&self, name: &ContainerName) -> anyhow::Result<()> {
        let mut state = self.check()?;
        let crash = state.crash;
        let Some(info) = state.containers.get_mut(name.as_str()) else {
            bail!("no container is named {name}");
        };
        info.running = !crash;
        info.exit_code = i64::from(crash);
        Ok(())
    }

    async fn stop_container(&self, name: &ContainerName, _: Duration) -> anyhow::Result<()> {
        let mut state = self.check()?;
        if let Some(info) = state.containers.get_mut(name.as_str()) {
            info.running = false;
        }
        Ok(())
    }

    async fn remove_container(&self, name: &ContainerName) -> anyhow::Result<()> {
        self.check()?.containers.remove(name.as_str());
        Ok(())
    }

    async fn inspect_container(
        &self,
        name: &ContainerName,
    ) -> anyhow::Result<Option<ContainerInfo>> {
        Ok(self.check()?.containers.get(name.as_str()).cloned())
    }

    async fn list_containers(&self, app: &AppName) -> anyhow::Result<Vec<ContainerSummary>> {
        let state = self.check()?;
        let summaries = state
            .containers
            .values()
            .filter(|info| info.labels.get(APP_LABEL).map(String::as_str) == Some(app.as_str()))
            .map(|info| ContainerSummary {
                id: info.id.clone(),
                name: info.name.clone(),
                state: if info.running { "running" } else { "exited" }.into(),
                labels: info.labels.clone(),
            })
            .collect();
        Ok(summaries)
    }

    async fn logs(&self, name: &ContainerName, _: u32) -> anyhow::Result<String> {
        let _state = self.check()?;
        Ok(format!("the log of {name}\n"))
    }
}
