//! Storage for node-level threshold alert rules.

use super::PlatformStore;
use r3v3rs3_api::platform::NodeAlertRules;

#[derive(sqlx::FromRow)]
struct NodeAlertRulesRow {
    enabled: bool,
    min_free_disk_percent: Option<i64>,
    bandwidth_spike_bytes_per_sec: Option<i64>,
}

impl From<NodeAlertRulesRow> for NodeAlertRules {
    fn from(row: NodeAlertRulesRow) -> Self {
        Self {
            enabled: row.enabled,
            min_free_disk_percent: row.min_free_disk_percent.map(|p| p as u8),
            bandwidth_spike_bytes_per_sec: row.bandwidth_spike_bytes_per_sec.map(|b| b as u64),
        }
    }
}

impl PlatformStore {
    pub async fn get_node_alert_rules(&self) -> anyhow::Result<NodeAlertRules> {
        let row = sqlx::query_as::<_, NodeAlertRulesRow>(
            "SELECT enabled, min_free_disk_percent, bandwidth_spike_bytes_per_sec FROM node_alert_rules WHERE id = 'default'",
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Into::into).unwrap_or_default())
    }

    pub async fn save_node_alert_rules(&self, rules: &NodeAlertRules) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO node_alert_rules (id, enabled, min_free_disk_percent, bandwidth_spike_bytes_per_sec)
             VALUES ('default', ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
             enabled = excluded.enabled,
             min_free_disk_percent = excluded.min_free_disk_percent,
             bandwidth_spike_bytes_per_sec = excluded.bandwidth_spike_bytes_per_sec",
        )
        .bind(rules.enabled)
        .bind(rules.min_free_disk_percent.map(|p| p as i64))
        .bind(rules.bandwidth_spike_bytes_per_sec.map(|b| b as i64))
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
