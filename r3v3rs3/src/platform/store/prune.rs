//! Storage for the platform disk hygiene prune policy.

use super::PlatformStore;
use r3v3rs3_api::platform::PrunePolicy;

#[derive(sqlx::FromRow)]
struct PrunePolicyRow {
    enabled: bool,
    interval: String,
    prune_images: bool,
    prune_volumes: bool,
    prune_build_cache: bool,
    prune_containers: bool,
    last_pruned_at: Option<i64>,
    last_reclaimed_bytes: Option<i64>,
}

impl From<PrunePolicyRow> for PrunePolicy {
    fn from(row: PrunePolicyRow) -> Self {
        Self {
            enabled: row.enabled,
            interval: row.interval,
            prune_images: row.prune_images,
            prune_volumes: row.prune_volumes,
            prune_build_cache: row.prune_build_cache,
            prune_containers: row.prune_containers,
            last_pruned_at: row.last_pruned_at.map(|t| t as u64),
            last_reclaimed_bytes: row.last_reclaimed_bytes.map(|b| b as u64),
        }
    }
}

impl PlatformStore {
    pub async fn get_prune_policy(&self) -> anyhow::Result<PrunePolicy> {
        let row = sqlx::query_as::<_, PrunePolicyRow>(
            "SELECT enabled, interval, prune_images, prune_volumes, prune_build_cache, prune_containers, last_pruned_at, last_reclaimed_bytes FROM prune_policy WHERE id = 'default'",
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into).unwrap_or_default())
    }

    pub async fn save_prune_policy(&self, policy: &PrunePolicy) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO prune_policy (id, enabled, interval, prune_images, prune_volumes, prune_build_cache, prune_containers, last_pruned_at, last_reclaimed_bytes)
             VALUES ('default', ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
             enabled = excluded.enabled,
             interval = excluded.interval,
             prune_images = excluded.prune_images,
             prune_volumes = excluded.prune_volumes,
             prune_build_cache = excluded.prune_build_cache,
             prune_containers = excluded.prune_containers,
             last_pruned_at = excluded.last_pruned_at,
             last_reclaimed_bytes = excluded.last_reclaimed_bytes",
        )
        .bind(policy.enabled)
        .bind(&policy.interval)
        .bind(policy.prune_images)
        .bind(policy.prune_volumes)
        .bind(policy.prune_build_cache)
        .bind(policy.prune_containers)
        .bind(policy.last_pruned_at.map(|t| t as i64))
        .bind(policy.last_reclaimed_bytes.map(|b| b as i64))
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
