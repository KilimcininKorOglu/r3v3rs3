//! The recorded metrics of the apps: one row per app and collection moment, kept for a day.

use super::PlatformStore;
use r3v3rs3_api::id::ShortId;

/// One recorded sample of one app.
#[derive(Debug, Clone, PartialEq)]
pub struct AppMetricRow {
    pub app: ShortId,
    pub ts: u64,
    pub running: bool,
    /// The container health, empty when the image declares no health check.
    pub health: String,
    pub cpu: f64,
    pub memory: u64,
    /// The bytes of the network interfaces since the previous sample.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// The served requests since the previous sample.
    pub requests: u64,
}

/// The columns of one app_metrics row, as SQLite holds them.
#[derive(sqlx::FromRow)]
struct MetricRow {
    app_id: String,
    ts: i64,
    running: bool,
    health: String,
    cpu: f64,
    memory: i64,
    rx_bytes: i64,
    tx_bytes: i64,
    requests: i64,
}

impl PlatformStore {
    /// Records one sample of one app.
    pub async fn insert_app_metric(&self, row: &AppMetricRow) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO app_metrics (app_id, ts, running, health, cpu, memory, rx_bytes,
                tx_bytes, requests) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.app.to_string())
        .bind(sql_int(row.ts))
        .bind(row.running)
        .bind(&row.health)
        .bind(row.cpu)
        .bind(sql_int(row.memory))
        .bind(sql_int(row.rx_bytes))
        .bind(sql_int(row.tx_bytes))
        .bind(sql_int(row.requests))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The samples of one app from `since`, oldest first.
    pub async fn app_metrics(&self, app: ShortId, since: u64) -> anyhow::Result<Vec<AppMetricRow>> {
        let rows = sqlx::query_as::<_, MetricRow>(
            "SELECT app_id, ts, running, health, cpu, memory, rx_bytes, tx_bytes, requests
            FROM app_metrics WHERE app_id = ? AND ts >= ? ORDER BY ts",
        )
        .bind(app.to_string())
        .bind(sql_int(since))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(metric_of).collect()
    }

    /// Deletes the samples older than `before`.
    pub async fn trim_app_metrics(&self, before: u64) -> anyhow::Result<()> {
        sqlx::query("DELETE FROM app_metrics WHERE ts < ?")
            .bind(sql_int(before))
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

/// The recorded sample of one row.
fn metric_of(row: MetricRow) -> anyhow::Result<AppMetricRow> {
    Ok(AppMetricRow {
        app: row.app_id.parse()?,
        ts: row.ts as u64,
        running: row.running,
        health: row.health,
        cpu: row.cpu,
        memory: row.memory as u64,
        rx_bytes: row.rx_bytes as u64,
        tx_bytes: row.tx_bytes as u64,
        requests: row.requests as u64,
    })
}

/// The i64 that SQLite stores for a millisecond time or a byte count.
fn sql_int(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}
