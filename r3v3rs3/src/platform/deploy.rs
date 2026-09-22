//! The deploy pipeline of an image app on the local target. A deployment is blue-green: the old
//! container serves until the new container passes its health check, then the proxy switches to
//! the new container and the old container stops after a drain period.

use super::store::{NewDeployment, StoredEnv};
use super::{Platform, new_id, not_found, proxy};
use crate::clock::unix_ms;
use anyhow::{Context as _, anyhow, bail};
use r3v3rs3_api::container::{
    APP_LABEL, AppName, ContainerName, ContainerSpec, EnvVar, ImageRef, NetworkName,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{
    AppEntry, AppSource, AppSpec, DeploymentEntry, DeploymentStatus, DeploymentTrigger,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{error, info};

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

/// What one deployment runs. The pipeline reads only this snapshot, so an edit of the app during
/// the deployment does not change it.
struct Job {
    app: ShortId,
    deployment: ShortId,
    spec: AppSpec,
    env: Vec<StoredEnv>,
    /// The image to pull, or the digest of the deployment that a rollback repeats.
    image: ImageRef,
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

    /// Starts a deployment of the current spec and environment of an app, and returns it while
    /// it is queued.
    pub async fn deploy(
        self: &Arc<Self>,
        id: ShortId,
        username: &str,
        trigger: DeploymentTrigger,
    ) -> anyhow::Result<DeploymentEntry> {
        let app = self.app(id).await?;
        let AppSource::Image { image } = app.spec.source.clone();
        let env = self.store.env(id).await?;
        let lock = self.lock_app(&app)?;
        let new = new_deployment(&app, trigger, username, &app.spec, &env, None)?;
        self.start_job(lock, new, image).await
    }

    /// Starts a deployment that repeats the image digest, the spec and the environment of an
    /// earlier deployment.
    pub async fn rollback(
        self: &Arc<Self>,
        id: ShortId,
        username: &str,
    ) -> anyhow::Result<DeploymentEntry> {
        let (app, digest) = self.rollback_source(id).await?;
        let image = digest
            .parse::<ImageRef>()
            .map_err(|_| Error::RollbackUnavailable { id })?;
        let (spec, env) = self
            .store
            .deployment_snapshot(id)
            .await?
            .ok_or_else(|| not_found(id))?;
        let app = self.app(app).await?;
        let lock = self.lock_app(&app)?;
        let trigger = DeploymentTrigger::Rollback;
        let new = new_deployment(&app, trigger, username, &spec, &env, Some(&digest))?;
        self.start_job(lock, new, image).await
    }

    /// The app and the image digest of the deployment that a rollback repeats.
    async fn rollback_source(&self, id: ShortId) -> anyhow::Result<(ShortId, String)> {
        let source = self.deployment(id).await?;
        let digest = source
            .image_digest
            .ok_or(Error::RollbackUnavailable { id })?;
        Ok((source.app, digest))
    }

    async fn start_job(
        self: &Arc<Self>,
        lock: AppLock,
        new: NewDeployment<'_>,
        image: ImageRef,
    ) -> anyhow::Result<DeploymentEntry> {
        self.store.insert_deployment(&new).await?;
        let entry = self
            .store
            .deployment(new.id)
            .await?
            .context("the new deployment was not stored")?;
        let job = Job {
            app: new.app,
            deployment: new.id,
            spec: new.spec.clone(),
            env: new.env.to_vec(),
            image,
        };
        let platform = self.clone();
        tokio::spawn(async move {
            platform.run_job(&job).await;
            drop(lock);
        });
        Ok(entry)
    }

    async fn run_job(&self, job: &Job) {
        info!(app = %job.app, deployment = %job.deployment, "deployment started");
        let result = match self.start_container(job).await {
            Ok(name) => self.switch(job, &name).await,
            Err(err) => Err(err),
        };
        let Err(err) = result else {
            info!(app = %job.app, deployment = %job.deployment, "deployment finished");
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
    }

    /// Starts the new container and waits for its health check. A failure removes the new
    /// container, and the old container keeps serving.
    async fn start_container(&self, job: &Job) -> anyhow::Result<ContainerName> {
        self.store
            .set_status(job.deployment, DeploymentStatus::Deploying)
            .await?;
        let digest = self.resolve_image(&job.image).await?;
        self.store.set_image_digest(job.deployment, &digest).await?;
        let name = proxy::container_name(job.app, job.deployment)?;
        let result = self.run_container(job, &name, &digest).await;
        if result.is_err()
            && let Err(err) = self.local.remove_container(&name).await
        {
            error!(container = %name, "failed to remove a failed container: {err:#}");
        }
        result.map(|()| name)
    }

    /// Pulls the image unless it is present, and returns the reference that pins it: a
    /// repository digest, or the image id of an image that has none.
    async fn resolve_image(&self, image: &ImageRef) -> anyhow::Result<String> {
        let present = match self.local.inspect_image(image).await? {
            Some(info) if pinned(image) => Some(info),
            _ => None,
        };
        let info = match present {
            Some(info) => info,
            None => {
                self.local.pull_image(image).await?;
                self.local
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
        job: &Job,
        name: &ContainerName,
        digest: &str,
    ) -> anyhow::Result<()> {
        let app_label = app_label(job.app)?;
        let network = app_network(job.app)?;
        self.local.ensure_network(&network, &app_label).await?;
        let spec = ContainerSpec {
            name: name.clone(),
            image: digest.parse()?,
            env: self.open_env(job.app, &job.env)?,
            labels: BTreeMap::from([
                (APP_LABEL.to_string(), app_label.to_string()),
                (DEPLOYMENT_LABEL.to_string(), job.deployment.to_string()),
            ]),
            network: Some(network),
            volumes: job.spec.volumes.clone(),
            restart: job.spec.restart,
            limits: job.spec.limits,
            publish: Some(job.spec.port),
        };
        self.local.create_container(&spec).await?;
        self.local.start_container(name).await?;
        self.wait_healthy(name, &job.spec).await
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

    /// Waits until the container passes its health check. A stopped container fails at once.
    async fn wait_healthy(&self, name: &ContainerName, spec: &AppSpec) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + self.timing.health_timeout;
        loop {
            let info = self
                .local
                .inspect_container(name)
                .await?
                .ok_or_else(|| anyhow!("the container {name} disappeared"))?;
            if !info.running {
                let log = self.log_tail(name).await;
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
                let log = self.log_tail(name).await;
                bail!(
                    "the app did not pass its health check in {} seconds{log}",
                    self.timing.health_timeout.as_secs()
                );
            }
            tokio::time::sleep(self.timing.health_interval).await;
        }
    }

    /// The last lines of the container log, for a failure message.
    async fn log_tail(&self, name: &ContainerName) -> String {
        match self.local.logs(name, LOG_TAIL).await {
            Ok(log) if !log.trim().is_empty() => format!("\n{}", log.trim_end()),
            Ok(_) => String::new(),
            Err(err) => format!("\nthe log is not readable: {err:#}"),
        }
    }

    /// Routes the app to the new container, marks the deployment running, and removes the old
    /// containers after the drain period.
    async fn switch(&self, job: &Job, name: &ContainerName) -> anyhow::Result<()> {
        self.store
            .finish_deployment(job.app, job.deployment, unix_ms())
            .await?;
        self.publish().await;
        tokio::time::sleep(self.timing.drain).await;
        if let Err(err) = self.remove_containers(job.app, Some(name)).await {
            // The deployment serves already, so only the log shows the failed cleanup.
            error!(app = %job.app, "failed to remove the old containers: {err:#}");
        }
        Ok(())
    }

    /// Stops and removes every container of an app except `keep`.
    pub(super) async fn remove_containers(
        &self,
        app: ShortId,
        keep: Option<&ContainerName>,
    ) -> anyhow::Result<()> {
        let containers = self.local.list_containers(&app_label(app)?).await?;
        for container in containers {
            let name = container
                .name
                .trim_start_matches('/')
                .parse::<ContainerName>()?;
            if Some(&name) == keep {
                continue;
            }
            self.local
                .stop_container(&name, self.timing.stop_timeout)
                .await?;
            self.local.remove_container(&name).await?;
        }
        Ok(())
    }

    /// Removes the containers and the network of an app.
    pub(super) async fn remove_app_resources(&self, app: ShortId) -> anyhow::Result<()> {
        self.remove_containers(app, None).await?;
        self.local.remove_network(&app_network(app)?).await
    }
}

fn new_deployment<'a>(
    app: &AppEntry,
    trigger: DeploymentTrigger,
    username: &'a str,
    spec: &'a AppSpec,
    env: &'a [StoredEnv],
    image_digest: Option<&'a str>,
) -> anyhow::Result<NewDeployment<'a>> {
    Ok(NewDeployment {
        id: new_id()?,
        app: app.id,
        trigger,
        username,
        spec,
        env,
        image_digest,
        started_at: unix_ms(),
    })
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
    Ok(format!("r3v3rs3-{app}").parse()?)
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

#[cfg(test)]
mod tests {
    use super::super::fake::{DIGEST, FakeRuntime};
    use super::super::tests::{TempDir, platform_with, request};
    use super::*;
    use r3v3rs3_api::platform::{EnvEntry, PlatformConfig};
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
        app: AppEntry,
        _dir: TempDir,
    }

    /// A platform with the app `shop`, whose container answers its health check.
    async fn setup() -> anyhow::Result<Setup> {
        // The tests read no proxy snapshots.
        let (command, _) = mpsc::channel(1);
        let (platform, runtime, dir) = platform_with(&PlatformConfig::default(), command).await?;
        let platform = Arc::new(platform);
        runtime.state().host_port = app_server(200).await?;
        let mut shop = request("shop");
        shop.spec.health_check_path = Some("/healthz".into());
        let app = platform.add_app(shop, 1).await?;
        Ok(Setup {
            platform,
            runtime,
            app,
            _dir: dir,
        })
    }

    /// Waits until the pipeline of the app has ended, and returns the deployment.
    async fn finished(setup: &Setup, id: ShortId) -> anyhow::Result<DeploymentEntry> {
        for _ in 0..500 {
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
}
