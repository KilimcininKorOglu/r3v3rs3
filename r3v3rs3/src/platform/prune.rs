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

pub(super) fn spawn_prune_scheduler(platform: &Arc<Platform>) {
    let platform = Arc::downgrade(platform);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let Some(platform) = platform.upgrade() else {
                return;
            };
            if let Err(err) = check_and_run_prune(&platform).await {
                error!(err = format!("{err:#}"), "automatic prune check failed");
            }
        }
    });
}

async fn check_and_run_prune(platform: &Platform) -> anyhow::Result<()> {
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
    let interval_secs = parse_interval_secs(&policy.interval);
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

fn parse_interval_secs(interval: &str) -> u64 {
    let s = interval.trim().to_ascii_lowercase();
    if s.ends_with('h') {
        s.trim_end_matches('h').parse::<u64>().unwrap_or(24) * 3600
    } else if s.ends_with('d') || s.ends_with("days") || s.ends_with("day") {
        let num = s
            .trim_end_matches("days")
            .trim_end_matches("day")
            .trim_end_matches('d')
            .parse::<u64>()
            .unwrap_or(1);
        num * 86400
    } else {
        s.parse::<u64>().unwrap_or(86400)
    }
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

    /// Saves the disk hygiene prune policy.
    pub async fn update_prune_policy(&self, policy: PrunePolicy) -> anyhow::Result<()> {
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
    fn parse_interval_understands_hours_and_days() {
        assert_eq!(parse_interval_secs("12h"), 12 * 3600);
        assert_eq!(parse_interval_secs("24h"), 24 * 3600);
        assert_eq!(parse_interval_secs("7days"), 7 * 86400);
        assert_eq!(parse_interval_secs("3d"), 3 * 86400);
        assert_eq!(parse_interval_secs("invalid"), 86400);
    }

    #[test]
    fn default_prune_policy_has_pressure_threshold() {
        let policy = PrunePolicy::default();
        assert_eq!(policy.disk_pressure_threshold, Some(85));
        assert!(policy.prune_images);
        assert!(policy.prune_build_cache);
        assert!(policy.prune_containers);
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
