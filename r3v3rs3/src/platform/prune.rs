//! The background scheduler and execution for the platform disk hygiene prune policy.

use super::Platform;
use crate::clock::unix_secs;
use r3v3rs3_api::platform::{PrunePolicy, PruneResult};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

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
    let interval_secs = parse_interval_secs(&policy.interval);
    let now = unix_secs();
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
}
