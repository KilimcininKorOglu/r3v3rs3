//! Stops, starts and restarts the containers of the running deployment of an app, without a new
//! deployment: no build, no pull and no new container.
//!
//! A stopped app keeps its proxy, which answers with the 503 page of the error page theme. The
//! mark is set before the containers stop and cleared after they start, so a partial failure
//! leaves a stopped app, and the refresh loop stops the containers that still run.

use super::compose::project_name;
use super::deploy::AppLock;
use super::store::RunningDeployment;
use super::{Platform, not_found, proxy};
use crate::audit::AuditRecord;
use crate::runtime::PruneKeep;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::platform::{AppEntry, AppSource};
use std::sync::Arc;
use std::time::Duration;

/// The time the processes of a container get to exit before Docker kills them.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

impl Platform {
    /// Stops the containers of an app and answers its domains with 503.
    pub async fn stop_app(self: &Arc<Self>, id: ShortId) -> anyhow::Result<AppEntry> {
        let app = self.app(id).await?;
        let lock = self.lock_app(&app)?;
        let result = self.stop_locked(&app).await;
        self.release(lock, id).await;
        result?;
        self.app(id).await
    }

    /// Starts the stopped containers of an app again and routes its domains to them.
    pub async fn start_app(self: &Arc<Self>, id: ShortId) -> anyhow::Result<AppEntry> {
        let app = self.app(id).await?;
        let lock = self.lock_app(&app)?;
        let result = self.start_locked(&app).await;
        self.release(lock, id).await;
        result?;
        self.app(id).await
    }

    /// Stops and starts the containers of an app; a stopped app only starts.
    pub async fn restart_app(self: &Arc<Self>, id: ShortId) -> anyhow::Result<AppEntry> {
        let app = self.app(id).await?;
        let lock = self.lock_app(&app)?;
        let result = self.restart_locked(&app).await;
        self.release(lock, id).await;
        result?;
        self.app(id).await
    }

    async fn restart_locked(&self, app: &AppEntry) -> anyhow::Result<()> {
        if !app.stopped {
            self.stop_locked(app).await?;
        }
        self.start_locked(app).await
    }

    /// Whether an automatic job of `app` must not run because the app is stopped. The first
    /// skip of each `job` during a stop goes to the audit log; a job that its schedule keeps
    /// due is not audited again in every round.
    pub(super) fn skips_stopped(&self, app: &AppEntry, job: &str) -> bool {
        if !app.stopped {
            return false;
        }
        let first = self
            .stopped_skips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((app.id, job.to_string()));
        if first && let Some(audit) = &self.audit {
            let record = AuditRecord::new(AuditAction::SkipStoppedApp)
                .id(app.id)
                .summary(format!("{}: skipped {job}: app stopped", app.name));
            audit.record("system", None, record);
        }
        tracing::info!(app = %app.name, job, "skipped: app stopped");
        true
    }

    async fn stop_locked(&self, app: &AppEntry) -> anyhow::Result<()> {
        let deployment = self.serving(app).await?;
        self.stopped_skips
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(id, _)| *id != app.id);
        self.store.set_app_stopped(app.id, true).await?;
        self.set_running(&deployment, false).await
    }

    async fn start_locked(&self, app: &AppEntry) -> anyhow::Result<()> {
        let deployment = self.serving(app).await?;
        self.set_running(&deployment, true).await?;
        self.store.set_app_stopped(app.id, false).await?;
        Ok(())
    }

    /// Sends the new proxies and runs a webhook that arrived while the app was locked.
    async fn release(self: &Arc<Self>, lock: AppLock, id: ShortId) {
        drop(lock);
        self.publish().await;
        self.clone().run_queued_hook(id).await;
    }

    /// The running deployment of an app, with the spec that started its containers.
    async fn serving(&self, app: &AppEntry) -> anyhow::Result<RunningDeployment> {
        let deployment = self
            .store
            .running_deployments()
            .await?
            .into_iter()
            .find(|deployment| deployment.app == app.id);
        deployment.ok_or_else(|| {
            Error::AppNotDeployed {
                name: app.name.to_string(),
            }
            .into()
        })
    }

    /// Stops or starts the containers of a deployment on every target that runs them.
    async fn set_running(&self, deployment: &RunningDeployment, run: bool) -> anyhow::Result<()> {
        for target in deployment.spec.targets_or_default(deployment.target) {
            self.set_running_on(deployment, target, run).await?;
        }
        Ok(())
    }

    async fn set_running_on(
        &self,
        deployment: &RunningDeployment,
        target: ShortId,
        run: bool,
    ) -> anyhow::Result<()> {
        let backend = self.backend(target);
        if matches!(deployment.spec.source, AppSource::Compose { .. }) {
            let project = project_name(deployment.app)?;
            return match run {
                true => backend.compose.start(&project).await,
                false => backend.compose.stop(&project).await,
            };
        }
        let name = proxy::container_name(deployment.app, deployment.id)?;
        match run {
            true => backend.runtime.start_container(&name).await,
            false => backend.runtime.stop_container(&name, STOP_TIMEOUT).await,
        }
    }

    /// Stops the containers of stopped apps that run again: a container with the restart
    /// policy `always` starts with the Docker Engine. An app that another job holds is left to
    /// the next round, and the failure of one app, such as an offline agent, does not hold up
    /// the others.
    pub(super) async fn keep_stopped_apps_down(&self) -> anyhow::Result<()> {
        for deployment in self.store.running_deployments().await? {
            if !deployment.stopped {
                continue;
            }
            if let Err(err) = self.keep_down(&deployment).await {
                tracing::warn!(app = %deployment.app_name, error = %format!("{err:#}"), "cannot keep a stopped app down");
            }
        }
        Ok(())
    }

    async fn keep_down(&self, deployment: &RunningDeployment) -> anyhow::Result<()> {
        let app = self.store.app(deployment.app).await?;
        let app = app.ok_or_else(|| not_found(deployment.app))?;
        let Ok(_lock) = self.lock_app(&app) else {
            return Ok(());
        };
        self.stop_running_containers(deployment).await
    }

    /// The containers on `target` that a prune keeps: those of the stopped apps, so that a
    /// start finds them again.
    pub(super) async fn prune_keep(&self, target: ShortId) -> anyhow::Result<PruneKeep> {
        let mut keep = PruneKeep::default();
        for deployment in self.store.running_deployments().await? {
            let targets = deployment.spec.targets_or_default(deployment.target);
            if !deployment.stopped || !targets.contains(&target) {
                continue;
            }
            if matches!(deployment.spec.source, AppSource::Compose { .. }) {
                keep.projects.push(project_name(deployment.app)?);
            } else {
                let name = proxy::container_name(deployment.app, deployment.id)?;
                keep.containers.push(name);
            }
        }
        Ok(keep)
    }

    async fn stop_running_containers(&self, deployment: &RunningDeployment) -> anyhow::Result<()> {
        let name = proxy::container_name(deployment.app, deployment.id)?;
        for target in deployment.spec.targets_or_default(deployment.target) {
            let info = self.runtime(target).inspect_container(&name).await?;
            if info.is_some_and(|info| info.running) {
                tracing::warn!(app = %deployment.app_name, %target, "a stopped app runs again; stopping it");
                self.set_running_on(deployment, target, false).await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::fake::{FakeCompose, FakeRuntime};
    use super::super::store::NewDeployment;
    use super::super::tests::{TempDir, platform_with, request};
    use super::*;
    use crate::discovery::DiscoveredProxy;
    use anyhow::Context as _;
    use r3v3rs3_api::container::ContainerName;
    use r3v3rs3_api::platform::{DeploymentTrigger, PlatformConfig};
    use r3v3rs3_api::proxy::ProxyKind;
    use tokio::sync::mpsc;

    struct Setup {
        platform: Arc<Platform>,
        runtime: Arc<FakeRuntime>,
        compose: Arc<FakeCompose>,
        audit: Arc<crate::audit::MemoryAuditStore>,
        _dir: TempDir,
    }

    async fn setup() -> anyhow::Result<Setup> {
        let (command, mut commands) = mpsc::channel(1);
        // The proxies go to a server that reads every command.
        tokio::spawn(async move { while commands.recv().await.is_some() {} });
        let (mut platform, runtime, dir) =
            platform_with(&PlatformConfig::default(), command).await?;
        let compose = Arc::new(FakeCompose::new(runtime.clone()));
        platform.compose = compose.clone();
        let audit = Arc::new(crate::audit::MemoryAuditStore::new());
        platform.audit = Some(Arc::new(crate::audit::AuditLog::new(
            audit.clone(),
            String::new(),
        )));
        Ok(Setup {
            platform: Arc::new(platform),
            runtime,
            compose,
            audit,
            _dir: dir,
        })
    }

    /// The summaries of the skip records, once the background writes landed.
    async fn skip_records(
        audit: &crate::audit::MemoryAuditStore,
        count: usize,
    ) -> anyhow::Result<Vec<String>> {
        use crate::audit::{AuditFilter, AuditStore};
        let filter = AuditFilter {
            since: 0,
            until: u64::MAX,
            username: None,
            resource_id: None,
            action: Some(AuditAction::SkipStoppedApp),
            search: None,
            limit: 100,
        };
        for _ in 0..200 {
            let entries = audit.query(&filter).await?;
            if entries.len() >= count {
                return Ok(entries.into_iter().map(|e| e.summary).collect());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        anyhow::bail!("fewer than {count} skip records")
    }

    /// An app with a domain whose deployment runs in the container that this returns.
    async fn deployed(
        platform: &Platform,
        source: Option<AppSource>,
    ) -> anyhow::Result<(ShortId, ContainerName)> {
        let mut request = request("shop");
        request.spec.domains = vec!["shop.example.com".parse()?];
        let mut app = platform.add_app(request, 1).await?;
        if let Some(source) = source {
            app.spec.source = source;
            platform.store.update_app(&app).await?;
        }
        let deployment =
            NewDeployment::new(&app, DeploymentTrigger::Manual, "alice", &app.spec, &[])?;
        platform.store.insert_deployment(&deployment).await?;
        platform
            .store
            .finish_deployment(app.id, deployment.id, 3)
            .await?;
        Ok((app.id, proxy::container_name(app.id, deployment.id)?))
    }

    fn running(runtime: &FakeRuntime, name: &ContainerName) -> bool {
        runtime.state().containers[name.as_str()].running
    }

    async fn shop_proxy(platform: &Platform) -> anyhow::Result<DiscoveredProxy> {
        let (proxies, _) = platform.read_proxies().await?;
        proxies.into_iter().next().context("the app has no proxy")
    }

    fn http(proxy: &DiscoveredProxy) -> &r3v3rs3_api::proxy::HttpProxy {
        match &proxy.definition.kind {
            ProxyKind::Http(http) => http,
            _ => panic!("not an HTTP proxy"),
        }
    }

    /// Stop keeps the container and the proxy: the domain answers the 503 page of the theme
    /// instead of a connection error. Start runs the same container again, with no new
    /// container, build or pull, and the proxy routes to it as before.
    #[tokio::test]
    async fn a_stopped_app_keeps_its_container_and_answers_503() -> anyhow::Result<()> {
        let setup = setup().await?;
        let (id, name) = deployed(&setup.platform, None).await?;
        setup.runtime.set(&name, true, &[(80, 40001)]);
        let before = shop_proxy(&setup.platform).await?;

        assert!(setup.platform.stop_app(id).await?.stopped);
        assert!(!running(&setup.runtime, &name));
        check_stopped_proxy(&before, &shop_proxy(&setup.platform).await?);

        assert!(!setup.platform.start_app(id).await?.stopped);
        assert!(running(&setup.runtime, &name));
        check_nothing_created(&setup.runtime, &name);
        assert_eq!(shop_proxy(&setup.platform).await?, before);
        Ok(())
    }

    fn check_stopped_proxy(before: &DiscoveredProxy, stopped: &DiscoveredProxy) {
        assert_eq!(stopped.key, before.key, "the proxy id changed");
        assert_eq!(stopped.definition.acme, before.definition.acme);
        let maintenance = &http(stopped).maintenance;
        assert!(maintenance.enabled && maintenance.error_page && maintenance.status == 503);
    }

    /// The same container runs, and nothing was built, pulled or created.
    fn check_nothing_created(runtime: &FakeRuntime, name: &ContainerName) {
        assert_eq!(runtime.names(), [name.to_string()]);
        let state = runtime.state();
        assert!(state.builds.is_empty() && state.specs.is_empty() && state.images.is_empty());
    }

    /// Restart of a running app stops and starts the same container; of a stopped app it
    /// only starts it.
    #[tokio::test]
    async fn a_restart_runs_the_same_container_again() -> anyhow::Result<()> {
        let setup = setup().await?;
        let (id, name) = deployed(&setup.platform, None).await?;
        setup.runtime.set(&name, true, &[(80, 40001)]);
        assert!(!setup.platform.restart_app(id).await?.stopped);
        assert!(running(&setup.runtime, &name));
        setup.platform.stop_app(id).await?;
        assert!(!setup.platform.restart_app(id).await?.stopped);
        assert!(running(&setup.runtime, &name));
        assert_eq!(setup.runtime.names(), [name.to_string()]);
        Ok(())
    }

    /// A Compose app stops and starts its whole project, never `up` or `down`.
    #[tokio::test]
    async fn a_compose_app_stops_and_starts_its_project() -> anyhow::Result<()> {
        let setup = setup().await?;
        let source = AppSource::Compose {
            repository: None,
            branch: "main".parse()?,
            file: None,
            service: "web".parse()?,
            connection: None,
            compose_yaml: None,
        };
        let (id, _) = deployed(&setup.platform, Some(source)).await?;
        setup.platform.stop_app(id).await?;
        setup.platform.start_app(id).await?;
        let project = project_name(id)?;
        assert_eq!(
            setup.compose.state().calls,
            [format!("stop {project}"), format!("start {project}")]
        );
        Ok(())
    }

    /// Without a running deployment there is nothing to stop or start: a clear 400 error.
    #[tokio::test]
    async fn an_app_without_a_deployment_cannot_stop_or_start() -> anyhow::Result<()> {
        let setup = setup().await?;
        let app = setup.platform.add_app(request("shop"), 1).await?;
        for result in [
            setup.platform.stop_app(app.id).await,
            setup.platform.start_app(app.id).await,
        ] {
            let err = result
                .err()
                .context("a stop or start without a deployment worked")?;
            assert!(
                matches!(err.downcast_ref(), Some(Error::AppNotDeployed { .. })),
                "{err:#}"
            );
        }
        assert!(!setup.platform.app(app.id).await?.stopped);
        Ok(())
    }

    /// A container with the restart policy `always` comes back when the Docker Engine starts;
    /// the refresh stops it again, and a deployment that serves clears the mark.
    #[tokio::test]
    async fn a_stopped_app_stays_down_until_a_deployment_serves() -> anyhow::Result<()> {
        let setup = setup().await?;
        let (id, name) = deployed(&setup.platform, None).await?;
        setup.runtime.set(&name, true, &[(80, 40001)]);
        setup.platform.stop_app(id).await?;
        setup.runtime.set(&name, true, &[(80, 40001)]);
        setup.platform.keep_stopped_apps_down().await?;
        assert!(!running(&setup.runtime, &name));

        let app = setup.platform.app(id).await?;
        let next = NewDeployment::new(&app, DeploymentTrigger::Manual, "alice", &app.spec, &[])?;
        setup.platform.store.insert_deployment(&next).await?;
        setup
            .platform
            .store
            .finish_deployment(id, next.id, 4)
            .await?;
        assert!(!setup.platform.app(id).await?.stopped);
        Ok(())
    }

    /// A prune keeps the stopped container of a stopped app, so a start finds it; once the app
    /// runs, nothing is kept.
    #[tokio::test]
    async fn a_prune_keeps_the_container_of_a_stopped_app() -> anyhow::Result<()> {
        let setup = setup().await?;
        let (id, name) = deployed(&setup.platform, None).await?;
        setup.runtime.set(&name, true, &[(80, 40001)]);
        setup.platform.stop_app(id).await?;
        setup.platform.run_prune().await?;
        setup.platform.start_app(id).await?;
        setup.platform.run_prune().await?;
        let keeps = setup.runtime.state().prune_keeps.clone();
        let expected = PruneKeep {
            containers: vec![name],
            projects: Vec::new(),
        };
        assert_eq!(keeps, [expected, PruneKeep::default()]);
        Ok(())
    }

    /// A new image of a stopped app with auto-deploy deploys nothing; the skip is audited once
    /// while the schedule stays due, and once more after the next stop.
    #[tokio::test]
    async fn a_stopped_app_skips_an_image_auto_update() -> anyhow::Result<()> {
        let setup = setup().await?;
        let id = stopped_auto_deploy_app(&setup).await?;
        auto_update(&setup.platform, id).await?;
        auto_update(&setup.platform, id).await?;
        assert_eq!(setup.platform.store.deployments(id, 10).await?.len(), 1);

        start_and_stop_again(&setup.platform, id).await?;
        auto_update(&setup.platform, id).await?;
        skip_records(&setup.audit, 2).await?;
        // The writes run in the background; a repeated record would land in this time.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let records = skip_records(&setup.audit, 2).await?;
        let expected = "shop: skipped image auto-update: app stopped";
        assert_eq!(records, [expected, expected]);
        Ok(())
    }

    /// A running app with auto-deploy, stopped.
    async fn stopped_auto_deploy_app(setup: &Setup) -> anyhow::Result<ShortId> {
        let (id, name) = deployed(&setup.platform, None).await?;
        setup.runtime.set(&name, true, &[(80, 40001)]);
        let mut app = setup.platform.app(id).await?;
        app.spec.auto_deploy = true;
        setup.platform.store.update_app(&app).await?;
        setup.platform.stop_app(id).await?;
        Ok(id)
    }

    /// Starts a stopped app and stops it again: a new stop.
    async fn start_and_stop_again(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<()> {
        platform.start_app(id).await?;
        platform.stop_app(id).await?;
        Ok(())
    }

    /// The image check found a newer image of the app.
    async fn auto_update(platform: &Arc<Platform>, id: ShortId) -> anyhow::Result<()> {
        let status: r3v3rs3_api::platform::ImageUpdateStatus = serde_json::from_value(
            serde_json::json!({ "update_available": true, "checked_at": 1 }),
        )?;
        let app = platform.app(id).await?;
        super::super::image_check::auto_deploy_if_needed(platform, &app, &status).await
    }

    /// A due scheduled command of a stopped app waits: it is audited as skipped and does not
    /// run, so its last run stays empty.
    #[tokio::test]
    async fn a_stopped_app_skips_its_scheduled_commands() -> anyhow::Result<()> {
        let setup = setup().await?;
        let (id, name) = deployed(&setup.platform, None).await?;
        setup.runtime.set(&name, true, &[(80, 40001)]);
        let request = r3v3rs3_api::platform::ScheduledCommandRequest {
            name: "cleanup".into(),
            schedule: "@every 5m".into(),
            command: "true".into(),
            enabled: true,
        };
        let cmd = setup.platform.add_scheduled_command(id, request).await?;
        setup.platform.stop_app(id).await?;
        setup.platform.run_due_commands().await;
        let records = skip_records(&setup.audit, 1).await?;
        assert_eq!(
            records,
            [format!(
                "shop: skipped scheduled command {}: app stopped",
                cmd.id
            )]
        );
        let stored = setup.platform.store.scheduled_command(cmd.id).await?;
        assert!(stored.context("the command is gone")?.last_run_at.is_none());
        assert!(setup.runtime.state().execs.is_empty());
        Ok(())
    }

    /// A stopped app whose deploy window is open, with a pending deployment; returns its id.
    async fn stopped_app_with_pending_deployment(setup: &Setup) -> anyhow::Result<ShortId> {
        let (id, _) = deployed(&setup.platform, None).await?;
        let mut app = setup.platform.app(id).await?;
        // A window without a range is always open.
        app.spec.deploy_window = Some("always".into());
        setup.platform.store.update_app(&app).await?;
        setup.platform.store.set_app_stopped(id, true).await?;
        let pending = NewDeployment::new(&app, DeploymentTrigger::Manual, "alice", &app.spec, &[])?
            .pending("window");
        setup.platform.store.insert_deployment(&pending).await?;
        Ok(pending.id)
    }

    /// A pending deployment of a stopped app is not approved when its window opens.
    #[tokio::test]
    async fn a_stopped_app_keeps_its_window_deployment_pending() -> anyhow::Result<()> {
        let setup = setup().await?;
        let pending = stopped_app_with_pending_deployment(&setup).await?;
        setup.platform.check_window_deployments().await;
        let records = skip_records(&setup.audit, 1).await?;
        let expected = format!("shop: skipped window deployment {pending}: app stopped");
        assert_eq!(records, [expected]);
        let entry = setup.platform.store.deployment(pending).await?;
        let status = entry.context("the deployment is gone")?.status;
        assert_eq!(
            status,
            r3v3rs3_api::platform::DeploymentStatus::PendingApproval
        );
        Ok(())
    }
}
