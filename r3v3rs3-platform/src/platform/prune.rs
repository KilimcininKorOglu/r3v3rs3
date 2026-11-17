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

    if let Some(threshold) = policy.disk_pressure_threshold
        && check_disk_pressure(platform, &policy, threshold, now).await?
    {
        return Ok(());
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
    let cooldown_secs = 900;
    let in_cooldown = match policy.last_pressure_pruned_at {
        Some(last) => now < last.saturating_add(cooldown_secs),
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

    /// Executes a disk hygiene prune immediately and updates the policy run stats.
    pub async fn run_prune(&self) -> anyhow::Result<PruneResult> {
        let mut policy = self.prune_policy().await?;
        let result = self.local.run_prune(&policy).await?;
        policy.last_pruned_at = Some(unix_secs());
        policy.last_reclaimed_bytes = Some(result.space_reclaimed_bytes);
        self.store.save_prune_policy(&policy).await?;
        Ok(result)
    }

    /// Vacuums container logs across the platform exceeding the optional MB threshold.
    pub async fn vacuum_all_logs(
        &self,
        threshold_mb: Option<u64>,
    ) -> anyhow::Result<r3v3rs3_api::container::LogVacuumResult> {
        let threshold_bytes = threshold_mb.map(|mb| mb.saturating_mul(1024 * 1024));
        let mut total = self.local.vacuum_all_logs(threshold_bytes).await?;
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
        let result = self.local.run_prune(&policy).await?;
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
            let record = AuditRecord::new(AuditAction::UpdatePlatformSettings).summary(format!(
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
