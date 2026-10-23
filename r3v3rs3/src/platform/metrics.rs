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
    node_alerts: Mutex<BTreeMap<&'static str, u64>>,
    host_net: Mutex<Option<(u64, u64, u64)>>,
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

    /// Checks whether an alert for a node metric can be fired or is within cooldown (15 minutes).
    fn should_alert_node(&self, metric: &'static str, now: u64) -> bool {
        const NODE_COOLDOWN_MS: u64 = 15 * 60 * 1000;
        let mut alerts = self
            .node_alerts
            .lock()
            .expect("the node alert map is intact");
        if let Some(&last) = alerts.get(metric)
            && now.saturating_sub(last) < NODE_COOLDOWN_MS
        {
            return false;
        }
        alerts.insert(metric, now);
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
    /// Reads the current node threshold alert rules.
    pub async fn node_alert_rules(&self) -> anyhow::Result<r3v3rs3_api::platform::NodeAlertRules> {
        self.store.get_node_alert_rules().await
    }

    /// Saves the node threshold alert rules.
    pub async fn update_node_alert_rules(
        &self,
        rules: r3v3rs3_api::platform::NodeAlertRules,
    ) -> anyhow::Result<()> {
        self.store.save_node_alert_rules(&rules).await
    }

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
    let mut total_rx: u64 = 0;
    let mut total_tx: u64 = 0;
    for app in platform.apps().await? {
        match collect_app(platform, previous, &app, &requests).await {
            Ok((rx, tx)) => {
                total_rx = total_rx.saturating_add(rx);
                total_tx = total_tx.saturating_add(tx);
            }
            Err(err) => {
                tracing::error!(
                    err = format!("{err:#}"),
                    app = %app.name,
                    "failed to collect the metrics of the app"
                );
            }
        }
    }
    check_node_alerts(platform, previous, now, total_rx, total_tx).await;
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
) -> anyhow::Result<(u64, u64)> {
    let Some(deployment) = platform.store.running_deployment(app.id).await? else {
        return Ok((0, 0));
    };
    let container = proxy::container_name(app.id, deployment)?;
    let runtime = platform.runtime(app.target);
    let info = runtime
        .inspect_container(&container)
        .await?
        .ok_or_else(|| anyhow::anyhow!("the container {container} disappeared"))?;
    let now = crate::clock::unix_ms();
    check_oom_alert(platform, previous, app, &info, now).await;
    if !info.running {
        return Ok((0, 0));
    }
    let stats = runtime.stats(&container).await.unwrap_or_default();
    let requests = requests
        .iter()
        .find(|(id, _)| *id == app.name.as_str())
        .map(|(_, count)| *count)
        .unwrap_or(0);
    let (rx, tx, served) =
        previous.deltas(app.id, stats.net_rx_bytes, stats.net_tx_bytes, requests);
    let health = health_status(info.health);
    platform
        .store
        .insert_app_metric(&super::store::AppMetricRow {
            app: app.id,
            ts: now,
            running: true,
            health: health.to_string(),
            cpu: stats.cpu_percent,
            memory: stats.memory_bytes,
            rx_bytes: rx,
            tx_bytes: tx,
            requests: served,
        })
        .await?;
    check_app_alerts(
        platform,
        previous,
        app,
        app.spec.alert_rules.as_ref(),
        &stats,
    )
    .await;
    Ok((rx, tx))
}

fn health_status(health: Option<r3v3rs3_api::container::ContainerHealth>) -> &'static str {
    match health {
        Some(r3v3rs3_api::container::ContainerHealth::Starting) => "starting",
        Some(r3v3rs3_api::container::ContainerHealth::Healthy) => "healthy",
        Some(r3v3rs3_api::container::ContainerHealth::Unhealthy) => "unhealthy",
        None => "",
    }
}

async fn check_oom_alert(
    platform: &Arc<Platform>,
    previous: &Previous,
    app: &r3v3rs3_api::platform::AppEntry,
    info: &r3v3rs3_api::container::ContainerInfo,
    now: u64,
) {
    if info.oom_killed && previous.should_alert(app.id, "oom_killed", now) {
        platform
            .notify_alert(
                app.id,
                app.name.to_string(),
                "oom_killed",
                "Container terminated by OOM killer".to_string(),
                "memory limit exceeded".to_string(),
            )
            .await;
    }
}

async fn check_app_alerts(
    platform: &Arc<Platform>,
    previous: &Previous,
    app: &r3v3rs3_api::platform::AppEntry,
    rules: Option<&r3v3rs3_api::platform::AlertRules>,
    stats: &r3v3rs3_api::container::ContainerStats,
) {
    let now = crate::clock::unix_ms();
    if let Some(rules) = rules {
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
    } else if app.spec.limits.memory_bytes.is_some() {
        check_mem_pct_alert(
            platform,
            previous,
            app,
            Some(90),
            stats.memory_bytes,
            stats.memory_limit,
            now,
        )
        .await;
    }
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

async fn check_node_alerts(
    platform: &Arc<Platform>,
    previous: &Previous,
    now: u64,
    container_rx_delta: u64,
    container_tx_delta: u64,
) {
    let rules = match platform.node_alert_rules().await {
        Ok(rules) => rules,
        Err(err) => {
            tracing::warn!(%err, "failed to read node alert rules");
            return;
        }
    };
    if !rules.enabled {
        return;
    }
    check_node_disk_alert(platform, previous, &rules, now).await;
    check_node_bandwidth_alert(
        platform,
        previous,
        &rules,
        now,
        container_rx_delta,
        container_tx_delta,
    )
    .await;
    check_node_fd_alert(platform, previous, &rules, now).await;
    check_node_socket_alert(platform, previous, &rules, now).await;
}

async fn check_node_disk_alert(
    platform: &Arc<Platform>,
    previous: &Previous,
    rules: &r3v3rs3_api::platform::NodeAlertRules,
    now: u64,
) {
    let Some(min_free_pct) = rules.min_free_disk_percent else {
        return;
    };
    let Some(space) = super::prune::system_disk_space() else {
        return;
    };
    let free_pct = ((space.available_bytes as f64 / space.total_bytes.max(1) as f64) * 100.0) as u8;
    if free_pct <= min_free_pct && previous.should_alert_node("disk_low", now) {
        let avail_mb = space.available_bytes / 1024 / 1024;
        let total_mb = space.total_bytes / 1024 / 1024;
        let summary = format!(
            "Node free disk space is low: {free_pct}% free ({avail_mb} MB available of {total_mb} MB, threshold: <={min_free_pct}%)"
        );
        let payload = serde_json::json!({
            "node": "local",
            "free_percent": free_pct,
            "available_bytes": space.available_bytes,
            "total_bytes": space.total_bytes,
            "used_percent": space.used_percent,
            "threshold_percent": min_free_pct
        });
        platform
            .dispatch_webhook_event(
                r3v3rs3_api::webhook::WebhookEvent::SystemDiskLow,
                summary,
                payload,
            )
            .await;
    }
}

fn host_network_bytes() -> Option<(u64, u64)> {
    let content = std::fs::read_to_string("/proc/net/dev").ok()?;
    let mut total_rx = 0u64;
    let mut total_tx = 0u64;
    for line in content.lines().skip(2) {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        if iface.trim() == "lo" {
            continue;
        }
        let fields: Vec<&str> = rest.split_whitespace().collect();
        if fields.len() >= 9
            && let (Ok(rx), Ok(tx)) = (fields[0].parse::<u64>(), fields[8].parse::<u64>())
        {
            total_rx = total_rx.saturating_add(rx);
            total_tx = total_tx.saturating_add(tx);
        }
    }
    Some((total_rx, total_tx))
}

fn calculate_bandwidth_rate(
    previous: &Previous,
    now: u64,
    container_rx_delta: u64,
    container_tx_delta: u64,
) -> u64 {
    let (current_rx, current_tx) = host_network_bytes().unwrap_or((0, 0));
    let mut host_net = previous.host_net.lock().expect("host net lock");
    match *host_net {
        Some((prev_rx, prev_tx, prev_ts)) if current_rx > 0 && current_tx > 0 => {
            *host_net = Some((current_rx, current_tx, now));
            let delta_rx = current_rx.saturating_sub(prev_rx);
            let delta_tx = current_tx.saturating_sub(prev_tx);
            let elapsed_secs = (now.saturating_sub(prev_ts) / 1000).max(1);
            (delta_rx + delta_tx) / elapsed_secs
        }
        _ => {
            if current_rx > 0 && current_tx > 0 {
                *host_net = Some((current_rx, current_tx, now));
            }
            (container_rx_delta + container_tx_delta) / COLLECT_INTERVAL.as_secs().max(1)
        }
    }
}

async fn check_node_bandwidth_alert(
    platform: &Arc<Platform>,
    previous: &Previous,
    rules: &r3v3rs3_api::platform::NodeAlertRules,
    now: u64,
    container_rx_delta: u64,
    container_tx_delta: u64,
) {
    let Some(threshold_bps) = rules.bandwidth_spike_bytes_per_sec else {
        return;
    };
    let rate_bps = calculate_bandwidth_rate(previous, now, container_rx_delta, container_tx_delta);

    if rate_bps >= threshold_bps && previous.should_alert_node("bandwidth_spike", now) {
        let rate_mb_s = rate_bps as f64 / 1_000_000.0;
        let threshold_mb_s = threshold_bps as f64 / 1_000_000.0;
        let summary = format!(
            "Node network bandwidth spike detected: {rate_mb_s:.2} MB/s (threshold: {threshold_mb_s:.2} MB/s)"
        );
        let payload = serde_json::json!({
            "node": "local",
            "bytes_per_sec": rate_bps,
            "threshold_bytes_per_sec": threshold_bps,
            "mb_per_sec": rate_mb_s
        });
        platform
            .dispatch_webhook_event(
                r3v3rs3_api::webhook::WebhookEvent::SystemBandwidthSpike,
                summary,
                payload,
            )
            .await;
    }
}

async fn check_node_fd_alert(
    platform: &Arc<Platform>,
    previous: &Previous,
    rules: &r3v3rs3_api::platform::NodeAlertRules,
    now: u64,
) {
    let Some(max_fd_pct) = rules.max_fd_ratio_percent else {
        return;
    };
    let (Some(open), Some(max)) = (
        crate::admin::system_metrics::open_fds(),
        crate::admin::system_metrics::max_fds(),
    ) else {
        return;
    };
    if max == 0 {
        return;
    }
    let usage_pct = ((open as f64 / max as f64) * 100.0) as u8;
    if usage_pct >= max_fd_pct && previous.should_alert_node("fd_exhaustion", now) {
        let summary = format!(
            "Node open file descriptor exhaustion warning: {usage_pct}% used ({open} open of {max} max limit, threshold: >={max_fd_pct}%)"
        );
        let payload = serde_json::json!({
            "node": "local",
            "open_fds": open,
            "max_fds": max,
            "usage_percent": usage_pct,
            "threshold_percent": max_fd_pct,
        });
        platform
            .dispatch_webhook_event(
                r3v3rs3_api::webhook::WebhookEvent::SystemFdExhaustion,
                summary,
                payload,
            )
            .await;
    }
}

async fn check_node_socket_alert(
    platform: &Arc<Platform>,
    previous: &Previous,
    rules: &r3v3rs3_api::platform::NodeAlertRules,
    now: u64,
) {
    let Some(max_tw) = rules.max_time_wait_sockets else {
        return;
    };
    let sock = crate::admin::system_metrics::socket_stats();
    let Some(tw) = sock.tcp_tw else {
        return;
    };
    if tw >= max_tw as u64 && previous.should_alert_node("socket_exhaustion", now) {
        let inuse = sock.tcp_inuse.unwrap_or(0);
        let total = sock.total_used.unwrap_or(0);
        let summary = format!(
            "Node TCP socket / TIME_WAIT exhaustion warning: {tw} TIME_WAIT sockets (threshold: >={max_tw}, active inuse: {inuse}, total sockets: {total})"
        );
        let payload = serde_json::json!({
            "node": "local",
            "tcp_tw": tw,
            "tcp_inuse": inuse,
            "total_sockets": total,
            "threshold_tw": max_tw,
        });
        platform
            .dispatch_webhook_event(
                r3v3rs3_api::webhook::WebhookEvent::SystemSocketExhaustion,
                summary,
                payload,
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

    #[test]
    fn node_alert_cooldown_prevents_spam() {
        let previous = Previous::default();
        let t0 = 1_000_000;
        assert!(previous.should_alert_node("disk_low", t0));
        // Alert within 15 minutes (900_000 ms) is suppressed
        assert!(!previous.should_alert_node("disk_low", t0 + 60_000));
        assert!(!previous.should_alert_node("disk_low", t0 + 899_999));
        // Different node metric succeeds immediately
        assert!(previous.should_alert_node("bandwidth_spike", t0 + 60_000));
        // After 15 minutes, fires again
        assert!(previous.should_alert_node("disk_low", t0 + 900_001));
    }

    #[test]
    fn bandwidth_rate_fallback() {
        let previous = Previous::default();
        // 30MB total delta over 30s interval = 1MB/s = 1_000_000 bytes/sec
        let rate = calculate_bandwidth_rate(&previous, 100_000, 15_000_000, 15_000_000);
        assert_eq!(rate, 1_000_000);
    }
}
