//! A container runtime in memory for the unit tests of the platform.

use crate::runtime::ContainerRuntime;
use anyhow::bail;
use r3v3rs3_api::container::{
    APP_LABEL, AppName, ContainerInfo, ContainerName, ContainerSpec, ContainerSummary, ImageInfo,
    ImageRef, NetworkName,
};
use r3v3rs3_api::git::RelPath;
use std::collections::{BTreeMap, BTreeSet};
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
}

#[derive(Default)]
pub struct FakeRuntime(Mutex<FakeState>);

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
        _: Vec<u8>,
        _: &RelPath,
        tag: &ImageRef,
    ) -> anyhow::Result<String> {
        let mut state = self.check()?;
        if let Some(error) = &state.build_error {
            bail!("failed to build {tag}: {error}");
        }
        let id = format!("sha256:{:064x}", state.images.len() + 1);
        let info = ImageInfo {
            id: id.clone(),
            repo_digests: Vec::new(),
        };
        state.images.insert(tag.to_string(), info.clone());
        state.images.insert(id.clone(), info);
        Ok(id)
    }

    async fn remove_image(&self, image: &ImageRef) -> anyhow::Result<()> {
        self.check()?.images.remove(image.as_str());
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
