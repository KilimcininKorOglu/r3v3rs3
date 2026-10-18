//! The background collector of the app metrics: every round it reads the container stats, the
//! health and the request counters of the local apps, records one sample per app and trims the
//! samples older than a day. An agent target carries no stats, so its apps get no samples.

use super::{Platform, proxy};
use r3v3rs3_api::id::ShortId;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// How often the collector takes a sample.
const COLLECT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a sample stays in the platform database.
const RETENTION: u64 = 24 * 60 * 60 * 1000;

/// The cumulative counters of the previous round, per app, for the deltas.
#[derive(Default)]
struct Previous {
    samples: Mutex<BTreeMap<ShortId, (u64, u64, u64)>>,
    alerts: Mutex<BTreeMap<(ShortId, &'static str), u64>>,
}

impl Previous {
    /// The deltas of the network and the request counters against the last round.
    fn deltas(&self, app: ShortId, rx: u64, tx: u64, requests: u64) -> (u64, u64, u64) {
        let mut samples = self.samples.lock().expect("the counter map is intact");
        let delta = |previous: Option<&u64>, value: u64| match previous {
            // A counter that went backwards means a restart: the whole value is new.
            Some(previous) if value >= *previous => value - previous,
            _ => value,
        };
        let previous = samples.insert(app, (rx, tx, requests));
        match previous {
            Some((previous_rx, previous_tx, previous_requests)) => (
                delta(Some(&previous_rx), rx),
                delta(Some(&previous_tx), tx),
                delta(Some(&previous_requests), requests),
            ),
            None => (0, 0, 0),
        }
    }

    /// Checks whether an alert for (app, metric) can be fired or is within cooldown (5 minutes).
    fn should_alert(&self, app: ShortId, metric: &'static str, now: u64) -> bool {
        const COOLDOWN_MS: u64 = 5 * 60 * 1000;
        let mut alerts = self.alerts.lock().expect("the alert map is intact");
        if let Some(&last) = alerts.get(&(app, metric))
            && now.saturating_sub(last) < COOLDOWN_MS
        {
            return false;
        }
        alerts.insert((app, metric), now);
        true
    }
}

/// Runs the collector until the platform shuts down.
pub(super) fn spawn_collector(platform: &Arc<Platform>) {
    let platform = Arc::downgrade(platform);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(COLLECT_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let previous = Previous::default();
        loop {
            interval.tick().await;
            let Some(platform) = platform.upgrade() else {
                return;
            };
            if let Err(err) = collect_round(&platform, &previous).await {
                tracing::error!(err = format!("{err:#}"), "the metrics collector failed");
            }
        }
    });
}

impl Platform {
    /// The recorded samples of an app, oldest first, over the last hours.
    pub async fn app_metrics(
        &self,
        id: ShortId,
        hours: u32,
    ) -> anyhow::Result<Vec<r3v3rs3_api::platform::AppMetricsPoint>> {
        self.app(id).await?;
        let since = crate::clock::unix_ms().saturating_sub(hours.min(24) as u64 * 3_600_000);
        Ok(self
            .store
            .app_metrics(id, since)
            .await?
            .into_iter()
            .map(|row| r3v3rs3_api::platform::AppMetricsPoint {
                ts: row.ts as i64,
                running: row.running,
                health: row.health,
                cpu: row.cpu,
                memory: row.memory,
                rx_bytes: row.rx_bytes,
                tx_bytes: row.tx_bytes,
                requests: row.requests,
            })
            .collect())
    }
}

/// Records one sample for every running local app, then trims the old samples.
async fn collect_round(platform: &Arc<Platform>, previous: &Previous) -> anyhow::Result<()> {
    let now = crate::clock::unix_ms();
    let requests = crate::proxy::stats::snapshot_resources();
    for app in platform.apps().await? {
        if let Err(err) = collect_app(platform, previous, &app, &requests).await {
            tracing::error!(
                err = format!("{err:#}"),
                app = %app.name,
                "failed to collect the metrics of the app"
            );
        }
    }
    platform
        .store
        .trim_app_metrics(now.saturating_sub(RETENTION))
        .await
}

/// Reads the stats and the health of one app and records its sample.
async fn collect_app(
    platform: &Arc<Platform>,
    previous: &Previous,
    app: &r3v3rs3_api::platform::AppEntry,
    requests: &[(String, u64)],
) -> anyhow::Result<()> {
    let Some(deployment) = platform.store.running_deployment(app.id).await? else {
        return Ok(());
    };
    let container = proxy::container_name(app.id, deployment)?;
    let runtime = platform.runtime(app.target);
    let info = runtime
        .inspect_container(&container)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the container {container} disappeared"))?;
    if !info.running {
        return Ok(());
    }
    let stats = runtime.stats(&container).await.unwrap_or_default();
    // The requests count under the resource name of the proxy, which a platform proxy
    // names after its app.
    let requests = requests
        .iter()
        .find(|(id, _)| *id == app.name.as_str())
        .map(|(_, count)| *count)
        .unwrap_or(0);
    let (rx, tx, served) =
        previous.deltas(app.id, stats.net_rx_bytes, stats.net_tx_bytes, requests);
    let health = match info.health {
        Some(r3v3rs3_api::container::ContainerHealth::Starting) => "starting",
        Some(r3v3rs3_api::container::ContainerHealth::Healthy) => "healthy",
        Some(r3v3rs3_api::container::ContainerHealth::Unhealthy) => "unhealthy",
        None => "",
    };
    platform
        .store
        .insert_app_metric(&super::store::AppMetricRow {
            app: app.id,
            ts: crate::clock::unix_ms(),
            running: true,
            health: health.to_string(),
            cpu: stats.cpu_percent,
            memory: stats.memory_bytes,
            rx_bytes: rx,
            tx_bytes: tx,
            requests: served,
        })
        .await?;
    if let Some(rules) = &app.spec.alert_rules {
        check_app_alerts(platform, previous, app, rules, &stats).await;
    }
    Ok(())
}

async fn check_app_alerts(
    platform: &Arc<Platform>,
    previous: &Previous,
    app: &r3v3rs3_api::platform::AppEntry,
    rules: &r3v3rs3_api::platform::AlertRules,
    stats: &r3v3rs3_api::container::ContainerStats,
) {
    let now = crate::clock::unix_ms();
    check_cpu_alert(
        platform,
        previous,
        app,
        rules.cpu_percent,
        stats.cpu_percent,
        now,
    )
    .await;
    check_mem_bytes_alert(
        platform,
        previous,
        app,
        rules.memory_bytes,
        stats.memory_bytes,
        now,
    )
    .await;
    check_mem_pct_alert(
        platform,
        previous,
        app,
        rules.memory_percent,
        stats.memory_bytes,
        stats.memory_limit,
        now,
    )
    .await;
}

async fn check_cpu_alert(
    platform: &Arc<Platform>,
    previous: &Previous,
    app: &r3v3rs3_api::platform::AppEntry,
    threshold: Option<u32>,
    cpu_percent: f64,
    now: u64,
) {
    let Some(limit) = threshold else { return };
    if cpu_percent >= limit as f64 && previous.should_alert(app.id, "cpu", now) {
        platform
            .notify_alert(
                app.id,
                app.name.to_string(),
                "cpu",
                format!("{:.1}%", cpu_percent),
                format!("{limit}%"),
            )
            .await;
    }
}

async fn check_mem_bytes_alert(
    platform: &Arc<Platform>,
    previous: &Previous,
    app: &r3v3rs3_api::platform::AppEntry,
    threshold: Option<u64>,
    memory_bytes: u64,
    now: u64,
) {
    let Some(limit) = threshold else { return };
    if memory_bytes >= limit && previous.should_alert(app.id, "memory_bytes", now) {
        let current_mb = memory_bytes / 1024 / 1024;
        let limit_mb = limit / 1024 / 1024;
        platform
            .notify_alert(
                app.id,
                app.name.to_string(),
                "memory_bytes",
                format!("{current_mb} MB"),
                format!("{limit_mb} MB"),
            )
            .await;
    }
}

async fn check_mem_pct_alert(
    platform: &Arc<Platform>,
    previous: &Previous,
    app: &r3v3rs3_api::platform::AppEntry,
    threshold: Option<u32>,
    memory_bytes: u64,
    memory_limit: u64,
    now: u64,
) {
    let Some(limit) = threshold else { return };
    if memory_limit == 0 {
        return;
    }
    let pct = ((memory_bytes as f64 / memory_limit as f64) * 100.0) as u32;
    if pct >= limit && previous.should_alert(app.id, "memory_percent", now) {
        platform
            .notify_alert(
                app.id,
                app.name.to_string(),
                "memory_percent",
                format!("{pct}%"),
                format!("{limit}%"),
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::super::store::AppMetricRow;
    use super::*;

    fn sample(app: &str, ts: u64) -> AppMetricRow {
        AppMetricRow {
            app: app.parse().unwrap(),
            ts,
            running: true,
            health: "healthy".to_string(),
            cpu: 12.5,
            memory: 1024,
            rx_bytes: 10,
            tx_bytes: 20,
            requests: 3,
        }
    }

    #[tokio::test]
    async fn samples_round_trip_and_trim() -> anyhow::Result<()> {
        let (command, _) = tokio::sync::mpsc::channel(1);
        let (platform, _, _dir) = super::super::tests::platform_with(
            &r3v3rs3_api::platform::PlatformConfig::default(),
            command,
        )
        .await?;
        let platform = Arc::new(platform);
        let app = platform
            .add_app(super::super::tests::request("m1"), 1)
            .await?;
        platform
            .store
            .insert_app_metric(&sample(&app.id.to_string(), 1_000))
            .await?;
        platform
            .store
            .insert_app_metric(&sample(&app.id.to_string(), 2_000))
            .await?;
        let id = app.id;
        let rows = platform.store.app_metrics(id, 1_000).await?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].ts, 1_000);
        assert_eq!(rows[1].health, "healthy");
        platform.store.trim_app_metrics(1_500).await?;
        assert_eq!(platform.store.app_metrics(id, 0).await?.len(), 1);
        Ok(())
    }

    #[test]
    fn deltas_come_from_the_previous_sample() {
        let previous = Previous::default();
        let app = "abc".parse().unwrap();
        // The first round has no previous sample, so the deltas start at zero.
        assert_eq!(previous.deltas(app, 100, 200, 10), (0, 0, 0));
        assert_eq!(previous.deltas(app, 150, 260, 13), (50, 60, 3));
        // A counter that went backwards means a restart: the whole value counts.
        assert_eq!(previous.deltas(app, 5, 0, 1), (5, 0, 1));
    }

    #[test]
    fn alert_cooldown_prevents_spam() {
        let previous = Previous::default();
        let app = "abc".parse().unwrap();
        let t0 = 1_000_000;
        // First alert succeeds
        assert!(previous.should_alert(app, "cpu", t0));
        // Alert within 5 minutes (300_000 ms) is suppressed
        assert!(!previous.should_alert(app, "cpu", t0 + 60_000));
        assert!(!previous.should_alert(app, "cpu", t0 + 299_999));
        // Different metric for same app succeeds immediately
        assert!(previous.should_alert(app, "memory_bytes", t0 + 60_000));
        // After 5 minutes, alert fires again
        assert!(previous.should_alert(app, "cpu", t0 + 300_001));
    }
}
