//! The background scheduler and execution for the platform disk hygiene prune policy.

use super::Platform;
use crate::audit::AuditRecord;
use crate::clock::unix_secs;
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::dashboard::DiskSpace;
use r3v3rs3_api::platform::{PrunePolicy, PruneResult};
use r3v3rs3_api::webhook::WebhookEvent;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info, warn};

/// The time after a disk-pressure prune of a host before the next one of the same host.
const PRESSURE_COOLDOWN_SECS: u64 = 900;

fn add_prune(total: &mut PruneResult, part: &PruneResult) {
    total.containers_deleted += part.containers_deleted;
    total.images_deleted += part.images_deleted;
    total.volumes_deleted += part.volumes_deleted;
    total.build_cache_deleted += part.build_cache_deleted;
    total.logs_truncated += part.logs_truncated;
    total.space_reclaimed_bytes += part.space_reclaimed_bytes;
}

/// Deletes the rotated log files of the named containers whose logs reach the threshold, the
/// same rule as the vacuum of the local Docker host.
async fn vacuum_over(
    runtime: &dyn crate::runtime::ContainerRuntime,
    names: &[r3v3rs3_api::container::ContainerName],
    threshold_bytes: Option<u64>,
) -> anyhow::Result<r3v3rs3_api::container::LogVacuumResult> {
    let mut total = r3v3rs3_api::container::LogVacuumResult::default();
    for name in names {
        let Some(info) = runtime.container_log_info(name).await? else {
            continue;
        };
        if info.size_bytes < threshold_bytes.unwrap_or(0) {
            continue;
        }
        let freed = runtime.vacuum_container_logs(name).await?;
        total.truncated_files += freed.truncated_files;
        total.freed_bytes += freed.freed_bytes;
    }
    Ok(total)
}

pub(super) fn spawn_prune_scheduler(platform: &Arc<Platform>) {
    let weak = Arc::downgrade(platform);
    let cancel = platform.shutdown_token().clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = interval.tick() => {}
            }
            let Some(platform) = weak.upgrade() else {
                return;
            };
            if let Err(err) = check_and_run_prune(&platform).await {
                error!(err = format!("{err:#}"), "automatic prune check failed");
            }
        }
    });
}

async fn check_and_run_prune(platform: &Platform) -> anyhow::Result<()> {
    let _guard = platform.track_job();
    let policy = platform.prune_policy().await?;
    if !policy.enabled {
        return Ok(());
    }
    let now = unix_secs();

    if let Some(threshold) = policy.disk_pressure_threshold {
        platform
            .prune_agents_under_pressure(&policy, threshold, now)
            .await?;
        if check_disk_pressure(platform, &policy, threshold, now).await? {
            return Ok(());
        }
    }

    check_periodic_prune(platform, &policy, now).await
}

async fn check_disk_pressure(
    platform: &Platform,
    policy: &PrunePolicy,
    threshold: u8,
    now: u64,
) -> anyhow::Result<bool> {
    let Some(disk) = system_disk_space() else {
        return Ok(false);
    };
    if disk.used_percent < threshold {
        return Ok(false);
    }
    let in_cooldown = match policy.last_pressure_pruned_at {
        Some(last) => now < last.saturating_add(PRESSURE_COOLDOWN_SECS),
        None => false,
    };
    if in_cooldown {
        return Ok(false);
    }
    warn!(
        used_percent = disk.used_percent,
        threshold, "disk pressure threshold exceeded, triggering emergency reactive prune"
    );
    let result = platform
        .run_pressure_prune(disk.used_percent, threshold)
        .await?;
    info!(
        reclaimed_bytes = result.space_reclaimed_bytes,
        used_percent = disk.used_percent,
        "emergency reactive disk prune completed"
    );
    Ok(true)
}

async fn check_periodic_prune(
    platform: &Platform,
    policy: &PrunePolicy,
    now: u64,
) -> anyhow::Result<()> {
    let interval_secs = policy.interval_secs()?;
    let should_run = match policy.last_pruned_at {
        Some(last) => now >= last.saturating_add(interval_secs),
        None => true,
    };
    if should_run {
        info!("running automatic disk hygiene prune");
        let result = platform.run_prune().await?;
        info!(
            reclaimed_bytes = result.space_reclaimed_bytes,
            images = result.images_deleted,
            containers = result.containers_deleted,
            volumes = result.volumes_deleted,
            build_cache = result.build_cache_deleted,
            "automatic prune completed"
        );
    }
    Ok(())
}

/// Reads the disk space metrics for the given filesystem path.
pub fn get_disk_space(path: &Path) -> Option<DiskSpace> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }

    let frsize = stat.f_frsize as u64;
    let total_bytes = stat.f_blocks as u64 * frsize;
    let available_bytes = stat.f_bavail as u64 * frsize;
    let used_bytes = total_bytes.saturating_sub(available_bytes);
    let used_percent = if total_bytes > 0 {
        ((used_bytes as f64 / total_bytes as f64) * 100.0).round() as u8
    } else {
        0
    };

    Some(DiskSpace {
        total_bytes,
        available_bytes,
        used_bytes,
        used_percent,
    })
}

/// Checks the Docker data directory or root filesystem for available disk metrics.
pub fn system_disk_space() -> Option<DiskSpace> {
    let docker_dir = Path::new("/var/lib/docker");
    if docker_dir.exists() {
        get_disk_space(docker_dir)
    } else {
        get_disk_space(Path::new("/"))
    }
}

impl Platform {
    /// Reads the current disk hygiene prune policy.
    pub async fn prune_policy(&self) -> anyhow::Result<PrunePolicy> {
        self.store.get_prune_policy().await
    }

    /// Saves the disk hygiene prune policy after it checks the interval and the threshold.
    pub async fn update_prune_policy(&self, policy: PrunePolicy) -> anyhow::Result<()> {
        policy.validate()?;
        self.store.save_prune_policy(&policy).await
    }

    /// Executes a disk hygiene prune immediately on the server host and on every agent target,
    /// and updates the policy run stats. The time of the run is saved before the agents run, so
    /// an offline agent does not make the scheduler repeat the prune of the server host.
    pub async fn run_prune(&self) -> anyhow::Result<PruneResult> {
        let mut policy = self.prune_policy().await?;
        let keep = self
            .prune_keep(r3v3rs3_api::platform::LOCAL_TARGET.parse()?)
            .await?;
        let mut result = self.local.run_prune(&policy, &keep).await?;
        policy.last_pruned_at = Some(unix_secs());
        policy.last_reclaimed_bytes = Some(result.space_reclaimed_bytes);
        self.store.save_prune_policy(&policy).await?;
        let failures = self.prune_agents(&policy, &mut result).await?;
        if !failures.is_empty() {
            anyhow::bail!("the prune failed on {}", failures.join("; "));
        }
        policy.last_reclaimed_bytes = Some(result.space_reclaimed_bytes);
        self.store.save_prune_policy(&policy).await?;
        Ok(result)
    }

    /// Prunes each agent host whose disk usage reaches the threshold, once in the cooldown of
    /// that host, and names the target in the `system.disk_pressure` webhook. Returns the names
    /// of the pruned targets. An agent that cannot answer is logged and skipped, so one offline
    /// agent does not stop the checks of the others.
    pub(super) async fn prune_agents_under_pressure(
        &self,
        policy: &PrunePolicy,
        threshold: u8,
        now: u64,
    ) -> anyhow::Result<Vec<String>> {
        let mut pruned = Vec::new();
        for target in self.store.targets().await? {
            if super::is_local(target.id) || self.in_pressure_cooldown(target.id, now) {
                continue;
            }
            match self
                .prune_agent_if_full(policy, &target, threshold, now)
                .await
            {
                Ok(true) => pruned.push(target.name),
                Ok(false) => {}
                Err(err) => {
                    warn!(target = %target.name, err = format!("{err:#}"), "disk pressure check of an agent failed")
                }
            }
        }
        Ok(pruned)
    }

    fn in_pressure_cooldown(&self, target: r3v3rs3_api::id::ShortId, now: u64) -> bool {
        let prunes = self
            .agent_pressure_prunes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        prunes
            .get(&target)
            .is_some_and(|last| now < last.saturating_add(PRESSURE_COOLDOWN_SECS))
    }

    async fn prune_agent_if_full(
        &self,
        policy: &PrunePolicy,
        target: &r3v3rs3_api::platform::TargetEntry,
        threshold: u8,
        now: u64,
    ) -> anyhow::Result<bool> {
        let runtime = self.runtime(target.id);
        let Some(disk) = runtime.disk_space().await? else {
            return Ok(false);
        };
        if disk.used_percent < threshold {
            return Ok(false);
        }
        self.agent_pressure_prunes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(target.id, now);
        let keep = self.prune_keep(target.id).await?;
        let result = runtime.run_prune(policy, &keep).await?;
        let payload = serde_json::json!({
            "target": target.name,
            "used_percent": disk.used_percent,
            "threshold": threshold,
            "space_reclaimed_bytes": result.space_reclaimed_bytes,
        });
        let summary = format!(
            "Disk pressure on {} at {}% exceeded {threshold}% threshold, emergency prune reclaimed {} bytes",
            target.name, disk.used_percent, result.space_reclaimed_bytes
        );
        self.dispatch_webhook_event(WebhookEvent::SystemDiskPressure, summary, payload)
            .await;
        Ok(true)
    }

    /// Prunes the host of every agent target and adds its result. Returns the error of each
    /// target that failed; the other targets still run.
    async fn prune_agents(
        &self,
        policy: &PrunePolicy,
        result: &mut PruneResult,
    ) -> anyhow::Result<Vec<String>> {
        let mut failures = Vec::new();
        for target in self.store.targets().await? {
            if super::is_local(target.id) {
                continue;
            }
            let keep = self.prune_keep(target.id).await?;
            match self.runtime(target.id).run_prune(policy, &keep).await {
                Ok(part) => add_prune(result, &part),
                Err(err) => failures.push(format!("{}: {err:#}", target.name)),
            }
        }
        if policy.prune_logs {
            let threshold = policy
                .log_vacuum_threshold_mb
                .map(|mb| mb.saturating_mul(1024 * 1024));
            let vacuum = self.vacuum_agent_logs(threshold).await?;
            result.logs_truncated += vacuum.truncated_files as u64;
            result.space_reclaimed_bytes += vacuum.freed_bytes;
        }
        Ok(failures)
    }

    /// Vacuums container logs across the platform exceeding the optional MB threshold.
    pub async fn vacuum_all_logs(
        &self,
        threshold_mb: Option<u64>,
    ) -> anyhow::Result<r3v3rs3_api::container::LogVacuumResult> {
        let threshold_bytes = threshold_mb.map(|mb| mb.saturating_mul(1024 * 1024));
        let mut total = self.local.vacuum_all_logs(threshold_bytes).await?;
        let agents = self.vacuum_agent_logs(threshold_bytes).await?;
        total.truncated_files += agents.truncated_files;
        total.freed_bytes += agents.freed_bytes;
        Ok(total)
    }

    /// Vacuums the logs of the running apps on their agent targets, on the agent hosts.
    async fn vacuum_agent_logs(
        &self,
        threshold_bytes: Option<u64>,
    ) -> anyhow::Result<r3v3rs3_api::container::LogVacuumResult> {
        let mut total = r3v3rs3_api::container::LogVacuumResult::default();
        for app in self.store.apps().await? {
            let Some(deployment) = self.store.running_deployment(app.id).await? else {
                continue;
            };
            let remote = app.spec.targets_or_default(app.target);
            for target in remote
                .into_iter()
                .filter(|target| !super::is_local(*target))
            {
                let names = self.container_names_on(&app, deployment, target).await?;
                let runtime = self.runtime(target);
                let freed = vacuum_over(runtime.as_ref(), &names, threshold_bytes).await?;
                total.truncated_files += freed.truncated_files;
                total.freed_bytes += freed.freed_bytes;
            }
        }
        Ok(total)
    }

    /// The containers of the running deployment of an app on one of its targets.
    async fn container_names_on(
        &self,
        app: &r3v3rs3_api::platform::AppEntry,
        deployment: r3v3rs3_api::id::ShortId,
        target: r3v3rs3_api::id::ShortId,
    ) -> anyhow::Result<Vec<r3v3rs3_api::container::ContainerName>> {
        if !app.spec.source.is_compose() {
            return Ok(vec![super::proxy::container_name(app.id, deployment)?]);
        }
        let project = super::compose::project_name(app.id)?;
        let list = self
            .runtime(target)
            .list_project_containers(&project)
            .await?;
        Ok(list
            .into_iter()
            .filter_map(|item| item.name.parse().ok())
            .collect())
    }

    /// Executes an emergency disk pressure prune, dispatches a system.disk_pressure webhook,
    /// and records the event in audit logs.
    pub async fn run_pressure_prune(
        &self,
        used_percent: u8,
        threshold: u8,
    ) -> anyhow::Result<PruneResult> {
        let mut policy = self.prune_policy().await?;
        let keep = self
            .prune_keep(r3v3rs3_api::platform::LOCAL_TARGET.parse()?)
            .await?;
        let result = self.local.run_prune(&policy, &keep).await?;
        let now = unix_secs();
        policy.last_pruned_at = Some(now);
        policy.last_pressure_pruned_at = Some(now);
        policy.last_reclaimed_bytes = Some(result.space_reclaimed_bytes);
        self.store.save_prune_policy(&policy).await?;

        let payload = serde_json::json!({
            "used_percent": used_percent,
            "threshold": threshold,
            "space_reclaimed_bytes": result.space_reclaimed_bytes,
            "images_deleted": result.images_deleted,
            "containers_deleted": result.containers_deleted,
            "volumes_deleted": result.volumes_deleted,
            "build_cache_deleted": result.build_cache_deleted,
        });
        let summary = format!(
            "Disk pressure at {used_percent}% exceeded {threshold}% threshold, emergency prune reclaimed {} bytes",
            result.space_reclaimed_bytes
        );
        self.dispatch_webhook_event(WebhookEvent::SystemDiskPressure, summary, payload)
            .await;

        if let Some(audit) = &self.audit {
            let record = AuditRecord::new(AuditAction::RunPrune).summary(format!(
                "Emergency disk pressure prune at {used_percent}% (threshold {threshold}%): reclaimed {} bytes",
                result.space_reclaimed_bytes
            ));
            audit.record("system", None, record);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_prune_policy_has_pressure_threshold() {
        let policy = PrunePolicy::default();
        assert_eq!(policy.disk_pressure_threshold, Some(85));
        assert!(policy.prune_images);
        assert!(policy.prune_build_cache);
        assert!(policy.prune_containers);
        assert!(policy.prune_logs);
        assert_eq!(policy.log_vacuum_threshold_mb, Some(50));
        assert!(!policy.prune_volumes);
        assert_eq!(policy.last_pressure_pruned_at, None);
    }

    #[test]
    fn disk_space_calculation() {
        let temp = std::env::temp_dir();
        let space = get_disk_space(&temp);
        assert!(space.is_some());
        let space = space.unwrap();
        assert!(space.total_bytes > 0);
        assert!(space.used_percent <= 100);
    }
}
