//! The build step of a deployment from a Git source: clone the branch, pack the build context and
//! build the image with the Docker Engine. Each deployment tags its own image, and the images of
//! old deployments are removed.

use super::Platform;
use crate::agent::executor::BUILD_REPOSITORY_PREFIX;
use crate::build::{Revision, context_archive};
use crate::runtime::ContainerRuntime;
use anyhow::Context as _;
use r3v3rs3_api::container::ImageRef;
use r3v3rs3_api::git::{GitRef, RelPath, RepoUrl};
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{BuildKitConfig, DeploymentStatus};
use std::path::Path;
use tracing::error;

/// The directory of the checkouts in the config directory. A checkout exists only during its
/// build.
pub const BUILD_DIR: &str = "builds";

/// The builds that run at the same time.
pub const MAX_BUILDS: usize = 2;

/// The newest deployments of an app whose built images stay, so that a rollback can start them.
pub const KEPT_BUILDS: usize = 5;

/// The Git source of one deployment.
#[derive(Debug, Clone)]
pub(super) struct GitBuild {
    pub repository: RepoUrl,
    pub branch: GitRef,
    pub context: RelPath,
    pub dockerfile: RelPath,
    /// The Git provider connection that clones the repository.
    pub connection: Option<ShortId>,
    pub buildkit: Option<r3v3rs3_api::platform::BuildKitConfig>,
}

impl Platform {
    /// Builds the image of a deployment and returns its image id. The checkout is removed after
    /// the build, whether the build passed or not.
    pub(super) async fn build(
        &self,
        runtime: &dyn ContainerRuntime,
        app: ShortId,
        deployment: ShortId,
        source: &GitBuild,
    ) -> anyhow::Result<String> {
        let _permit = self
            .builds
            .acquire()
            .await
            .context("the build queue is closed")?;
        let dir = self.build_dir.join(deployment.to_string());
        let result = self.build_in(runtime, app, deployment, source, &dir).await;
        if let Err(err) = remove_dir(&dir).await {
            error!(dir = %dir.display(), "failed to remove a checkout: {err:#}");
        }
        result
    }

    async fn build_in(
        &self,
        runtime: &dyn ContainerRuntime,
        app: ShortId,
        deployment: ShortId,
        source: &GitBuild,
        dir: &Path,
    ) -> anyhow::Result<String> {
        tokio::fs::create_dir_all(dir).await?;
        let checkout = dir.join("src");
        let credential = self.git_credential(app, source.connection).await?;
        let sha = self
            .fetcher
            .fetch(
                &source.repository,
                Revision::Branch(&source.branch),
                credential.as_ref(),
                &checkout,
            )
            .await?;
        self.store.set_commit_sha(deployment, &sha).await?;
        let tag = build_tag(app, deployment)?;
        if let Some(img_id) = try_buildkit(source, &checkout, &tag).await {
            return Ok(img_id);
        }
        let context = source.context.clone();
        let archive =
            tokio::task::spawn_blocking(move || context_archive(&checkout, &context)).await??;
        runtime.build_image(archive, &source.dockerfile, &tag).await
    }

    /// Removes the built images of the deployments after the newest [`KEPT_BUILDS`], except the
    /// image of the running deployment. Each pipeline runs it, so each deployment passes the
    /// window once.
    pub(super) async fn prune_builds(
        &self,
        runtime: &dyn ContainerRuntime,
        app: ShortId,
    ) -> anyhow::Result<()> {
        let kept = self
            .prune_policy()
            .await
            .ok()
            .and_then(|p| p.kept_builds_count)
            .unwrap_or(KEPT_BUILDS as u32) as usize;
        let window = u32::try_from(kept.saturating_mul(2))?;
        let deployments = self.store.deployments(app, window).await?;
        let old = deployments
            .iter()
            .skip(kept)
            .filter(|deployment| deployment.status != DeploymentStatus::Running);
        for deployment in old {
            runtime
                .remove_image(&build_tag(app, deployment.id)?)
                .await?;
        }
        Ok(())
    }

    /// Removes the built images of every listed deployment of an app.
    pub(super) async fn remove_builds(
        &self,
        runtime: &dyn ContainerRuntime,
        app: ShortId,
    ) -> anyhow::Result<()> {
        let deployments = self
            .store
            .deployments(app, super::DEPLOYMENT_LIST_LIMIT)
            .await?;
        for deployment in deployments {
            runtime
                .remove_image(&build_tag(app, deployment.id)?)
                .await?;
        }
        Ok(())
    }
}

async fn try_buildkit(source: &GitBuild, checkout: &Path, tag: &ImageRef) -> Option<String> {
    let config = source.buildkit.as_ref()?;
    if !config.enabled {
        return None;
    }
    let context_path = checkout.join(source.context.as_str());
    let dockerfile_path = checkout.join(source.dockerfile.as_str());
    prepare_buildkit_dockerfile(&dockerfile_path, config).await;
    let runner = crate::build::buildkit::BuildKitRunner::new(None::<String>);
    runner
        .build(&context_path, &dockerfile_path, tag, config)
        .await
        .ok()
}

async fn prepare_buildkit_dockerfile(dockerfile_path: &Path, config: &BuildKitConfig) {
    if config.cache_mounts.is_empty() && config.custom_cache_paths.is_empty() {
        return;
    }
    if let Ok(content) = tokio::fs::read_to_string(dockerfile_path).await {
        let optimized = crate::build::buildkit::inject_cache_mounts(&content, config);
        let _ = tokio::fs::write(dockerfile_path, optimized).await;
    }
}

/// The tag of the image that a deployment builds.
pub(super) fn build_tag(app: ShortId, deployment: ShortId) -> anyhow::Result<ImageRef> {
    Ok(format!("{BUILD_REPOSITORY_PREFIX}{app}:{deployment}").parse()?)
}

pub(super) use crate::build::remove_dir;
