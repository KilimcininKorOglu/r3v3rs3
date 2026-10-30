//! The deploy pipeline of an app on the local target. A deployment is blue-green: the old
//! container serves until the new container passes its health check, then the proxy switches to
//! the new container and the old container stops after a drain period. A Compose app deploys by
//! recreate instead (`compose.rs`).

use super::build::{GitBuild, remove_dir};
use super::compose::{ComposeBuild, ComposeJob, SourceRevision};
use super::notices::DeploymentRef;
use super::store::{NewDeployment, StoredEnv};
use super::{Backend, Platform, not_found, proxy};
use crate::agent::executor::RESOURCE_PREFIX;
use crate::clock::unix_ms;
use crate::notify::NotificationEvent;
use crate::runtime::ContainerRuntime;
use anyhow::{Context as _, anyhow, bail};
use r3v3rs3_api::container::{
    APP_LABEL, AppName, ContainerName, ContainerSpec, EnvVar, ImageInfo, ImageRef, NetworkName,
    RestartPolicy,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::git::GitRef;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, AppSpec, DeploymentEntry, DeploymentStatus, DeploymentTrigger,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use time::OffsetDateTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{error, info, warn};

/// The label that names the deployment of a container.
pub const DEPLOYMENT_LABEL: &str = "r3v3rs3.deployment";

/// The log lines of a failed container that the failure message carries.
const LOG_TAIL: u32 = 20;

/// The longest status line of a health check response.
const MAX_STATUS_LINE: usize = 1024;

/// The durations of a deployment.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// The time that a new container has to pass its health check.
    pub health_timeout: Duration,
    /// The time between two health checks.
    pub health_interval: Duration,
    /// The time of one health check.
    pub probe_timeout: Duration,
    /// The time that an old container keeps running after the switch, so that its requests end.
    pub drain: Duration,
    /// The time that a stopping container has before Docker kills it.
    pub stop_timeout: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            health_timeout: Duration::from_secs(120),
            health_interval: Duration::from_secs(1),
            probe_timeout: Duration::from_secs(5),
            drain: Duration::from_secs(10),
            stop_timeout: Duration::from_secs(10),
        }
    }
}

struct PreviousCompose {
    deployment: ShortId,
    source: ComposeBuild,
    port: u16,
    env: Vec<EnvVar>,
    container: ContainerName,
    targets: Vec<ShortId>,
}

/// What one deployment runs. The pipeline reads only this snapshot, so an edit of the app during
/// the deployment does not change it.
struct Job {
    app: ShortId,
    /// The name and the trigger name the deployment in its notifications.
    app_name: AppName,
    app_target: ShortId,
    trigger: DeploymentTrigger,
    deployment: ShortId,
    /// The runtime and the Compose runner of the target of the app at the start of the
    /// deployment.
    backend: Backend,
    spec: AppSpec,
    env: Vec<StoredEnv>,
    image: JobImage,
}

impl Job {
    fn notice(&self) -> DeploymentRef<'_> {
        DeploymentRef {
            id: self.deployment,
            app: self.app,
            app_name: &self.app_name,
            trigger: self.trigger,
        }
    }
}

/// Where the image of a deployment comes from.
enum JobImage {
    /// The image of the app, or the digest of the deployment that a rollback repeats.
    Pull(ImageRef),
    Build(GitBuild),
    /// The services of a Compose file, which Compose builds and starts itself.
    Compose(ComposeBuild),
}

impl From<AppSource> for JobImage {
    fn from(source: AppSource) -> Self {
        match source {
            AppSource::Image { image } => Self::Pull(image),
            AppSource::Git {
                repository,
                branch,
                context,
                dockerfile,
                connection,
            } => Self::Build(GitBuild {
                repository,
                branch,
                context,
                dockerfile,
                connection,
                buildkit: None,
            }),
            AppSource::Compose {
                repository,
                branch,
                file,
                service,
                connection,
                compose_yaml,
            } => Self::Compose(ComposeBuild {
                repository,
                revision: SourceRevision::Branch(branch),
                file,
                service,
                connection,
                compose_yaml,
            }),
        }
    }
}

impl JobImage {
    pub fn from_spec(spec: &AppSpec) -> Self {
        match &spec.source {
            AppSource::Image { image } => Self::Pull(image.clone()),
            AppSource::Git {
                repository,
                branch,
                context,
                dockerfile,
                connection,
            } => Self::Build(GitBuild {
                repository: repository.clone(),
                branch: branch.clone(),
                context: context.clone(),
                dockerfile: dockerfile.clone(),
                connection: *connection,
                buildkit: spec.buildkit.clone(),
            }),
            AppSource::Compose {
                repository,
                branch,
                file,
                service,
                connection,
                compose_yaml,
            } => Self::Compose(ComposeBuild {
                repository: repository.clone(),
                revision: SourceRevision::Branch(branch.clone()),
                file: file.clone(),
                service: service.clone(),
                connection: *connection,
                compose_yaml: compose_yaml.clone(),
            }),
        }
    }

    /// What a rollback to `source` starts: the image that it ran, or for a Compose app its
    /// commit.
    fn rollback(spec: &AppSpec, source: &DeploymentEntry) -> Result<Self, Error> {
        let unavailable = || Error::RollbackUnavailable { id: source.id };
        let mut image = Self::from(spec.source.clone());
        if let Self::Compose(build) = &mut image {
            if build.repository.is_none() {
                return Ok(image);
            }
            let commit = source
                .commit_sha
                .as_deref()
                .and_then(|sha| sha.parse().ok());
            build.revision = SourceRevision::Commit(commit.ok_or_else(unavailable)?);
            return Ok(image);
        }
        let digest = source.image_digest.as_deref().and_then(|d| d.parse().ok());
        Ok(Self::Pull(digest.ok_or_else(unavailable)?))
    }
}

fn job_image_for_entry(entry: &DeploymentEntry, spec: &AppSpec) -> Result<JobImage, Error> {
    match entry.trigger {
        DeploymentTrigger::Rollback => JobImage::rollback(spec, entry),
        _ => Ok(JobImage::from(spec.source.clone())),
    }
}

/// Holds the app of a running pipeline, so that the app gets one pipeline at a time.
pub struct AppLock {
    platform: Arc<Platform>,
    app: ShortId,
}

impl Drop for AppLock {
    fn drop(&mut self) {
        self.platform.busy_apps().remove(&self.app);
    }
}

impl Platform {
    /// Takes the app for one pipeline or one deletion.
    pub fn lock_app(self: &Arc<Self>, app: &AppEntry) -> Result<AppLock, Error> {
        if !self.busy_apps().insert(app.id) {
            return Err(Error::AppBusy {
                name: app.name.to_string(),
            });
        }
        Ok(AppLock {
            platform: self.clone(),
            app: app.id,
        })
    }

    async fn insert_pending(&self, new: NewDeployment<'_>) -> anyhow::Result<DeploymentEntry> {
        self.store.insert_deployment(&new).await?;
        self.store
            .deployment(new.id)
            .await?
            .context("the new deployment was not stored")
    }

    fn spawn_job(self: &Arc<Self>, lock: AppLock, job: Job) {
        let platform = self.clone();
        tokio::spawn(async move {
            platform.run_job(&job).await;
            drop(lock);
            platform.run_queued_hook(job.app).await;
        });
    }

    /// Starts a deployment of the current spec and environment of an app, and returns it while
    /// it is queued.
    pub async fn deploy(
        self: &Arc<Self>,
        id: ShortId,
        username: &str,
        trigger: DeploymentTrigger,
    ) -> anyhow::Result<DeploymentEntry> {
        self.deploy_with_ref(id, username, trigger, None).await
    }

    async fn sync_image_spec(&self, app: &AppEntry, spec: &AppSpec) -> anyhow::Result<()> {
        if matches!(app.spec.source, AppSource::Image { .. }) && spec.source != app.spec.source {
            let request = r3v3rs3_api::platform::AppRequest {
                name: app.name.clone(),
                target: app.target,
                spec: spec.clone(),
            };
            self.update_app(app.id, request, unix_ms()).await?;
        }
        Ok(())
    }

    /// Starts a deployment of an app, optionally overriding the source branch or tag.
    pub async fn deploy_with_ref(
        self: &Arc<Self>,
        id: ShortId,
        username: &str,
        trigger: DeploymentTrigger,
        git_ref: Option<GitRef>,
    ) -> anyhow::Result<DeploymentEntry> {
        let app = self.app(id).await?;
        let mut spec = app.spec.clone();
        apply_target_ref(&mut spec, git_ref.as_ref());
        self.sync_image_spec(&app, &spec).await?;
        let env = self.store.env(id).await?;
        let new = NewDeployment::new(&app, trigger, username, &spec, &env)?;
        if let Some(reason) = needs_approval(&spec) {
            return self.insert_pending(new.pending(reason)).await;
        }
        let image = JobImage::from_spec(&spec);
        let lock = self.lock_app(&app)?;
        self.start_job(lock, &app, new, image).await
    }

    async fn rollback_source(
        &self,
        id: ShortId,
    ) -> anyhow::Result<(DeploymentEntry, AppSpec, Vec<StoredEnv>)> {
        let source = self.deployment(id).await?;
        let (spec, env) = self
            .store
            .deployment_snapshot(id)
            .await?
            .ok_or_else(|| not_found(id))?;
        Ok((source, spec, env))
    }

    /// Starts a deployment that repeats the image digest, or for a Compose app the commit, the
    /// spec and the environment of an earlier deployment.
    pub async fn rollback(
        self: &Arc<Self>,
        id: ShortId,
        username: &str,
    ) -> anyhow::Result<DeploymentEntry> {
        let (source, spec, env) = self.rollback_source(id).await?;
        let app = self.app(source.app).await?;
        let digest = source.image_digest.as_deref();
        let commit = source.commit_sha.as_deref();
        let new = NewDeployment::new(&app, DeploymentTrigger::Rollback, username, &spec, &env)?
            .with_source_info(digest, commit);
        if let Some(reason) = needs_approval(&app.spec) {
            return self.insert_pending(new.pending(reason)).await;
        }
        let image = JobImage::rollback(&spec, &source)?;
        let lock = self.lock_app(&app)?;
        self.start_job(lock, &app, new, image).await
    }

    /// Approves a pending deployment and starts its pipeline.
    pub async fn approve_deployment(
        self: &Arc<Self>,
        id: ShortId,
        _username: &str,
    ) -> anyhow::Result<DeploymentEntry> {
        let entry = self.deployment(id).await?;
        if entry.status != DeploymentStatus::PendingApproval {
            return Err(Error::DeploymentNotPendingApproval { id }.into());
        }
        let app = self.app(entry.app).await?;
        let lock = self.lock_app(&app)?;
        let (spec, env) = self
            .store
            .deployment_snapshot(id)
            .await?
            .ok_or_else(|| not_found(id))?;
        let image = job_image_for_entry(&entry, &spec)?;
        self.store.approve_deployment(id, unix_ms()).await?;
        let job = Job {
            app: entry.app,
            app_name: app.name.clone(),
            app_target: app.target,
            trigger: entry.trigger,
            deployment: entry.id,
            backend: self.backend(app.target),
            spec,
            env,
            image,
        };
        self.spawn_job(lock, job);
        self.deployment(id).await
    }

    /// Rejects a pending deployment with cancelled status.
    pub async fn reject_deployment(
        self: &Arc<Self>,
        id: ShortId,
        username: &str,
    ) -> anyhow::Result<DeploymentEntry> {
        let entry = self.deployment(id).await?;
        if entry.status != DeploymentStatus::PendingApproval {
            return Err(Error::DeploymentNotPendingApproval { id }.into());
        }
        let message = format!("Rejected by {username}");
        self.store
            .fail_deployment(id, DeploymentStatus::Cancelled, &message, unix_ms())
            .await?;
        self.deployment(id).await
    }

    async fn start_job(
        self: &Arc<Self>,
        lock: AppLock,
        app: &AppEntry,
        new: NewDeployment<'_>,
        image: JobImage,
    ) -> anyhow::Result<DeploymentEntry> {
        self.store.insert_deployment(&new).await?;
        let entry = self
            .store
            .deployment(new.id)
            .await?
            .context("the new deployment was not stored")?;
        let job = Job {
            app: new.app,
            app_name: app.name.clone(),
            app_target: app.target,
            trigger: new.trigger,
            deployment: new.id,
            backend: self.backend(app.target),
            spec: new.spec.clone(),
            env: new.env.to_vec(),
            image,
        };
        self.spawn_job(lock, job);
        Ok(entry)
    }

    async fn run_job(&self, job: &Job) {
        self.run_pipeline(job).await;
        // A failed deployment can leave a built image too, so the cleanup follows every build.
        for target in job.spec.targets_or_default(job.app_target) {
            let runtime = &*self.backend(target).runtime;
            let pruned = match job.spec.source {
                AppSource::Git { .. } => self.prune_builds(runtime, job.app).await,
                AppSource::Compose { .. } => self.prune_compose_images(runtime, job.app).await,
                AppSource::Image { .. } => Ok(()),
            };
            if let Err(err) = pruned {
                error!(app = %job.app, target = %target, "failed to remove the old built images: {err:#}");
            }
        }
    }

    async fn run_pipeline(&self, job: &Job) {
        info!(app = %job.app, deployment = %job.deployment, "deployment started");
        self.notify_deployment(NotificationEvent::DeploymentStarted, job.notice(), None)
            .await;
        let result = match self.start_job_container(job).await {
            Ok(name) => self.switch(job, &name).await,
            Err(err) => Err(err),
        };
        let Err(err) = result else {
            self.run_post_deploy_hook(job).await;
            info!(app = %job.app, deployment = %job.deployment, "deployment finished");
            self.notify_deployment(NotificationEvent::DeploymentRunning, job.notice(), None)
                .await;
            return;
        };
        let message = format!("{err:#}");
        error!(app = %job.app, deployment = %job.deployment, %message, "deployment failed");
        let failed = self
            .store
            .fail_deployment(
                job.deployment,
                DeploymentStatus::Failed,
                &message,
                unix_ms(),
            )
            .await;
        if let Err(err) = failed {
            error!(deployment = %job.deployment, "failed to store the failure: {err:#}");
        }
        // The notification follows the stored state, so a reader of the API sees the failure.
        let event = NotificationEvent::DeploymentFailed;
        self.notify_deployment(event, job.notice(), Some(message))
            .await;
    }

    /// Starts the container that receives the requests of the new deployment, and waits for its
    /// health check.
    async fn start_job_container(&self, job: &Job) -> anyhow::Result<ContainerName> {
        let JobImage::Compose(source) = &job.image else {
            return self.start_container(job).await;
        };
        let name = proxy::container_name(job.app, job.deployment)?;
        let env = self.effective_env(job).await?;
        let previous = self.previous_compose(job.app).await;
        let mut started = false;
        for target in job.spec.targets_or_default(job.app_target) {
            let backend = self.backend(target);
            let compose = ComposeJob {
                runner: &*backend.compose,
                app: job.app,
                deployment: job.deployment,
                port: job.spec.port,
                source,
                env: &env,
                container: &name,
            };
            if let Err(err) = self.compose_up(&compose).await {
                if started {
                    self.rollback_previous_compose(job.app, &previous).await;
                }
                return Err(err);
            }
            started = true;
            if let Err(err) = self.wait_healthy(&*backend.runtime, &name, &job.spec).await {
                self.rollback_previous_compose(job.app, &previous).await;
                let dir = self
                    .compose_dir
                    .join(job.app.to_string())
                    .join(job.deployment.to_string());
                let _ = remove_dir(&dir).await;
                return Err(err);
            }
        }
        Ok(name)
    }

    async fn previous_compose(&self, app: ShortId) -> Option<PreviousCompose> {
        let prev_id = self.store.running_deployment(app).await.ok().flatten()?;
        let prev_entry = self.store.deployment(prev_id).await.ok().flatten()?;
        let (prev_spec, prev_stored_env) =
            self.store.deployment_snapshot(prev_id).await.ok().flatten()?;
        let JobImage::Compose(source) = JobImage::rollback(&prev_spec, &prev_entry).ok()? else {
            return None;
        };
        let container = proxy::container_name(app, prev_id).ok()?;
        let env = self
            .effective_env_for(app, prev_id, &prev_spec, &prev_stored_env)
            .await
            .ok()?;
        let app_entry = self.app(app).await.ok()?;
        let targets = prev_spec.targets_or_default(app_entry.target);
        Some(PreviousCompose {
            deployment: prev_id,
            source,
            port: prev_spec.port,
            env,
            container,
            targets,
        })
    }

    async fn rollback_previous_compose(&self, app: ShortId, previous: &Option<PreviousCompose>) {
        let Some(prev) = previous else {
            return;
        };
        for &target in &prev.targets {
            let backend = self.backend(target);
            let compose = ComposeJob {
                runner: &*backend.compose,
                app,
                deployment: prev.deployment,
                port: prev.port,
                source: &prev.source,
                env: &prev.env,
                container: &prev.container,
            };
            if let Err(err) = self.rollback_compose(&compose).await {
                error!(
                    app = %app,
                    deployment = %prev.deployment,
                    target = %target,
                    "failed to rollback compose deployment: {err:#}"
                );
            }
        }
        self.publish().await;
    }

    /// Starts the new container on all target nodes and waits for health checks.
    async fn start_container(&self, job: &Job) -> anyhow::Result<ContainerName> {
        let digest = self.prepare_image(job).await?;
        self.store.set_image_digest(job.deployment, &digest).await?;
        self.run_pre_deploy_hook(job, &digest).await?;
        let name = proxy::container_name(job.app, job.deployment)?;
        let targets = job.spec.targets_or_default(job.app_target);
        let stopped = self.stop_old_on_targets(job, &targets).await;
        let result = self.deploy_targets(job, &name, &digest, &targets).await;
        if result.is_err() {
            self.restart_stopped(&stopped).await;
        }
        result.map(|()| name)
    }

    async fn deploy_targets(
        &self,
        job: &Job,
        name: &ContainerName,
        digest: &str,
        targets: &[ShortId],
    ) -> anyhow::Result<()> {
        let mut started = Vec::new();
        for &target in targets {
            let backend = self.backend(target);
            self.ensure_target_image(&*backend.runtime, job).await?;
            if let Err(err) = self.run_container(&backend, job, name, digest).await {
                self.cleanup_failed_targets(name, &started).await;
                return Err(err);
            }
            started.push(target);
        }
        Ok(())
    }

    async fn ensure_target_image(
        &self,
        runtime: &dyn ContainerRuntime,
        job: &Job,
    ) -> anyhow::Result<()> {
        if let JobImage::Pull(image) = &job.image {
            self.resolve_image(runtime, image).await?;
        }
        Ok(())
    }

    async fn cleanup_failed_targets(&self, name: &ContainerName, targets: &[ShortId]) {
        for &target in targets {
            let runtime = self.runtime(target);
            if let Err(err) = runtime.remove_container(name).await {
                error!(container = %name, target = %target, "failed to remove a failed container: {err:#}");
            }
        }
    }

    async fn stop_old_on_targets(
        &self,
        job: &Job,
        targets: &[ShortId],
    ) -> Vec<(ShortId, Vec<ContainerName>)> {
        if job.spec.host_port.is_none() {
            return Vec::new();
        }
        let mut stopped = Vec::new();
        for &target in targets {
            let containers = self.stop_old_containers(target, job.app).await;
            stopped.push((target, containers));
        }
        stopped
    }

    async fn stop_old_containers(&self, target: ShortId, app: ShortId) -> Vec<ContainerName> {
        let runtime = self.runtime(target);
        let label = match app_label(app) {
            Ok(label) => label,
            Err(err) => {
                error!(app = %app, "failed to name the old containers: {err:#}");
                return Vec::new();
            }
        };
        let running = match runtime.list_containers(&label).await {
            Ok(containers) => containers
                .into_iter()
                .filter(|container| container.state == "running")
                .filter_map(|container| container.name.parse().ok())
                .collect::<Vec<_>>(),
            Err(err) => {
                error!(app = %app, "failed to list the old containers: {err:#}");
                return Vec::new();
            }
        };
        for container in &running {
            if let Err(err) = runtime
                .stop_container(container, Duration::from_secs(10))
                .await
            {
                error!(container = %container, target = %target, "failed to stop the old container: {err:#}");
            }
        }
        running
    }

    async fn restart_stopped(&self, stopped: &[(ShortId, Vec<ContainerName>)]) {
        for (target, containers) in stopped {
            let runtime = self.runtime(*target);
            for container in containers {
                if let Err(err) = runtime.start_container(container).await {
                    error!(container = %container, target = %target, "failed to start the old container again: {err:#}");
                }
            }
        }
    }

    /// Pulls or builds the image of the deployment, and returns the reference that pins it.
    async fn prepare_image(&self, job: &Job) -> anyhow::Result<String> {
        let deployment = job.deployment;
        match &job.image {
            JobImage::Pull(image) => {
                self.store
                    .set_status(deployment, DeploymentStatus::Deploying)
                    .await?;
                self.resolve_image(&*job.backend.runtime, image).await
            }
            JobImage::Build(source) => {
                self.store
                    .set_status(deployment, DeploymentStatus::Building)
                    .await?;
                let id = self
                    .build(&*job.backend.runtime, job.app, deployment, source)
                    .await?;
                self.store
                    .set_status(deployment, DeploymentStatus::Deploying)
                    .await?;
                Ok(id)
            }
            JobImage::Compose(_) => bail!("a Compose deployment has no image of its own"),
        }
    }

    /// The present image of a pinned reference, which needs no pull.
    async fn present_image(
        runtime: &dyn ContainerRuntime,
        image: &ImageRef,
    ) -> anyhow::Result<Option<ImageInfo>> {
        match runtime.inspect_image(image).await? {
            Some(info) if pinned(image) => Ok(Some(info)),
            // An image id names a built image, which no registry has.
            None if image.as_str().starts_with("sha256:") => {
                bail!("the image {image} is no longer present")
            }
            _ => Ok(None),
        }
    }

    /// Pulls the image unless it is present, and returns the reference that pins it: a
    /// repository digest, or the image id of an image that has none.
    async fn resolve_image(
        &self,
        runtime: &dyn ContainerRuntime,
        image: &ImageRef,
    ) -> anyhow::Result<String> {
        let info = match Self::present_image(runtime, image).await? {
            Some(info) => info,
            None => {
                let auth = self.registry_auth_for_image(image.as_str()).await?;
                runtime.pull_image_with_auth(image, auth.as_ref()).await?;
                runtime
                    .inspect_image(image)
                    .await?
                    .ok_or_else(|| anyhow!("the image {image} is missing after its pull"))?
            }
        };
        let prefix = format!("{}@", image.repository());
        let digest = info
            .repo_digests
            .iter()
            .find(|digest| digest.starts_with(&prefix))
            .or_else(|| info.repo_digests.first())
            .cloned()
            .unwrap_or(info.id);
        Ok(digest)
    }

    async fn run_container(
        &self,
        backend: &Backend,
        job: &Job,
        name: &ContainerName,
        digest: &str,
    ) -> anyhow::Result<()> {
        let runtime = &*backend.runtime;
        let app_label = app_label(job.app)?;
        let network = app_network(job.app)?;
        runtime.ensure_network(&network, &app_label).await?;
        let spec = self
            .build_container_spec(job, name, digest, runtime, &app_label, &network)
            .await?;
        runtime.create_container(&spec).await?;
        runtime.start_container(name).await?;
        let healthy = self.wait_healthy(runtime, name, &job.spec).await;
        if healthy.is_err() {
            let _ = runtime.remove_container(name).await;
        }
        healthy
    }

    async fn build_container_spec(
        &self,
        job: &Job,
        name: &ContainerName,
        digest: &str,
        runtime: &dyn ContainerRuntime,
        app_label: &AppName,
        network: &NetworkName,
    ) -> anyhow::Result<ContainerSpec> {
        let mut env = self.effective_env(job).await?;
        let extra_hosts = if job.spec.uses_database.is_some() {
            env.extend(self.database_env(job, runtime).await?);
            vec![(
                "host.docker.internal".to_string(),
                "host-gateway".to_string(),
            )]
        } else {
            Vec::new()
        };
        Ok(ContainerSpec {
            name: name.clone(),
            image: digest.parse()?,
            env,
            labels: BTreeMap::from([
                (APP_LABEL.to_string(), app_label.to_string()),
                (DEPLOYMENT_LABEL.to_string(), job.deployment.to_string()),
            ]),
            network: Some(network.clone()),
            volumes: job.spec.volumes.clone(),
            restart: job.spec.restart,
            limits: job.spec.limits,
            publish: Some(job.spec.port),
            host_port: job.spec.host_port,
            extra_hosts,
            log_max_size: None,
            log_max_file: None,
            command: None,
        })
    }

    async fn effective_env(&self, job: &Job) -> anyhow::Result<Vec<EnvVar>> {
        self.effective_env_for(job.app, job.deployment, &job.spec, &job.env)
            .await
    }

    async fn effective_env_for(
        &self,
        app: ShortId,
        deployment: ShortId,
        spec: &AppSpec,
        stored_env: &[StoredEnv],
    ) -> anyhow::Result<Vec<EnvVar>> {
        let mut vars = if let Some(config) = &spec.git_config {
            self.fetch_git_config_env(app, deployment, spec, config)
                .await?
        } else {
            Vec::new()
        };
        for &group_id in &spec.env_groups {
            let group_stored = self.store.env_group_vars(group_id).await?;
            let group_vars = self.open_env(group_id, &group_stored)?;
            merge_env_vars(&mut vars, group_vars);
        }
        if let Some(env_id) = spec.environment_id {
            let env_stored = self.store.environment_vars(env_id).await?;
            let env_vars = self.open_env(env_id, &env_stored)?;
            merge_env_vars(&mut vars, env_vars);
        }
        let panel_env = self.open_env(app, stored_env)?;
        merge_env_vars(&mut vars, panel_env);
        Ok(vars)
    }

    async fn fetch_git_config_env(
        &self,
        app: ShortId,
        deployment: ShortId,
        spec: &AppSpec,
        config: &r3v3rs3_api::platform::GitConfigFile,
    ) -> anyhow::Result<Vec<EnvVar>> {
        let (repo, default_branch, connection) = match &config.repository {
            Some(repo) => (
                repo.clone(),
                config.branch.clone().unwrap_or_else(GitRef::main),
                None,
            ),
            None => match &spec.source {
                AppSource::Git {
                    repository,
                    branch,
                    connection,
                    ..
                } => (repository.clone(), branch.clone(), *connection),
                AppSource::Compose {
                    repository: Some(repository),
                    branch,
                    connection,
                    ..
                } => (repository.clone(), branch.clone(), *connection),
                AppSource::Compose {
                    repository: None, ..
                }
                | AppSource::Image { .. } => {
                    bail!("git_config requires repository for image or inline compose apps");
                }
            },
        };
        let branch = config.branch.as_ref().unwrap_or(&default_branch);
        let checkout_dir = self.build_dir.join(format!("git-env-{deployment}"));
        tokio::fs::create_dir_all(&checkout_dir).await?;
        let credential = self.git_credential(app, connection).await?;
        let fetch_res = self
            .fetcher
            .fetch(
                &repo,
                crate::build::Revision::Branch(branch),
                credential.as_ref(),
                &checkout_dir,
            )
            .await;
        let file_path = checkout_dir.join(config.path.as_str());
        let read_res = match fetch_res {
            Ok(_) => tokio::fs::read_to_string(&file_path)
                .await
                .with_context(|| format!("failed to read git config file: {}", config.path)),
            Err(err) => Err(err),
        };
        let _ = tokio::fs::remove_dir_all(&checkout_dir).await;
        let content = read_res?;
        let parsed = r3v3rs3_api::platform::parse_env_file(&content);
        let vars = parsed
            .into_iter()
            .map(|(key, value)| EnvVar { key, value })
            .collect();
        Ok(vars)
    }

    fn open_env(&self, app: ShortId, env: &[StoredEnv]) -> anyhow::Result<Vec<EnvVar>> {
        env.iter()
            .map(|var| {
                Ok(EnvVar {
                    key: var.key.parse()?,
                    value: self.open_value(app, var)?,
                })
            })
            .collect()
    }
}

fn merge_env_vars(target: &mut Vec<EnvVar>, new_vars: Vec<EnvVar>) {
    for var in new_vars {
        if let Some(existing) = target.iter_mut().find(|v| v.key == var.key) {
            existing.value = var.value;
        } else {
            target.push(var);
        }
    }
}

impl Platform {
    /// The connection of the database that the app uses, as the environment of the container.
    /// The container reaches the published port of the database over the host gateway.
    async fn database_env(
        &self,
        job: &Job,
        runtime: &dyn ContainerRuntime,
    ) -> anyhow::Result<Vec<EnvVar>> {
        let Some(db_id) = job.spec.uses_database else {
            return Ok(Vec::new());
        };
        let db = self.app(db_id).await?;
        let Some(engine) = db.spec.database else {
            bail!("the app {} is not a database", db.name);
        };
        let info = r3v3rs3_api::platform::engine_info(&engine)
            .ok_or_else(|| anyhow!("the engine {} is not in the database catalog", engine))?;
        let Some(host_port) = db.spec.host_port else {
            bail!("the database {} has no host port", db.name);
        };
        ensure_database_running(runtime, db_id, &db.name).await?;
        let password = self.open_password(db_id, &info).await?;
        let replicas = self.running_replica_ports(db_id, runtime).await;
        let mut env = Vec::new();
        for (key, value) in connection_values(&info, host_port, password, &replicas) {
            env.push(EnvVar {
                key: key.parse()?,
                value,
            });
        }
        Ok(env)
    }

    /// Opens the password of the database, which its creation stored as a secret variable.
    pub(crate) async fn open_password(
        &self,
        db_id: ShortId,
        info: &r3v3rs3_api::platform::EngineInfo,
    ) -> anyhow::Result<String> {
        let stored = self.store.env(db_id).await?;
        match stored
            .iter()
            .find(|var| var.key.as_str() == info.password_env)
        {
            Some(var) => self.open_value(db_id, var),
            None => Ok(String::new()),
        }
    }

    /// Finds the published ports of all running read replicas of the database.
    async fn running_replica_ports(
        &self,
        db_id: ShortId,
        runtime: &dyn ContainerRuntime,
    ) -> Vec<u16> {
        let Ok(apps) = self.store.apps().await else {
            return Vec::new();
        };
        let mut ports = Vec::new();
        for app in apps {
            if app.spec.replica_of == Some(db_id)
                && let Some(port) = app.spec.host_port
                && ensure_database_running(runtime, app.id, &app.name)
                    .await
                    .is_ok()
            {
                ports.push(port);
            }
        }
        ports
    }

    async fn run_pre_deploy_hook(&self, job: &Job, digest: &str) -> anyhow::Result<()> {
        let Some(hooks) = &job.spec.lifecycle_hooks else {
            return Ok(());
        };
        let Some(cmd) = hooks.pre_deploy.as_deref().filter(|c| !c.trim().is_empty()) else {
            return Ok(());
        };
        info!(app = %job.app, deployment = %job.deployment, command = %cmd, "running pre-deploy hook");
        let backend = self.backend(job.app_target);
        let runtime = &*backend.runtime;
        let hook_name: ContainerName =
            format!("r3v3rs3-hook-{}-{}", job.app, job.deployment).parse()?;
        let app_label = app_label(job.app)?;
        let network = app_network(job.app)?;
        runtime.ensure_network(&network, &app_label).await?;
        let mut spec = self
            .build_container_spec(job, &hook_name, digest, runtime, &app_label, &network)
            .await?;
        spec.publish = None;
        spec.host_port = None;
        spec.restart = RestartPolicy::No;
        spec.command = Some(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            cmd.to_string(),
        ]);
        let _ = runtime.remove_container(&hook_name).await;
        runtime.create_container(&spec).await?;
        runtime.start_container(&hook_name).await?;
        let result = Self::wait_hook_exit(runtime, &hook_name, self.timing.health_timeout).await;
        let _ = runtime.remove_container(&hook_name).await;
        result
    }

    async fn run_post_deploy_hook(&self, job: &Job) {
        let Some(hooks) = &job.spec.lifecycle_hooks else {
            return;
        };
        let Some(cmd) = hooks
            .post_deploy
            .as_deref()
            .filter(|c| !c.trim().is_empty())
        else {
            return;
        };
        let Ok(name) = proxy::container_name(job.app, job.deployment) else {
            return;
        };
        let runtime = &*self.backend(job.app_target).runtime;
        let env = match self.effective_env(job).await {
            Ok(vars) => vars
                .iter()
                .map(|v| format!("{}={}", v.key.as_str(), v.value))
                .collect(),
            Err(_) => Vec::new(),
        };
        let command = vec!["/bin/sh".to_string(), "-c".to_string(), cmd.to_string()];
        info!(app = %job.app, deployment = %job.deployment, command = %cmd, "running post-deploy hook");
        if let Err(err) = runtime.exec_capture(&name, command, env).await {
            warn!(app = %job.app, deployment = %job.deployment, "post-deploy hook failed: {err:#}");
        }
    }

    async fn wait_hook_exit(
        runtime: &dyn ContainerRuntime,
        name: &ContainerName,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let info = runtime
                .inspect_container(name)
                .await?
                .ok_or_else(|| anyhow!("the hook container {name} disappeared"))?;
            if !info.running {
                if info.exit_code != 0 {
                    let log = log_tail(runtime, name).await;
                    bail!("pre-deploy hook exited with code {}{log}", info.exit_code);
                }
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let log = log_tail(runtime, name).await;
                bail!(
                    "pre-deploy hook timed out after {} seconds{log}",
                    timeout.as_secs()
                );
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    /// Waits until the container passes its health check. A stopped container fails at once.
    async fn wait_healthy(
        &self,
        runtime: &dyn ContainerRuntime,
        name: &ContainerName,
        spec: &AppSpec,
    ) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + self.timing.health_timeout;
        loop {
            let info = runtime
                .inspect_container(name)
                .await?
                .ok_or_else(|| anyhow!("the container {name} disappeared"))?;
            if !info.running {
                let log = log_tail(runtime, name).await;
                bail!(
                    "the container stopped with exit code {}{log}",
                    info.exit_code
                );
            }
            let port = info.published.get(&spec.port).copied();
            if let Some(port) = port
                && probe(port, spec.health_check_path.as_deref(), &self.timing).await
            {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let log = log_tail(runtime, name).await;
                bail!(
                    "the app did not pass its health check in {} seconds{log}",
                    self.timing.health_timeout.as_secs()
                );
            }
            tokio::time::sleep(self.timing.health_interval).await;
        }
    }

    /// Routes the app to the new container, marks the deployment running, and removes the old
    /// containers after the drain period. A Compose deployment has no drain period, because
    /// Compose stopped the old container already.
    async fn switch(&self, job: &Job, name: &ContainerName) -> anyhow::Result<()> {
        self.store
            .finish_deployment(job.app, job.deployment, unix_ms())
            .await?;
        self.publish().await;
        let compose = matches!(job.image, JobImage::Compose(_));
        if !compose {
            tokio::time::sleep(self.timing.drain).await;
        }
        for target in job.spec.targets_or_default(job.app_target) {
            let backend = self.backend(target);
            if let Err(err) = self.remove_old(&backend, job.app, name, compose).await {
                // The deployment serves already, so only the log shows the failed cleanup.
                error!(app = %job.app, target = %target, "failed to remove the old containers: {err:#}");
            }
            if compose {
                let app_dir = self.compose_dir.join(job.app.to_string());
                let dir = app_dir.join(job.deployment.to_string());
                if let Err(err) = backend.compose.retain(&app_dir, &dir).await {
                    error!(app = %job.app, target = %target, "failed to remove the old checkouts: {err:#}");
                }
            }
        }
        Ok(())
    }

    /// Removes the containers of the earlier deployments, also those of another source kind
    /// after a change of the app source.
    async fn remove_old(
        &self,
        backend: &Backend,
        app: ShortId,
        keep: &ContainerName,
        compose: bool,
    ) -> anyhow::Result<()> {
        self.remove_containers(&*backend.runtime, app, Some(keep))
            .await?;
        if !compose {
            self.remove_compose_project(backend, app).await?;
        }
        Ok(())
    }

    /// Stops and removes every container of an app except `keep`.
    pub(super) async fn remove_containers(
        &self,
        runtime: &dyn ContainerRuntime,
        app: ShortId,
        keep: Option<&ContainerName>,
    ) -> anyhow::Result<()> {
        let containers = runtime.list_containers(&app_label(app)?).await?;
        for container in containers {
            let name = container
                .name
                .trim_start_matches('/')
                .parse::<ContainerName>()?;
            if Some(&name) == keep {
                continue;
            }
            runtime
                .stop_container(&name, self.timing.stop_timeout)
                .await?;
            runtime.remove_container(&name).await?;
        }
        Ok(())
    }

    /// Removes the containers, the network, the Compose project and the built images of an app.
    pub(super) async fn remove_app_resources(
        &self,
        backend: &Backend,
        app: ShortId,
    ) -> anyhow::Result<()> {
        let runtime = &*backend.runtime;
        self.remove_compose_project(backend, app).await?;
        self.remove_containers(runtime, app, None).await?;
        runtime.remove_network(&app_network(app)?).await?;
        self.remove_builds(runtime, app).await
    }
}

fn needs_approval(spec: &AppSpec) -> Option<&'static str> {
    if spec.require_approval {
        return Some("Waiting for manual approval");
    }
    if let Some(window) = spec.deploy_window.as_deref() {
        let now = OffsetDateTime::now_utc();
        let hour = now.hour() as u32;
        let minute = now.minute() as u32;
        if !r3v3rs3_api::platform::is_within_deploy_window(window, hour, minute) {
            return Some("Outside deploy window");
        }
    }
    None
}

fn apply_target_ref(spec: &mut AppSpec, target_ref: Option<&GitRef>) {
    let Some(target) = target_ref else {
        return;
    };
    match &mut spec.source {
        AppSource::Git { branch, .. } | AppSource::Compose { branch, .. } => {
            *branch = target.clone();
        }
        AppSource::Image { image } => {
            let repo = image.repository();
            if let Ok(new_image) = format!("{repo}:{target}").parse::<ImageRef>() {
                *image = new_image;
            }
        }
    }
}

/// The last lines of the container log, for a failure message.
async fn log_tail(runtime: &dyn ContainerRuntime, name: &ContainerName) -> String {
    match runtime.logs(name, LOG_TAIL).await {
        Ok(log) if !log.trim().is_empty() => format!("\n{}", log.trim_end()),
        Ok(_) => String::new(),
        Err(err) => format!("\nthe log is not readable: {err:#}"),
    }
}

/// Whether a reference names one image version, so that a present image needs no pull.
fn pinned(image: &ImageRef) -> bool {
    image.as_str().contains('@') || image.as_str().starts_with("sha256:")
}

/// The value of the app label. It is the app id, because an app can be renamed.
fn app_label(app: ShortId) -> anyhow::Result<AppName> {
    Ok(app.to_string().parse()?)
}

/// The network of an app, so that the containers of different apps do not reach each other.
fn app_network(app: ShortId) -> anyhow::Result<NetworkName> {
    Ok(format!("{RESOURCE_PREFIX}{app}").parse()?)
}

/// Checks a published port once. With a path, the app must answer the HTTP request with 2xx or
/// 3xx. Without a path, the connection must stay open: the Docker userland proxy accepts a
/// connection before the app listens, and then closes it at once.
async fn probe(port: u16, path: Option<&str>, timing: &Timing) -> bool {
    let check = async {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        match path {
            Some(path) => http_status_ok(&mut stream, path, port).await,
            None => stays_open(&mut stream).await,
        }
    };
    matches!(
        tokio::time::timeout(timing.probe_timeout, check).await,
        Ok(Ok(true))
    )
}

async fn http_status_ok(stream: &mut TcpStream, path: &str, port: u16) -> std::io::Result<bool> {
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: r3v3rs3\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    let mut line = Vec::new();
    let mut buf = [0u8; 256];
    while !line.contains(&b'\n') && line.len() < MAX_STATUS_LINE {
        let read = stream.read(&mut buf).await?;
        if read == 0 {
            break;
        }
        line.extend_from_slice(&buf[..read]);
    }
    let status = String::from_utf8_lossy(&line)
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok());
    Ok(status.is_some_and(|code| (200..400).contains(&code)))
}

async fn stays_open(stream: &mut TcpStream) -> std::io::Result<bool> {
    let mut buf = [0u8; 1];
    match tokio::time::timeout(Duration::from_millis(500), stream.read(&mut buf)).await {
        // The app waits for a request.
        Err(_) => Ok(true),
        // The app sent a banner.
        Ok(Ok(read)) => Ok(read > 0),
        Ok(Err(err)) => Err(err),
    }
}

/// Fails while no container of the database runs.
async fn ensure_database_running(
    runtime: &dyn ContainerRuntime,
    db_id: ShortId,
    name: &AppName,
) -> anyhow::Result<()> {
    let running = runtime
        .list_containers(&app_label(db_id)?)
        .await?
        .iter()
        .any(|container| container.state == "running");
    if running {
        Ok(())
    } else {
        bail!("the database {name} is not running")
    }
}

/// The environment entries of a database connection: the variables of the engine,
/// DATABASE_URL, and read replica URLs, for `host.docker.internal` on the host.
fn connection_values(
    info: &r3v3rs3_api::platform::EngineInfo,
    host_port: u16,
    password: String,
    replicas: &[u16],
) -> Vec<(String, String)> {
    let host = "host.docker.internal";
    let mut values = vec![
        (format!("{}_HOST", info.env_prefix), host.to_string()),
        (format!("{}_PORT", info.env_prefix), host_port.to_string()),
        (format!("{}_USER", info.env_prefix), info.user.clone()),
        (format!("{}_PASSWORD", info.env_prefix), password.clone()),
    ];
    if !info.default_db.is_empty() {
        values.push((
            format!("{}_DATABASE", info.env_prefix),
            info.default_db.clone(),
        ));
    }
    let url = format_url(info, host, host_port, &password);
    values.push(("DATABASE_URL".to_string(), url));
    if !replicas.is_empty() {
        let read_port = replicas[0];
        let read_url = format_url(info, host, read_port, &password);
        values.push(("DATABASE_READ_URL".to_string(), read_url.clone()));
        values.push(("DATABASE_REPLICA_URL".to_string(), read_url));
        values.push((format!("{}_READ_HOST", info.env_prefix), host.to_string()));
        values.push((
            format!("{}_READ_PORT", info.env_prefix),
            read_port.to_string(),
        ));
        let all_urls = replicas
            .iter()
            .map(|&port| format_url(info, host, port, &password))
            .collect::<Vec<_>>()
            .join(",");
        values.push(("DATABASE_READ_URLS".to_string(), all_urls));
    }
    values
}

fn format_url(
    info: &r3v3rs3_api::platform::EngineInfo,
    host: &str,
    port: u16,
    password: &str,
) -> String {
    match (info.user.is_empty(), info.url_database) {
        (true, _) => format!("{}://{host}:{port}", info.url_scheme),
        (false, true) => format!(
            "{}://{}:{password}@{host}:{port}/{}",
            info.url_scheme, info.user, info.default_db
        ),
        (false, false) => format!(
            "{}://{}:{password}@{host}:{port}",
            info.url_scheme, info.user
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::super::build::KEPT_BUILDS;
    use super::super::compose::OVERRIDE_FILE;
    use super::super::fake::{COMMIT, DIGEST, FakeCompose, FakeFetcher, FakeRuntime};
    use super::super::tests::{TempDir, platform_with, request};
    use super::*;
    use crate::build::compose::{ComposePort, DockerCompose};
    use r3v3rs3_api::platform::{AppRequest, EnvEntry, PlatformConfig};

    /// Installs the embedded engine catalog for the tests that connect apps to a database.
    fn catalog() {
        let catalog: r3v3rs3_api::platform::EnginesCatalog =
            serde_json::from_str(include_str!("../../data/engines.json")).unwrap();
        r3v3rs3_api::platform::install_engines(catalog);
    }
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    /// An app server on 127.0.0.1 that answers every request with `status`.
    async fn app_server(status: u16) -> anyhow::Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let response = format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n");
                    let _ = stream.write_all(response.as_bytes()).await;
                });
            }
        });
        Ok(port)
    }

    struct Setup {
        platform: Arc<Platform>,
        runtime: Arc<FakeRuntime>,
        fetcher: Arc<FakeFetcher>,
        compose: Arc<FakeCompose>,
        app: AppEntry,
        dir: TempDir,
    }

    /// A platform with the image app `shop`, whose container answers its health check.
    async fn setup() -> anyhow::Result<Setup> {
        setup_with(request("shop")).await
    }

    /// A platform with the app of `request`, whose container answers its health check.
    async fn setup_with(mut app: AppRequest) -> anyhow::Result<Setup> {
        // The tests read no proxy snapshots.
        let (command, _) = mpsc::channel(1);
        let (mut platform, runtime, dir) =
            platform_with(&PlatformConfig::default(), command).await?;
        let fetcher = Arc::new(FakeFetcher::default());
        platform.fetcher = fetcher.clone();
        let compose = Arc::new(FakeCompose::new(runtime.clone()));
        platform.compose = compose.clone();
        let platform = Arc::new(platform);
        runtime.state().host_port = app_server(200).await?;
        app.spec.health_check_path = Some("/healthz".into());
        let app = platform.add_app(app, 1).await?;
        Ok(Setup {
            platform,
            runtime,
            fetcher,
            compose,
            app,
            dir,
        })
    }

    /// The app `stack` whose service `web` of `compose.yaml` on the branch `release` receives
    /// the requests.
    fn compose_request() -> anyhow::Result<AppRequest> {
        let mut stack = request("stack");
        stack.spec.source = compose_source()?;
        Ok(stack)
    }

    fn compose_source() -> anyhow::Result<AppSource> {
        Ok(AppSource::Compose {
            connection: None,
            repository: Some("https://git.example.com/team/stack.git".parse()?),
            branch: "release".parse()?,
            file: None,
            service: "web".parse()?,
            compose_yaml: None,
        })
    }

    fn project(setup: &Setup) -> String {
        format!("r3v3rs3-{}", setup.app.id)
    }

    fn checkout_dir(setup: &Setup, deployment: &DeploymentEntry) -> std::path::PathBuf {
        let app = setup.platform.compose_dir.join(setup.app.id.to_string());
        app.join(deployment.id.to_string())
    }

    #[tokio::test]
    async fn a_compose_deployment_recreates_the_traffic_service() -> anyhow::Result<()> {
        let setup = setup_with(compose_request()?).await?;
        let first = deploy_mode(&setup, "blue").await?;
        assert_eq!(first.status, DeploymentStatus::Running);
        assert_eq!(first.commit_sha.as_deref(), Some(COMMIT));
        assert!(first.image_digest.is_none());
        let project = project(&setup);
        {
            let compose = setup.compose.state();
            let calls = [format!("config {project}"), format!("up {project}")];
            assert_eq!(compose.calls, calls);
            assert_eq!(compose.env, [("MODE".to_string(), "blue".to_string())]);
        }
        assert_eq!(setup.runtime.names(), [container_of(&setup, &first)]);
        let written = std::fs::read(checkout_dir(&setup, &first).join(OVERRIDE_FILE))?;
        assert!(!String::from_utf8(written)?.contains("blue"));

        let second = deploy_mode(&setup, "green").await?;
        assert_eq!(second.status, DeploymentStatus::Running);
        assert_eq!(setup.runtime.names(), [container_of(&setup, &second)]);
        let first = setup.platform.deployment(first.id).await?;
        assert_eq!(first.status, DeploymentStatus::Superseded);
        assert!(!checkout_dir(&setup, &first).exists());
        assert!(
            checkout_dir(&setup, &second)
                .join("src/compose.yaml")
                .exists()
        );
        assert!(setup.runtime.state().pruned.contains(&(project, false)));
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_compose_deployment_rolls_back_to_previous_checkout_and_container(
    ) -> anyhow::Result<()> {
        let setup = setup_with(compose_request()?).await?;
        let first = deploy_mode(&setup, "blue").await?;
        assert_eq!(first.status, DeploymentStatus::Running);
        assert_eq!(setup.runtime.names(), [container_of(&setup, &first)]);
        assert!(checkout_dir(&setup, &first).exists());

        setup.runtime.state().host_port = app_server(503).await?;
        let failed = deploy_mode(&setup, "green").await?;
        assert_eq!(failed.status, DeploymentStatus::Failed);

        let first = setup.platform.deployment(first.id).await?;
        assert_eq!(first.status, DeploymentStatus::Running);
        assert_eq!(setup.runtime.names(), [container_of(&setup, &first)]);
        assert!(checkout_dir(&setup, &first).exists());
        assert!(!checkout_dir(&setup, &failed).exists());
        Ok(())
    }

    #[tokio::test]
    async fn a_compose_file_that_publishes_a_port_fails_before_up() -> anyhow::Result<()> {
        let setup = setup_with(compose_request()?).await?;
        let published = ComposePort {
            target: 5432,
            published: Some("5432".into()),
            host_ip: None,
        };
        setup.compose.state().extra_ports = vec![("db".into(), published)];
        let failed = deploy(&setup).await?;
        assert_eq!(failed.status, DeploymentStatus::Failed);
        let message = failed.message.clone().context("message")?;
        assert!(
            message.contains("the service db publishes its port 5432"),
            "{message}"
        );
        assert_eq!(setup.compose.state().calls.len(), 1);
        assert!(!checkout_dir(&setup, &failed).exists());

        setup.compose.state().extra_ports.clear();
        setup.compose.state().up_error = Some("pull access denied".into());
        let failed = deploy(&setup).await?;
        let message = failed.message.context("message")?;
        assert!(message.contains("pull access denied"), "{message}");
        assert!(setup.runtime.names().is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_compose_rollback_checks_out_the_old_commit() -> anyhow::Result<()> {
        let setup = setup_with(compose_request()?).await?;
        let first = deploy_mode(&setup, "blue").await?;
        deploy_mode(&setup, "green").await?;
        let rollback = setup.platform.rollback(first.id, "bob").await?;
        let rollback = finished(&setup, rollback.id).await?;
        assert_eq!(rollback.status, DeploymentStatus::Running);
        assert_eq!(rollback.commit_sha, first.commit_sha);
        let revisions = setup
            .fetcher
            .state()
            .fetched
            .iter()
            .map(|(_, revision, _)| revision.clone())
            .collect::<Vec<_>>();
        assert_eq!(revisions, ["release", "release", COMMIT]);
        assert_eq!(setup.compose.state().env[0].1, "blue");

        setup.fetcher.state().error = Some("repository not found".into());
        let failed = deploy(&setup).await?;
        assert!(failed.commit_sha.is_none());
        let err = setup.platform.rollback(failed.id, "bob").await.unwrap_err();
        assert!(matches!(
            err.downcast::<Error>()?,
            Error::RollbackUnavailable { .. }
        ));
        Ok(())
    }

    #[tokio::test]
    async fn deleting_a_compose_app_takes_its_project_down() -> anyhow::Result<()> {
        let setup = setup_with(compose_request()?).await?;
        deploy(&setup).await?;
        setup.platform.delete_app(setup.app.id).await?;
        let project = project(&setup);
        assert!(
            setup
                .compose
                .state()
                .calls
                .contains(&format!("down {project}"))
        );
        assert!(setup.runtime.state().pruned.contains(&(project, true)));
        assert!(setup.runtime.names().is_empty());
        let app_dir = setup.platform.compose_dir.join(setup.app.id.to_string());
        assert!(!app_dir.exists());
        Ok(())
    }

    #[tokio::test]
    async fn a_new_source_kind_removes_the_containers_of_the_old_one() -> anyhow::Result<()> {
        let setup = setup().await?;
        deploy(&setup).await?;
        let mut app = request("shop");
        app.spec.health_check_path = Some("/healthz".into());
        app.spec.source = compose_source()?;
        setup
            .platform
            .update_app(setup.app.id, app.clone(), 2)
            .await?;
        let stack = deploy(&setup).await?;
        assert_eq!(setup.runtime.names(), [container_of(&setup, &stack)]);

        app.spec.source = request("shop").spec.source;
        setup.platform.update_app(setup.app.id, app, 3).await?;
        let image = deploy(&setup).await?;
        assert_eq!(setup.runtime.names(), [container_of(&setup, &image)]);
        let down = format!("down {}", project(&setup));
        assert!(setup.compose.state().calls.contains(&down));
        Ok(())
    }

    /// A Compose file whose traffic service publishes a port itself, which the override replaces.
    /// The image comes from the repository's GHCR mirror, which `mirror-images.yml` fills, so
    /// the CI runners stay below the unauthenticated Docker Hub pull rate limit.
    const DOCKER_COMPOSE_FILE: &str = "services:\n  web:\n    image: ghcr.io/kilimcininkoroglu/r3v3rs3/whoami:v1.10.3\n    ports: [\"18080:80\"]\n  sidecar:\n    image: ghcr.io/kilimcininkoroglu/r3v3rs3/whoami:v1.10.3\n";

    /// The app `stack` on the real Docker Engine, with a checkout that holds
    /// [`DOCKER_COMPOSE_FILE`].
    async fn docker_setup() -> anyhow::Result<Setup> {
        let (command, _) = mpsc::channel(1);
        let (mut platform, runtime, dir) =
            platform_with(&PlatformConfig::default(), command).await?;
        let host = std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into());
        platform.local = Arc::new(super::super::docker_runtime(&host)?);
        platform.compose = Arc::new(DockerCompose::new(host));
        platform.timing.health_timeout = Duration::from_secs(60);
        let fetcher = Arc::new(FakeFetcher::default());
        fetcher.state().compose_file = Some(DOCKER_COMPOSE_FILE.into());
        platform.fetcher = fetcher.clone();
        let compose = Arc::new(FakeCompose::new(runtime.clone()));
        let platform = Arc::new(platform);
        let mut app = compose_request()?;
        app.spec.health_check_path = Some("/health".into());
        let app = platform.add_app(app, 1).await?;
        Ok(Setup {
            platform,
            runtime,
            fetcher,
            compose,
            app,
            dir,
        })
    }

    #[tokio::test]
    #[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
    async fn a_compose_app_runs_on_the_docker_engine() -> anyhow::Result<()> {
        let setup = docker_setup().await?;
        let result = compose_on_docker(&setup).await;
        let deleted = setup.platform.delete_app(setup.app.id).await;
        result?;
        deleted?;
        let app_dir = setup.platform.compose_dir.join(setup.app.id.to_string());
        assert!(!app_dir.exists());
        Ok(())
    }

    async fn compose_on_docker(setup: &Setup) -> anyhow::Result<()> {
        let blue = deploy_mode(setup, "blue").await?;
        anyhow::ensure!(blue.status == DeploymentStatus::Running, "{blue:?}");
        check_whoami(setup, &blue, "MODE=blue").await?;
        let green = deploy_mode(setup, "green").await?;
        anyhow::ensure!(green.status == DeploymentStatus::Running, "{green:?}");
        check_whoami(setup, &green, "MODE=green").await?;
        let blue = container_of(setup, &blue).parse()?;
        anyhow::ensure!(
            setup
                .platform
                .local
                .inspect_container(&blue)
                .await?
                .is_none()
        );
        Ok(())
    }

    /// Checks that the traffic container publishes only its port on 127.0.0.1, and that the
    /// variable of the app reached it.
    async fn check_whoami(
        setup: &Setup,
        deployment: &DeploymentEntry,
        expected: &str,
    ) -> anyhow::Result<()> {
        let name = container_of(setup, deployment).parse()?;
        let info = setup
            .platform
            .local
            .inspect_container(&name)
            .await?
            .context("the traffic container")?;
        anyhow::ensure!(info.published.len() == 1, "{:?}", info.published);
        let port = info.published[&80];
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        stream
            .write_all(b"GET /?env=true HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .await?;
        let mut reply = String::new();
        stream.read_to_string(&mut reply).await?;
        anyhow::ensure!(reply.contains(expected), "{reply}");
        Ok(())
    }

    /// The app `site` that builds `app/Dockerfile` of the branch `release`.
    fn git_request() -> anyhow::Result<AppRequest> {
        let mut site = request("site");
        site.spec.source = AppSource::Git {
            connection: None,
            repository: "https://git.example.com/team/site.git".parse()?,
            branch: "release".parse()?,
            context: "app".parse()?,
            dockerfile: "Dockerfile".parse()?,
        };
        Ok(site)
    }

    /// The tags of the images that the deployments of the app built.
    fn built_tags(setup: &Setup) -> Vec<String> {
        let prefix = format!("r3v3rs3/{}:", setup.app.id);
        let state = setup.runtime.state();
        state
            .images
            .keys()
            .filter(|name| name.starts_with(&prefix))
            .cloned()
            .collect()
    }

    fn tar_entries(archive: &[u8]) -> anyhow::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in tar::Archive::new(archive).entries()? {
            names.push(entry?.path()?.to_string_lossy().into_owned());
        }
        Ok(names)
    }

    #[tokio::test]
    async fn a_git_deployment_builds_its_image_from_the_checkout() -> anyhow::Result<()> {
        let setup = setup_with(git_request()?).await?;
        let deployment = deploy(&setup).await?;
        assert_eq!(deployment.status, DeploymentStatus::Running);
        assert_eq!(deployment.commit_sha.as_deref(), Some(COMMIT));
        assert_eq!(
            setup.fetcher.state().fetched,
            [(
                "https://git.example.com/team/site.git".to_string(),
                "release".to_string(),
                None
            )]
        );
        let state = setup.runtime.state();
        let [build] = state.builds.as_slice() else {
            bail!("one build expected");
        };
        assert_eq!(
            build.tag,
            format!("r3v3rs3/{}:{}", setup.app.id, deployment.id)
        );
        assert_eq!(build.dockerfile, "Dockerfile");
        let entries = tar_entries(&build.context)?;
        assert!(
            entries.iter().any(|name| name == "Dockerfile"),
            "{entries:?}"
        );
        assert!(
            !entries.iter().any(|name| name.contains(".git")),
            "{entries:?}"
        );
        assert_eq!(
            deployment.image_digest.as_deref(),
            Some(state.images[&build.tag].id.as_str())
        );
        assert!(
            !setup
                .platform
                .build_dir
                .join(deployment.id.to_string())
                .exists()
        );
        Ok(())
    }

    /// The tokens of the fetches, in their order.
    fn fetched_tokens(setup: &Setup) -> Vec<Option<String>> {
        let state = setup.fetcher.state();
        state
            .fetched
            .iter()
            .map(|(_, _, token)| token.clone())
            .collect()
    }

    const TOKEN: &str = "ghp_s3cr3tT0ken";

    /// Whether the database file holds `needle` in plain text.
    fn database_contains(setup: &Setup, needle: &str) -> anyhow::Result<bool> {
        let database = std::fs::read(setup.dir.0.join(super::super::DATABASE_FILE))?;
        let needle = needle.as_bytes();
        Ok(database.windows(needle.len()).any(|w| w == needle))
    }

    #[tokio::test]
    async fn a_git_token_is_one_word_of_printable_ascii() -> anyhow::Result<()> {
        let setup = setup_with(git_request()?).await?;
        for invalid in ["", "two words", "line\nbreak", &"x".repeat(4097)] {
            let result = setup.platform.set_git_token(setup.app.id, invalid).await;
            let err = result.unwrap_err().downcast::<Error>()?;
            assert!(matches!(err, Error::InvalidContainerSpec { .. }));
        }
        Ok(())
    }

    #[tokio::test]
    async fn the_git_token_reaches_only_the_clone() -> anyhow::Result<()> {
        let setup = setup_with(git_request()?).await?;
        let app = setup.platform.set_git_token(setup.app.id, TOKEN).await?;
        assert!(app.git_token_set);
        assert!(!serde_json::to_string(&app)?.contains(TOKEN));
        // The database holds only the sealed token.
        assert!(!database_contains(&setup, TOKEN)?);
        deploy(&setup).await?;

        let app = setup.platform.delete_git_token(setup.app.id).await?;
        assert!(!app.git_token_set);
        deploy(&setup).await?;
        let credential = format!("x-access-token:{TOKEN}");
        assert_eq!(fetched_tokens(&setup), [Some(credential), None]);
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_build_fails_the_deployment_without_a_container() -> anyhow::Result<()> {
        let setup = setup_with(git_request()?).await?;
        setup.runtime.state().build_error = Some("RUN make exited with 2".into());
        let failed = deploy(&setup).await?;
        assert_eq!(failed.status, DeploymentStatus::Failed);
        let message = failed.message.context("message")?;
        assert!(message.contains("RUN make exited with 2"), "{message}");
        assert_eq!(failed.commit_sha.as_deref(), Some(COMMIT));

        setup.runtime.state().build_error = None;
        setup.fetcher.state().error = Some("repository not found".into());
        let failed = deploy(&setup).await?;
        let message = failed.message.context("message")?;
        assert!(message.contains("repository not found"), "{message}");
        assert!(failed.commit_sha.is_none());
        assert!(setup.runtime.names().is_empty());
        assert!(
            !setup
                .platform
                .build_dir
                .join(failed.id.to_string())
                .exists()
        );
        Ok(())
    }

    #[tokio::test]
    async fn only_the_newest_built_images_stay() -> anyhow::Result<()> {
        let setup = setup_with(git_request()?).await?;
        let first = deploy(&setup).await?;
        let mut last = first.clone();
        for _ in 0..KEPT_BUILDS + 1 {
            last = deploy(&setup).await?;
        }
        let tags = built_tags(&setup);
        assert_eq!(tags.len(), KEPT_BUILDS, "{tags:?}");
        assert!(tags.contains(&format!("r3v3rs3/{}:{}", setup.app.id, last.id)));

        // The image of the first deployment is gone, so its rollback fails.
        let rollback = setup.platform.rollback(first.id, "bob").await?;
        let rollback = finished(&setup, rollback.id).await?;
        assert_eq!(rollback.status, DeploymentStatus::Failed);
        let message = rollback.message.context("message")?;
        assert!(message.contains("no longer present"), "{message}");

        setup.platform.delete_app(setup.app.id).await?;
        assert!(built_tags(&setup).is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn a_rollback_starts_a_kept_build_without_a_new_build() -> anyhow::Result<()> {
        let setup = setup_with(git_request()?).await?;
        let first = deploy(&setup).await?;
        deploy(&setup).await?;
        let rollback = setup.platform.rollback(first.id, "bob").await?;
        let rollback = finished(&setup, rollback.id).await?;
        assert_eq!(rollback.status, DeploymentStatus::Running);
        assert_eq!(rollback.image_digest, first.image_digest);
        assert_eq!(setup.runtime.state().builds.len(), 2);
        Ok(())
    }

    /// Waits until the pipeline of the app has ended, and returns the deployment. The deadline
    /// covers a real Docker Engine, which pulls and starts containers on a slow CI runner.
    async fn finished(setup: &Setup, id: ShortId) -> anyhow::Result<DeploymentEntry> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        while tokio::time::Instant::now() < deadline {
            let busy = setup.platform.busy_apps().contains(&setup.app.id);
            let entry = setup
                .platform
                .store
                .deployment(id)
                .await?
                .context("entry")?;
            if !busy {
                return Ok(entry);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        bail!("the deployment {id} did not end")
    }

    async fn deploy(setup: &Setup) -> anyhow::Result<DeploymentEntry> {
        let entry = setup
            .platform
            .deploy(setup.app.id, "alice", DeploymentTrigger::Manual)
            .await?;
        assert_eq!(entry.status, DeploymentStatus::Queued);
        finished(setup, entry.id).await
    }

    fn container_of(setup: &Setup, deployment: &DeploymentEntry) -> String {
        format!("r3v3rs3-{}-{}", setup.app.id, deployment.id)
    }

    /// Waits until no app of `apps` runs a pipeline, and returns the deployment.
    async fn settled(
        platform: &Arc<Platform>,
        apps: &[ShortId],
        id: ShortId,
    ) -> anyhow::Result<DeploymentEntry> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        while tokio::time::Instant::now() < deadline {
            let busy = apps.iter().any(|app| platform.busy_apps().contains(app));
            let entry = platform.store.deployment(id).await?.context("entry")?;
            if !busy {
                return Ok(entry);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        bail!("the deployment {id} did not end")
    }

    /// A platform with the fake fetcher, the fake Compose and a healthy health check port.
    async fn setup_platform() -> anyhow::Result<(Arc<Platform>, Arc<FakeRuntime>, TempDir)> {
        let (command, _) = mpsc::channel(1);
        let (mut platform, runtime, dir) =
            platform_with(&PlatformConfig::default(), command).await?;
        platform.fetcher = Arc::new(FakeFetcher::default());
        platform.compose = Arc::new(FakeCompose::new(runtime.clone()));
        let platform = Arc::new(platform);
        runtime.state().host_port = app_server(200).await?;
        Ok((platform, runtime, dir))
    }

    /// A platform whose database app `main-db` runs with a secret password, and the app
    /// `app-db` that uses it.
    async fn setup_with_database()
    -> anyhow::Result<(Arc<Platform>, Arc<FakeRuntime>, AppEntry, TempDir, u16)> {
        catalog();
        let (platform, runtime, dir) = setup_platform().await?;

        let mut db = request("main-db");
        db.spec.port = 5432;
        db.spec.database = Some("postgres".to_string());
        let host_port = std::net::TcpListener::bind(("127.0.0.1", 0))?
            .local_addr()?
            .port();
        db.spec.host_port = Some(host_port);
        db.spec.health_check_path = Some("/healthz".into());
        let db_app = platform.add_app(db, 1).await?;
        platform
            .set_env(
                db_app.id,
                vec![EnvEntry {
                    key: "POSTGRES_PASSWORD".parse()?,
                    value: Some("s3cret".into()),
                    secret: true,
                }],
            )
            .await?;
        let db_entry = platform
            .deploy(db_app.id, "alice", DeploymentTrigger::Manual)
            .await?;
        settled(&platform, &[db_app.id], db_entry.id).await?;

        let mut app = request("app-db");
        app.spec.uses_database = Some(db_app.id);
        app.spec.health_check_path = Some("/healthz".into());
        let linked = platform.add_app(app, 2).await?;
        Ok((platform, runtime, linked, dir, host_port))
    }

    #[tokio::test]
    async fn a_deployment_injects_the_connection_of_its_database() -> anyhow::Result<()> {
        let (platform, runtime, linked, _dir, host_port) = setup_with_database().await?;
        let db_id = linked.spec.uses_database.context("the database")?;
        let entry = platform
            .deploy(linked.id, "alice", DeploymentTrigger::Manual)
            .await?;
        let entry = settled(&platform, &[db_id, linked.id], entry.id).await?;
        assert_eq!(entry.status, DeploymentStatus::Running);

        let created = runtime
            .state()
            .specs
            .last()
            .cloned()
            .context("the created container")?;
        assert_eq!(
            created.extra_hosts,
            vec![(
                "host.docker.internal".to_string(),
                "host-gateway".to_string()
            )]
        );
        let env = |key: &str| {
            created
                .env
                .iter()
                .find(|var| var.key.as_str() == key)
                .map(|var| var.value.clone())
        };
        assert_eq!(
            env("DATABASE_URL").as_deref(),
            Some(&*format!(
                "postgres://postgres:s3cret@host.docker.internal:{host_port}/postgres"
            ))
        );
        assert_eq!(
            env("POSTGRES_HOST").as_deref(),
            Some("host.docker.internal")
        );
        assert_eq!(
            env("POSTGRES_PORT").as_deref(),
            Some(&*host_port.to_string())
        );
        assert_eq!(env("POSTGRES_USER").as_deref(), Some("postgres"));
        assert_eq!(env("POSTGRES_PASSWORD").as_deref(), Some("s3cret"));
        assert_eq!(env("POSTGRES_DATABASE").as_deref(), Some("postgres"));

        // A database that an app uses cannot be deleted.
        let err = platform.delete_app(db_id).await.unwrap_err();
        let err = err.downcast::<Error>().context("an api error")?;
        assert!(matches!(err, Error::DatabaseInUse { .. }), "{err}");
        Ok(())
    }

    async fn add_and_deploy_replica(
        platform: &Arc<Platform>,
        db_id: ShortId,
    ) -> anyhow::Result<(ShortId, u16)> {
        let replica_port = std::net::TcpListener::bind(("127.0.0.1", 0))?
            .local_addr()?
            .port();
        let mut replica_req = request("db-replica");
        replica_req.spec.host_port = Some(replica_port);
        replica_req.spec.replica_of = Some(db_id);
        let replica = platform.add_app(replica_req, 3).await?;
        let rep_dep = platform
            .deploy(replica.id, "alice", DeploymentTrigger::Manual)
            .await?;
        settled(platform, &[db_id, replica.id], rep_dep.id).await?;
        Ok((replica.id, replica_port))
    }

    #[tokio::test]
    async fn a_deployment_injects_read_replicas_connection_urls() -> anyhow::Result<()> {
        let (platform, runtime, linked, _dir, host_port) = setup_with_database().await?;
        let db_id = linked.spec.uses_database.context("the database")?;
        let (rep_id, replica_port) = add_and_deploy_replica(&platform, db_id).await?;

        let entry = platform
            .deploy(linked.id, "alice", DeploymentTrigger::Manual)
            .await?;
        let entry = settled(&platform, &[db_id, rep_id, linked.id], entry.id).await?;
        assert_eq!(entry.status, DeploymentStatus::Running);

        let created = runtime
            .state()
            .specs
            .last()
            .cloned()
            .context("the created container")?;

        let env = |key: &str| {
            created
                .env
                .iter()
                .find(|var| var.key.as_str() == key)
                .map(|var| var.value.clone())
        };

        assert_eq!(
            env("DATABASE_URL").as_deref(),
            Some(&*format!(
                "postgres://postgres:s3cret@host.docker.internal:{host_port}/postgres"
            ))
        );
        assert_eq!(
            env("DATABASE_READ_URL").as_deref(),
            Some(&*format!(
                "postgres://postgres:s3cret@host.docker.internal:{replica_port}/postgres"
            ))
        );
        assert_eq!(
            env("DATABASE_READ_URLS").as_deref(),
            Some(&*format!(
                "postgres://postgres:s3cret@host.docker.internal:{replica_port}/postgres"
            ))
        );
        assert_eq!(
            env("POSTGRES_READ_PORT").as_deref(),
            Some(&*replica_port.to_string())
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_deployment_of_a_connected_app_fails_while_the_database_is_down() -> anyhow::Result<()>
    {
        let (platform, runtime, linked, _dir, _host_port) = setup_with_database().await?;
        let db_id = linked.spec.uses_database.context("the database")?;
        let db_entry = platform
            .store
            .deployments(db_id, 1)
            .await?
            .into_iter()
            .next()
            .context("the database deployment")?;
        let db_container: ContainerName = format!("r3v3rs3-{db_id}-{}", db_entry.id).parse()?;
        runtime
            .stop_container(&db_container, Duration::from_secs(1))
            .await?;

        let entry = platform
            .deploy(linked.id, "alice", DeploymentTrigger::Manual)
            .await?;
        let entry = settled(&platform, &[db_id, linked.id], entry.id).await?;
        assert_eq!(entry.status, DeploymentStatus::Failed);
        let message = entry.message.as_deref().unwrap_or_default();
        assert!(message.contains("is not running"), "{entry:?}");
        Ok(())
    }

    #[tokio::test]
    async fn a_deployment_replaces_the_running_container() -> anyhow::Result<()> {
        let setup = setup().await?;
        let first = deploy(&setup).await?;
        assert_eq!(first.status, DeploymentStatus::Running);
        assert_eq!(
            first.image_digest.as_deref(),
            Some(&*format!("nginx@{DIGEST}"))
        );
        assert_eq!(setup.runtime.names(), [container_of(&setup, &first)]);

        let second = deploy(&setup).await?;
        assert_eq!(second.status, DeploymentStatus::Running);
        let first = setup
            .platform
            .store
            .deployment(first.id)
            .await?
            .context("first")?;
        assert_eq!(first.status, DeploymentStatus::Superseded);
        assert_eq!(setup.runtime.names(), [container_of(&setup, &second)]);

        let state = setup.runtime.state();
        let info = &state.containers[&container_of(&setup, &second)];
        assert_eq!(info.labels[APP_LABEL], setup.app.id.to_string());
        assert_eq!(info.labels[DEPLOYMENT_LABEL], second.id.to_string());
        assert!(
            state
                .networks
                .contains(&format!("r3v3rs3-{}", setup.app.id))
        );
        Ok(())
    }

    #[tokio::test]
    async fn the_log_comes_from_the_running_container() -> anyhow::Result<()> {
        let setup = setup().await?;
        let log = setup.platform.app_log(setup.app.id, 10, None).await?;
        assert_eq!(log.log, "");
        assert!(!log.running);

        let deployment = deploy(&setup).await?;
        let log = setup.platform.app_log(setup.app.id, 10, None).await?;
        assert!(log.running);
        assert_eq!(
            log.log,
            format!("the log of {}\n", container_of(&setup, &deployment))
        );

        let missing = setup.platform.app_log(ShortId::new(), 10, None).await;
        assert!(missing.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_deployment_keeps_the_old_container() -> anyhow::Result<()> {
        let setup = setup().await?;
        let first = deploy(&setup).await?;

        setup.runtime.state().crash = true;
        let crashed = deploy(&setup).await?;
        assert_eq!(crashed.status, DeploymentStatus::Failed);
        let message = crashed.message.context("message")?;
        assert!(message.contains("exit code 1"), "{message}");
        assert!(message.contains("the log of"), "{message}");

        setup.runtime.state().crash = false;
        setup.runtime.state().host_port = app_server(503).await?;
        let unhealthy = deploy(&setup).await?;
        assert_eq!(unhealthy.status, DeploymentStatus::Failed);
        let message = unhealthy.message.context("message")?;
        assert!(message.contains("health check"), "{message}");

        let first = setup
            .platform
            .store
            .deployment(first.id)
            .await?
            .context("first")?;
        assert_eq!(first.status, DeploymentStatus::Running);
        assert_eq!(setup.runtime.names(), [container_of(&setup, &first)]);
        Ok(())
    }

    /// Deploys the app with `MODE` set to `value`.
    async fn deploy_mode(setup: &Setup, value: &str) -> anyhow::Result<DeploymentEntry> {
        let env = EnvEntry {
            key: "MODE".parse()?,
            value: Some(value.into()),
            secret: true,
        };
        setup.platform.set_env(setup.app.id, vec![env]).await?;
        deploy(setup).await
    }

    #[tokio::test]
    async fn a_rollback_repeats_the_digest_and_the_environment() -> anyhow::Result<()> {
        let setup = setup().await?;
        let first = deploy_mode(&setup, "blue").await?;
        deploy_mode(&setup, "green").await?;

        // A rollback does not pull a present digest.
        setup.runtime.state().missing_image = Some("nginx".into());
        let rollback = setup.platform.rollback(first.id, "bob").await?;
        let rollback = finished(&setup, rollback.id).await?;
        assert_eq!(rollback.status, DeploymentStatus::Running);
        assert_eq!(rollback.trigger, DeploymentTrigger::Rollback);
        assert_eq!(rollback.image_digest, first.image_digest);
        let (_, sealed) = setup
            .platform
            .store
            .deployment_snapshot(rollback.id)
            .await?
            .context("snapshot")?;
        let opened = setup.platform.open_env(setup.app.id, &sealed)?;
        assert_eq!(opened[0].value, "blue");
        Ok(())
    }

    #[tokio::test]
    async fn a_deployment_without_an_image_cannot_be_rolled_back() -> anyhow::Result<()> {
        let setup = setup().await?;
        setup.runtime.state().missing_image = Some("nginx".into());
        let failed = deploy(&setup).await?;
        assert_eq!(failed.status, DeploymentStatus::Failed);
        assert!(failed.image_digest.is_none());
        let err = setup.platform.rollback(failed.id, "bob").await.unwrap_err();
        let err = err.downcast::<Error>()?;
        assert!(matches!(err, Error::RollbackUnavailable { .. }));
        Ok(())
    }

    #[tokio::test]
    async fn an_app_runs_one_pipeline_at_a_time() -> anyhow::Result<()> {
        let setup = setup().await?;
        let lock = setup.platform.lock_app(&setup.app)?;
        let deploy = setup
            .platform
            .deploy(setup.app.id, "alice", DeploymentTrigger::Manual)
            .await;
        let delete = setup.platform.delete_app(setup.app.id).await;
        for result in [deploy.map(|_| ()), delete.map(|_| ())] {
            let err = result.unwrap_err().downcast::<Error>()?;
            assert!(matches!(err, Error::AppBusy { .. }));
        }
        assert!(setup.platform.deployments(setup.app.id).await?.is_empty());
        drop(lock);
        assert!(setup.platform.lock_app(&setup.app).is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn deleting_an_app_removes_its_containers_and_its_network() -> anyhow::Result<()> {
        let setup = setup().await?;
        deploy(&setup).await?;
        setup.platform.delete_app(setup.app.id).await?;
        assert!(setup.runtime.names().is_empty());
        assert!(setup.runtime.state().networks.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn only_a_deployed_app_needs_docker_for_its_deletion() -> anyhow::Result<()> {
        let setup = setup().await?;
        let blog = setup.platform.add_app(request("blog"), 2).await?;
        deploy(&setup).await?;
        setup.runtime.state().broken = true;
        setup.platform.delete_app(blog.id).await?;
        assert!(setup.platform.delete_app(setup.app.id).await.is_err());
        assert_eq!(setup.platform.apps().await?, vec![setup.app.clone()]);
        Ok(())
    }

    #[tokio::test]
    async fn a_probe_without_a_path_needs_an_open_connection() -> anyhow::Result<()> {
        let timing = Timing::default();
        let port = app_server(200).await?;
        assert!(probe(port, None, &timing).await);
        // A listener that closes every connection, like the Docker userland proxy without an app.
        let closing = TcpListener::bind("127.0.0.1:0").await?;
        let closing_port = closing.local_addr()?.port();
        tokio::spawn(async move {
            while let Ok((stream, _)) = closing.accept().await {
                drop(stream);
            }
        });
        assert!(!probe(closing_port, None, &timing).await);
        assert!(probe(port, Some("/"), &timing).await);
        Ok(())
    }

    #[tokio::test]
    async fn an_app_with_require_approval_waits_and_can_be_approved() -> anyhow::Result<()> {
        let mut req = request("shop");
        req.spec.require_approval = true;
        let setup = setup_with(req).await?;
        let entry = setup
            .platform
            .deploy(setup.app.id, "alice", DeploymentTrigger::Manual)
            .await?;
        assert_eq!(entry.status, DeploymentStatus::PendingApproval);
        assert_eq!(
            entry.message.as_deref(),
            Some("Waiting for manual approval")
        );
        assert!(setup.runtime.names().is_empty());

        let approved = setup.platform.approve_deployment(entry.id, "bob").await?;
        assert_ne!(approved.status, DeploymentStatus::PendingApproval);
        let final_dep = finished(&setup, entry.id).await?;
        assert_eq!(final_dep.status, DeploymentStatus::Running);
        assert_eq!(setup.runtime.names(), [container_of(&setup, &final_dep)]);
        Ok(())
    }

    #[tokio::test]
    async fn a_pending_deployment_can_be_rejected() -> anyhow::Result<()> {
        let mut req = request("shop");
        req.spec.require_approval = true;
        let setup = setup_with(req).await?;
        let entry = setup
            .platform
            .deploy(setup.app.id, "alice", DeploymentTrigger::Manual)
            .await?;
        assert_eq!(entry.status, DeploymentStatus::PendingApproval);

        let rejected = setup.platform.reject_deployment(entry.id, "lead").await?;
        assert_eq!(rejected.status, DeploymentStatus::Cancelled);
        assert_eq!(rejected.message.as_deref(), Some("Rejected by lead"));
        assert!(setup.runtime.names().is_empty());

        let err = setup.platform.approve_deployment(entry.id, "bob").await;
        assert!(err.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn git_config_env_applied_during_deploy() -> anyhow::Result<()> {
        let setup = setup().await?;
        setup.fetcher.state().env_files.push((
            ".env".into(),
            "DB_HOST=git-host\nAPP_PORT=8080\nOVERRIDE_ME=git-val\n".into(),
        ));
        let mut app = setup.app.clone();
        app.spec.git_config = Some(r3v3rs3_api::platform::GitConfigFile {
            repository: Some("https://github.com/example/config.git".parse()?),
            branch: None,
            path: ".env".parse()?,
        });
        let req = AppRequest {
            name: app.name,
            target: app.target,
            spec: app.spec,
        };
        setup.platform.update_app(setup.app.id, req, 1).await?;
        let override_var = r3v3rs3_api::platform::EnvEntry {
            key: "OVERRIDE_ME".parse()?,
            value: Some("panel-val".into()),
            secret: false,
        };
        setup
            .platform
            .set_env(setup.app.id, vec![override_var])
            .await?;
        deploy(&setup).await?;
        let state = setup.runtime.state();
        let last_spec = state.specs.last().context("container spec")?;
        let get_env = |k: &str| {
            last_spec
                .env
                .iter()
                .find(|v| v.key.as_str() == k)
                .map(|v| v.value.as_str())
        };
        assert_eq!(get_env("DB_HOST"), Some("git-host"));
        assert_eq!(get_env("APP_PORT"), Some("8080"));
        assert_eq!(get_env("OVERRIDE_ME"), Some("panel-val"));
        Ok(())
    }

    async fn create_test_env_group(
        platform: &Platform,
        name: &str,
        shared: (&str, &str),
        chain: &str,
        now: u64,
    ) -> anyhow::Result<ShortId> {
        let group = platform
            .create_env_group(
                r3v3rs3_api::platform::EnvGroupSpec {
                    name: name.to_string(),
                    description: None,
                    variables: vec![
                        EnvEntry {
                            key: shared.0.parse()?,
                            value: Some(shared.1.to_string()),
                            secret: false,
                        },
                        EnvEntry {
                            key: "OVERRIDE_CHAIN".parse()?,
                            value: Some(chain.to_string()),
                            secret: false,
                        },
                    ],
                },
                now,
            )
            .await?;
        Ok(group.id)
    }

    #[tokio::test]
    async fn env_groups_applied_during_deploy_with_precedence() -> anyhow::Result<()> {
        let setup = setup().await?;
        let g1 = create_test_env_group(
            &setup.platform,
            "Group 1",
            ("SHARED_ONE", "val-1"),
            "group1-val",
            100,
        )
        .await?;
        let g2 = create_test_env_group(
            &setup.platform,
            "Group 2",
            ("SHARED_TWO", "val-2"),
            "group2-val",
            200,
        )
        .await?;

        let mut app = setup.app.clone();
        app.spec.env_groups = vec![g1, g2];
        let req = AppRequest {
            name: app.name,
            target: app.target,
            spec: app.spec,
        };
        setup.platform.update_app(setup.app.id, req, 300).await?;

        let override_var = EnvEntry {
            key: "OVERRIDE_CHAIN".parse()?,
            value: Some("app-val".into()),
            secret: false,
        };
        setup
            .platform
            .set_env(setup.app.id, vec![override_var])
            .await?;

        deploy(&setup).await?;
        let state = setup.runtime.state();
        let last_spec = state.specs.last().context("container spec")?;
        assert_eq!(env_value(last_spec, "SHARED_ONE"), Some("val-1"));
        assert_eq!(env_value(last_spec, "SHARED_TWO"), Some("val-2"));
        assert_eq!(env_value(last_spec, "OVERRIDE_CHAIN"), Some("app-val"));
        Ok(())
    }

    async fn setup_project_with_env(platform: &Platform) -> anyhow::Result<(ShortId, ShortId)> {
        let project = platform
            .create_project(
                r3v3rs3_api::platform::ProjectSpec {
                    name: "E-Commerce".into(),
                    description: None,
                    default_environments: vec!["production".into()],
                },
                100,
            )
            .await?;
        let env_id = project.environments[0].id;
        platform
            .set_environment_env(
                env_id,
                r3v3rs3_api::platform::EnvironmentEnvSpec {
                    variables: vec![
                        EnvEntry {
                            key: "ENV_SHARED".parse()?,
                            value: Some("env-prod-val".into()),
                            secret: false,
                        },
                        EnvEntry {
                            key: "OVERRIDE_VAR".parse()?,
                            value: Some("env-val".into()),
                            secret: false,
                        },
                    ],
                },
            )
            .await?;
        Ok((project.id, env_id))
    }

    #[tokio::test]
    async fn project_environment_env_applied_during_deploy_with_precedence() -> anyhow::Result<()> {
        let setup = setup().await?;
        let (project_id, env_id) = setup_project_with_env(&setup.platform).await?;

        let mut app = setup.app.clone();
        app.spec.project_id = Some(project_id);
        app.spec.environment_id = Some(env_id);
        let req = AppRequest {
            name: app.name,
            target: app.target,
            spec: app.spec,
        };
        setup.platform.update_app(setup.app.id, req, 200).await?;

        setup
            .platform
            .set_env(
                setup.app.id,
                vec![EnvEntry {
                    key: "OVERRIDE_VAR".parse()?,
                    value: Some("app-override-val".into()),
                    secret: false,
                }],
            )
            .await?;

        deploy(&setup).await?;
        let state = setup.runtime.state();
        let last_spec = state.specs.last().context("container spec")?;
        assert_eq!(env_value(last_spec, "ENV_SHARED"), Some("env-prod-val"));
        assert_eq!(
            env_value(last_spec, "OVERRIDE_VAR"),
            Some("app-override-val")
        );
        Ok(())
    }

    fn env_value<'a>(spec: &'a ContainerSpec, key: &str) -> Option<&'a str> {
        spec.env
            .iter()
            .find(|v| v.key.as_str() == key)
            .map(|v| v.value.as_str())
    }

    #[tokio::test]
    async fn pre_deploy_hook_success_runs_and_deploys() -> anyhow::Result<()> {
        let setup = setup().await?;
        let mut app = setup.app.clone();
        app.spec.lifecycle_hooks = Some(r3v3rs3_api::platform::DeploymentLifecycleHooks {
            pre_deploy: Some("echo migrate".into()),
            post_deploy: Some("echo warmup".into()),
        });
        let req = AppRequest {
            name: app.name,
            target: app.target,
            spec: app.spec,
        };
        setup.platform.update_app(setup.app.id, req, 200).await?;
        let dep = deploy(&setup).await?;
        assert_eq!(dep.status, DeploymentStatus::Running);
        Ok(())
    }

    #[tokio::test]
    async fn pre_deploy_hook_failure_aborts_deployment() -> anyhow::Result<()> {
        let setup = setup().await?;
        let first = deploy(&setup).await?;
        assert_eq!(first.status, DeploymentStatus::Running);

        setup.runtime.state().hook_fail = true;
        let mut app = setup.app.clone();
        app.spec.lifecycle_hooks = Some(r3v3rs3_api::platform::DeploymentLifecycleHooks {
            pre_deploy: Some("fail migrate".into()),
            post_deploy: None,
        });
        let req = AppRequest {
            name: app.name,
            target: app.target,
            spec: app.spec,
        };
        setup.platform.update_app(setup.app.id, req, 300).await?;
        let second = deploy(&setup).await?;
        assert_eq!(second.status, DeploymentStatus::Failed);
        assert!(
            second
                .message
                .unwrap_or_default()
                .contains("pre-deploy hook")
        );
        Ok(())
    }
}
