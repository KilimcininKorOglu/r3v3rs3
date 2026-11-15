//! Storage for the platform disk hygiene prune policy.

use super::PlatformStore;
use r3v3rs3_api::platform::PrunePolicy;

#[derive(sqlx::FromRow)]
struct PrunePolicyRow {
    enabled: bool,
    interval: String,
    disk_pressure_threshold: Option<i64>,
    prune_images: bool,
    prune_volumes: bool,
    prune_build_cache: bool,
    prune_containers: bool,
    prune_logs: bool,
    log_vacuum_threshold_mb: Option<i64>,
    container_log_max_size: Option<String>,
    container_log_max_file: Option<String>,
    build_cache_keep_storage_mb: Option<i64>,
    kept_builds_count: Option<i64>,
    dangling_layers_only: Option<bool>,
    last_pruned_at: Option<i64>,
    last_pressure_pruned_at: Option<i64>,
    last_reclaimed_bytes: Option<i64>,
}

impl From<PrunePolicyRow> for PrunePolicy {
    fn from(row: PrunePolicyRow) -> Self {
        Self {
            enabled: row.enabled,
            interval: row.interval,
            disk_pressure_threshold: row.disk_pressure_threshold.map(|t| t as u8),
            prune_images: row.prune_images,
            prune_volumes: row.prune_volumes,
            prune_build_cache: row.prune_build_cache,
            prune_containers: row.prune_containers,
            prune_logs: row.prune_logs,
            log_vacuum_threshold_mb: row.log_vacuum_threshold_mb.map(|t| t as u64),
            container_log_max_size: row.container_log_max_size,
            container_log_max_file: row.container_log_max_file,
            build_cache_keep_storage_mb: row.build_cache_keep_storage_mb.map(|m| m as u64),
            kept_builds_count: row.kept_builds_count.map(|k| k as u32),
            dangling_layers_only: row.dangling_layers_only.unwrap_or(true),
            last_pruned_at: row.last_pruned_at.map(|t| t as u64),
            last_pressure_pruned_at: row.last_pressure_pruned_at.map(|t| t as u64),
            last_reclaimed_bytes: row.last_reclaimed_bytes.map(|b| b as u64),
        }
    }
}

impl PlatformStore {
    pub async fn get_prune_policy(&self) -> anyhow::Result<PrunePolicy> {
        let row = sqlx::query_as::<_, PrunePolicyRow>(
            "SELECT enabled, interval, disk_pressure_threshold, prune_images, prune_volumes, prune_build_cache, prune_containers, prune_logs, log_vacuum_threshold_mb, container_log_max_size, container_log_max_file, build_cache_keep_storage_mb, kept_builds_count, dangling_layers_only, last_pruned_at, last_pressure_pruned_at, last_reclaimed_bytes FROM prune_policy WHERE id = 'default'",
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into).unwrap_or_default())
    }

    pub async fn save_prune_policy(&self, policy: &PrunePolicy) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO prune_policy (id, enabled, interval, disk_pressure_threshold, prune_images, prune_volumes, prune_build_cache, prune_containers, prune_logs, log_vacuum_threshold_mb, container_log_max_size, container_log_max_file, build_cache_keep_storage_mb, kept_builds_count, dangling_layers_only, last_pruned_at, last_pressure_pruned_at, last_reclaimed_bytes)
             VALUES ('default', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
             enabled = excluded.enabled,
             interval = excluded.interval,
             disk_pressure_threshold = excluded.disk_pressure_threshold,
             prune_images = excluded.prune_images,
             prune_volumes = excluded.prune_volumes,
             prune_build_cache = excluded.prune_build_cache,
             prune_containers = excluded.prune_containers,
             prune_logs = excluded.prune_logs,
             log_vacuum_threshold_mb = excluded.log_vacuum_threshold_mb,
             container_log_max_size = excluded.container_log_max_size,
             container_log_max_file = excluded.container_log_max_file,
             build_cache_keep_storage_mb = excluded.build_cache_keep_storage_mb,
             kept_builds_count = excluded.kept_builds_count,
             dangling_layers_only = excluded.dangling_layers_only,
             last_pruned_at = excluded.last_pruned_at,
             last_pressure_pruned_at = excluded.last_pressure_pruned_at,
             last_reclaimed_bytes = excluded.last_reclaimed_bytes",
        )
        .bind(policy.enabled)
        .bind(&policy.interval)
        .bind(policy.disk_pressure_threshold.map(|t| t as i64))
        .bind(policy.prune_images)
        .bind(policy.prune_volumes)
        .bind(policy.prune_build_cache)
        .bind(policy.prune_containers)
        .bind(policy.prune_logs)
        .bind(policy.log_vacuum_threshold_mb.map(|t| t as i64))
        .bind(&policy.container_log_max_size)
        .bind(&policy.container_log_max_file)
        .bind(policy.build_cache_keep_storage_mb.map(|m| m as i64))
        .bind(policy.kept_builds_count.map(|k| k as i64))
        .bind(policy.dangling_layers_only)
        .bind(policy.last_pruned_at.map(|t| t as i64))
        .bind(policy.last_pressure_pruned_at.map(|t| t as i64))
        .bind(policy.last_reclaimed_bytes.map(|b| b as i64))
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
