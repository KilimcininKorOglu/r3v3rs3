//! The Compose apps of an agent target. The master checks out the source and writes the override
//! file in its own deployment directory, as for a local app. [`RemoteCompose`] sends that
//! directory to the agent, and [`AgentCompose`] unpacks it at the same relative path in the
//! Compose directory of the agent and runs `docker compose` there.

use super::executor::RESOURCE_PREFIX;
use super::link::Link;
use super::protocol::{AgentOutput, AgentRequest, ComposeRequest};
use super::registry::AgentRegistry;
use crate::build::compose::{ComposeModel, ComposeProject, ComposeRunner};
use crate::build::{directory_archive, remove_other_dirs};
use anyhow::{Context as _, anyhow, bail};
use r3v3rs3_api::container::ProjectName;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git::RelPath;
use r3v3rs3_api::id::ShortId;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The agent side: runs the Compose requests of the master in `dir`.
pub struct AgentCompose {
    runner: Arc<dyn ComposeRunner>,
    dir: PathBuf,
}

impl AgentCompose {
    pub fn new(runner: Arc<dyn ComposeRunner>, dir: PathBuf) -> Self {
        Self { runner, dir }
    }

    fn app_dir(&self, app: ShortId) -> PathBuf {
        self.dir.join(app.to_string())
    }

    fn deployment_dir(&self, request: &ComposeRequest) -> PathBuf {
        self.app_dir(request.app)
            .join(request.deployment.to_string())
    }

    /// Unpacks the deployment directory and resolves the files of the project.
    pub async fn config(
        &self,
        request: &ComposeRequest,
        checkout: Vec<u8>,
    ) -> anyhow::Result<AgentOutput> {
        check_project(&request.project)?;
        let dir = self.deployment_dir(request);
        let target = dir.clone();
        tokio::task::spawn_blocking(move || unpack(&checkout, &target)).await??;
        let files = files_in(&dir, &request.files);
        let model = self.runner.config(&project(request, &dir, &files)).await?;
        Ok(AgentOutput::ComposeModel { model })
    }

    pub async fn up(&self, request: &ComposeRequest) -> anyhow::Result<AgentOutput> {
        check_project(&request.project)?;
        let dir = self.deployment_dir(request);
        if !tokio::fs::try_exists(&dir).await? {
            bail!(
                "the deployment {} has no files on the agent",
                request.deployment
            );
        }
        let files = files_in(&dir, &request.files);
        self.runner.up(&project(request, &dir, &files)).await?;
        Ok(AgentOutput::Done)
    }

    pub async fn down(&self, name: &ProjectName) -> anyhow::Result<AgentOutput> {
        check_project(name)?;
        self.runner.down(name).await?;
        Ok(AgentOutput::Done)
    }

    pub async fn retain(&self, app: ShortId, keep: ShortId) -> anyhow::Result<AgentOutput> {
        let app_dir = self.app_dir(app);
        if tokio::fs::try_exists(&app_dir).await? {
            let keep = app_dir.join(keep.to_string());
            self.runner.retain(&app_dir, &keep).await?;
        }
        Ok(AgentOutput::Done)
    }

    pub async fn remove(&self, app: ShortId) -> anyhow::Result<AgentOutput> {
        self.runner.remove(&self.app_dir(app)).await?;
        Ok(AgentOutput::Done)
    }
}

fn check_project(name: &ProjectName) -> anyhow::Result<()> {
    if !name.as_str().starts_with(RESOURCE_PREFIX) {
        bail!("{name} is not a Compose project of the platform");
    }
    Ok(())
}

fn files_in(dir: &Path, files: &[RelPath]) -> Vec<PathBuf> {
    files.iter().map(|file| dir.join(file.as_str())).collect()
}

fn project<'a>(
    request: &'a ComposeRequest,
    dir: &'a Path,
    files: &'a [PathBuf],
) -> ComposeProject<'a> {
    ComposeProject {
        name: &request.project,
        dir,
        files,
        env: &request.env,
    }
}

/// Replaces `dir` with the content of a tar archive. The archive cannot write outside `dir`.
fn unpack(archive: &[u8], dir: &Path) -> anyhow::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    tar::Archive::new(archive)
        .unpack(dir)
        .with_context(|| format!("cannot unpack the deployment into {}", dir.display()))
}

/// The master side: sends the Compose commands of an agent target to its agent.
pub struct RemoteCompose {
    target: ShortId,
    agents: Arc<AgentRegistry>,
    /// The Compose directory of the master, which holds the deployment directories.
    root: PathBuf,
}

impl RemoteCompose {
    pub fn new(target: ShortId, agents: Arc<AgentRegistry>, root: PathBuf) -> Self {
        Self {
            target,
            agents,
            root,
        }
    }

    fn link(&self) -> Result<Link, Error> {
        self.agents
            .link(self.target)
            .ok_or(Error::AgentOffline { id: self.target })
    }

    async fn call(&self, request: &AgentRequest) -> anyhow::Result<()> {
        match self.link()?.request(request).await? {
            AgentOutput::Done => Ok(()),
            other => Err(anyhow!("the agent gave an unexpected answer: {other:?}")),
        }
    }

    /// The ids of the app and the deployment of a directory in the Compose directory.
    fn ids(&self, dir: &Path) -> anyhow::Result<(ShortId, ShortId)> {
        let relative = dir
            .strip_prefix(&self.root)
            .with_context(|| format!("{} is not a deployment directory", dir.display()))?;
        let mut parts = relative.iter().map(|part| part.to_string_lossy());
        match (parts.next(), parts.next(), parts.next()) {
            (Some(app), Some(deployment), None) => Ok((app.parse()?, deployment.parse()?)),
            _ => bail!("{} is not a deployment directory", dir.display()),
        }
    }

    fn request(&self, project: &ComposeProject<'_>) -> anyhow::Result<ComposeRequest> {
        let (app, deployment) = self.ids(project.dir)?;
        // A file path is a join of the directory, or a canonical path inside it.
        let dirs = [project.dir.to_path_buf(), project.dir.canonicalize()?];
        let files = project
            .files
            .iter()
            .map(|file| relative_file(&dirs, file))
            .collect::<anyhow::Result<_>>()?;
        Ok(ComposeRequest {
            project: project.name.clone(),
            app,
            deployment,
            files,
            env: project.env.to_vec(),
        })
    }
}

fn relative_file(dirs: &[PathBuf], file: &Path) -> anyhow::Result<RelPath> {
    let relative = dirs
        .iter()
        .find_map(|dir| file.strip_prefix(dir).ok())
        .with_context(|| format!("{} is outside the deployment", file.display()))?;
    let relative = relative
        .to_str()
        .with_context(|| format!("{} is not UTF-8", file.display()))?;
    Ok(relative.parse()?)
}

#[async_trait::async_trait]
impl ComposeRunner for RemoteCompose {
    async fn config(&self, project: &ComposeProject<'_>) -> anyhow::Result<ComposeModel> {
        let request = AgentRequest::ComposeConfig(self.request(project)?);
        let dir = project.dir.to_path_buf();
        let archive = tokio::task::spawn_blocking(move || directory_archive(&dir)).await??;
        match self
            .link()?
            .request_with_payload(&request, &archive)
            .await?
        {
            AgentOutput::ComposeModel { model } => Ok(model),
            other => Err(anyhow!("the agent gave an unexpected answer: {other:?}")),
        }
    }

    async fn up(&self, project: &ComposeProject<'_>) -> anyhow::Result<()> {
        self.call(&AgentRequest::ComposeUp(self.request(project)?))
            .await
    }

    async fn down(&self, name: &ProjectName) -> anyhow::Result<()> {
        let project = name.clone();
        self.call(&AgentRequest::ComposeDown { project }).await
    }

    /// Removes the other deployment directories on the agent and on the master.
    async fn retain(&self, app_dir: &Path, keep: &Path) -> anyhow::Result<()> {
        let (app, deployment) = self.ids(keep)?;
        let request = AgentRequest::ComposeRetain {
            app,
            keep: deployment,
        };
        self.call(&request).await?;
        remove_other_dirs(app_dir, keep).await
    }

    async fn remove(&self, app_dir: &Path) -> anyhow::Result<()> {
        let app = app_dir
            .file_name()
            .and_then(|name| name.to_str())
            .context("the app directory has no name")?
            .parse()?;
        self.call(&AgentRequest::ComposeRemove { app }).await?;
        Ok(crate::build::remove_dir(app_dir).await?)
    }
}
