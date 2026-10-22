//! Prometheus and runtime anomaly detection engine and admin API handlers.

use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::proxy::stats;
use axum::{Extension, extract::State, response::Json};
use r3v3rs3_api::anomaly::{
    AnomalyAlert, AnomalyAlertRules, AnomalyKind, AnomalySeverity, SystemAnomaliesSnapshot,
};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::webhook::WebhookEvent;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tracing::info;

const COOLDOWN_DURATION: Duration = Duration::from_secs(300);
const RECENT_MAX_CAPACITY: usize = 50;

static ANOMALY_STATE: OnceLock<AnomalyMonitorState> = OnceLock::new();

pub struct AnomalyMonitorState {
    config_dir: PathBuf,
    rules: Mutex<AnomalyAlertRules>,
    active_anomalies: Mutex<Vec<AnomalyAlert>>,
    recent_anomalies: Mutex<VecDeque<AnomalyAlert>>,
    cooldowns: Mutex<HashMap<AnomalyKind, Instant>>,
    prev_stats: Mutex<Option<(u64, u64, Instant)>>,
}

impl AnomalyMonitorState {
    pub fn new(config_dir: PathBuf) -> Self {
        let rules_path = config_dir.join("anomaly_rules.json");
        let rules = if rules_path.exists() {
            std::fs::read(&rules_path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<AnomalyAlertRules>(&bytes).ok())
                .unwrap_or_default()
        } else {
            AnomalyAlertRules::default()
        };

        Self {
            config_dir,
            rules: Mutex::new(rules),
            active_anomalies: Mutex::new(Vec::new()),
            recent_anomalies: Mutex::new(VecDeque::new()),
            cooldowns: Mutex::new(HashMap::new()),
            prev_stats: Mutex::new(None),
        }
    }

    pub fn get_rules(&self) -> AnomalyAlertRules {
        *self.rules.lock().unwrap()
    }

    pub fn save_rules(&self, new_rules: AnomalyAlertRules) -> Result<(), AppError> {
        new_rules.validate().map_err(AppError::from)?;
        *self.rules.lock().unwrap() = new_rules;

        let rules_path = self.config_dir.join("anomaly_rules.json");
        if let Ok(bytes) = serde_json::to_vec_pretty(&new_rules) {
            let _ = std::fs::write(rules_path, bytes);
        }
        Ok(())
    }

    pub fn dismiss_all(&self) {
        self.active_anomalies.lock().unwrap().clear();
    }
}

pub fn get_or_init_state(config_dir: &Path) -> &'static AnomalyMonitorState {
    ANOMALY_STATE.get_or_init(|| AnomalyMonitorState::new(config_dir.to_path_buf()))
}

/// Spawns the anomaly detection ticker if not already running.
pub fn ensure_anomaly_monitor_running(state: AppState, config_dir: PathBuf) {
    let _ = get_or_init_state(&config_dir);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            run_evaluation_cycle(&state, &config_dir).await;
        }
    });
}

async fn run_evaluation_cycle(state: &AppState, config_dir: &Path) {
    let monitor = get_or_init_state(config_dir);
    let rules = monitor.get_rules();
    if !rules.enabled {
        monitor.active_anomalies.lock().unwrap().clear();
        return;
    }

    let now_instant = Instant::now();
    let metrics = calculate_realtime_metrics(monitor, now_instant);
    let detected = detect_anomalies(&rules, &metrics);

    update_active_and_recent_anomalies(monitor, detected, state).await;
}

struct RealtimeMetrics {
    p99_latency_ms: f64,
    error_rate_5xx_percent: f64,
    current_rps: f64,
    event_loop_lag_ms: f64,
}

fn calculate_realtime_metrics(
    monitor: &AnomalyMonitorState,
    now_instant: Instant,
) -> RealtimeMetrics {
    let (_, _, p99) = stats::latency_quantiles();
    let p99_latency_ms = p99 * 1000.0;
    let lag_micros = crate::admin::system_metrics::event_loop_lag_micros();
    let event_loop_lag_ms = lag_micros as f64 / 1000.0;

    let total_5xx = stats::snapshot_http_statuses()
        .into_iter()
        .find(|(k, _)| *k == "5xx")
        .map(|(_, c)| c)
        .unwrap_or(0);

    let (current_rps, error_rate_5xx_percent) = {
        let mut prev_guard = monitor.prev_stats.lock().unwrap();
        compute_rates_and_percentages(&mut prev_guard, total_5xx, now_instant)
    };

    RealtimeMetrics {
        p99_latency_ms,
        error_rate_5xx_percent,
        current_rps,
        event_loop_lag_ms,
    }
}

fn compute_rates_and_percentages(
    prev_guard: &mut Option<(u64, u64, Instant)>,
    total_5xx: u64,
    now_instant: Instant,
) -> (f64, f64) {
    let total_reqs = stats::snapshot_http_statuses()
        .into_iter()
        .map(|(_, c)| c)
        .sum::<u64>();

    if let Some((prev_reqs, prev_5xx, prev_time)) = *prev_guard {
        *prev_guard = Some((total_reqs, total_5xx, now_instant));
        let elapsed = now_instant.duration_since(prev_time).as_secs_f64().max(0.1);
        let delta_reqs = total_reqs.saturating_sub(prev_reqs);
        let delta_5xx = total_5xx.saturating_sub(prev_5xx);

        let rps = delta_reqs as f64 / elapsed;
        let err_pct = if delta_reqs >= 5 {
            (delta_5xx as f64 / delta_reqs as f64) * 100.0
        } else {
            0.0
        };
        (rps, err_pct)
    } else {
        *prev_guard = Some((total_reqs, total_5xx, now_instant));
        (0.0, 0.0)
    }
}

fn detect_anomalies(rules: &AnomalyAlertRules, m: &RealtimeMetrics) -> Vec<AnomalyAlert> {
    let now_ts = crate::clock::unix_ms() as i64;
    let mut alerts = Vec::new();

    if let Some(a) = check_latency_anomaly(rules, m.p99_latency_ms, now_ts) {
        alerts.push(a);
    }
    if let Some(a) = check_error_rate_anomaly(rules, m.error_rate_5xx_percent, now_ts) {
        alerts.push(a);
    }
    if let Some(a) = check_traffic_spike_anomaly(rules, m.current_rps, now_ts) {
        alerts.push(a);
    }
    if let Some(a) = check_event_loop_lag_anomaly(rules, m.event_loop_lag_ms, now_ts) {
        alerts.push(a);
    }

    alerts
}

fn check_latency_anomaly(
    rules: &AnomalyAlertRules,
    p99_ms: f64,
    now_ts: i64,
) -> Option<AnomalyAlert> {
    let max = rules.max_p99_latency_ms? as f64;
    if p99_ms > max {
        Some(AnomalyAlert {
            id: format!("latency-{now_ts}"),
            kind: AnomalyKind::HighLatency,
            severity: AnomalySeverity::Critical,
            summary: format!("High P99 latency anomaly: {p99_ms:.1} ms (threshold: {max:.0} ms)"),
            current_value: p99_ms,
            threshold_value: max,
            unit: "ms".to_string(),
            triggered_at: now_ts,
        })
    } else {
        None
    }
}

fn check_error_rate_anomaly(
    rules: &AnomalyAlertRules,
    err_pct: f64,
    now_ts: i64,
) -> Option<AnomalyAlert> {
    let max = rules.max_5xx_percent? as f64;
    if err_pct > max {
        Some(AnomalyAlert {
            id: format!("error-5xx-{now_ts}"),
            kind: AnomalyKind::HighErrorRate,
            severity: AnomalySeverity::Critical,
            summary: format!("High 5xx error rate: {err_pct:.1}% (threshold: {max:.0}%)"),
            current_value: err_pct,
            threshold_value: max,
            unit: "%".to_string(),
            triggered_at: now_ts,
        })
    } else {
        None
    }
}

fn check_traffic_spike_anomaly(
    rules: &AnomalyAlertRules,
    rps: f64,
    now_ts: i64,
) -> Option<AnomalyAlert> {
    let max = rules.max_request_rate_rps? as f64;
    if rps > max {
        Some(AnomalyAlert {
            id: format!("spike-{now_ts}"),
            kind: AnomalyKind::TrafficSpike,
            severity: AnomalySeverity::Warning,
            summary: format!("Traffic spike detected: {rps:.1} RPS (threshold: {max:.0} RPS)"),
            current_value: rps,
            threshold_value: max,
            unit: "RPS".to_string(),
            triggered_at: now_ts,
        })
    } else {
        None
    }
}

fn check_event_loop_lag_anomaly(
    rules: &AnomalyAlertRules,
    lag_ms: f64,
    now_ts: i64,
) -> Option<AnomalyAlert> {
    let max = rules.max_event_loop_lag_ms? as f64;
    if lag_ms > max {
        Some(AnomalyAlert {
            id: format!("lag-{now_ts}"),
            kind: AnomalyKind::EventLoopLag,
            severity: AnomalySeverity::Critical,
            summary: format!("Tokio event loop lag: {lag_ms:.1} ms (threshold: {max:.0} ms)"),
            current_value: lag_ms,
            threshold_value: max,
            unit: "ms".to_string(),
            triggered_at: now_ts,
        })
    } else {
        None
    }
}

async fn update_active_and_recent_anomalies(
    monitor: &AnomalyMonitorState,
    detected: Vec<AnomalyAlert>,
    state: &AppState,
) {
    {
        let mut active = monitor.active_anomalies.lock().unwrap();
        *active = detected.clone();
    }

    let mut to_dispatch = Vec::new();
    {
        let mut cooldowns = monitor.cooldowns.lock().unwrap();
        let mut recent = monitor.recent_anomalies.lock().unwrap();
        let now = Instant::now();

        for alert in &detected {
            let should_notify = if let Some(&last) = cooldowns.get(&alert.kind)
                && now.duration_since(last) < COOLDOWN_DURATION
            {
                false
            } else {
                cooldowns.insert(alert.kind, now);
                true
            };

            if should_notify {
                if recent.len() >= RECENT_MAX_CAPACITY {
                    recent.pop_back();
                }
                recent.push_front(alert.clone());
                to_dispatch.push(alert.clone());
            }
        }
    }

    for alert in to_dispatch {
        dispatch_anomaly_webhook(state, &alert).await;
    }
}

async fn dispatch_anomaly_webhook(state: &AppState, alert: &AnomalyAlert) {
    let event = match alert.kind {
        AnomalyKind::HighLatency => WebhookEvent::SystemHighLatency,
        AnomalyKind::HighErrorRate => WebhookEvent::SystemHighErrorRate,
        AnomalyKind::TrafficSpike => WebhookEvent::SystemTrafficSpike,
        AnomalyKind::EventLoopLag => WebhookEvent::SystemEventLoopLag,
    };

    let payload = serde_json::json!({
        "anomaly_id": alert.id,
        "kind": alert.kind.as_str(),
        "severity": format!("{:?}", alert.severity).to_lowercase(),
        "summary": alert.summary,
        "current_value": alert.current_value,
        "threshold_value": alert.threshold_value,
        "unit": alert.unit,
        "triggered_at": alert.triggered_at,
    });

    info!(kind = alert.kind.as_str(), summary = %alert.summary, "dispatching anomaly alert webhook");

    let platform_handle = state.data.lock().await.platform.clone();
    if let Ok(platform) = platform_handle.get() {
        platform
            .dispatch_webhook_event(event, alert.summary.clone(), payload)
            .await;
    }
}

/// Retrieves the current system anomaly detection snapshot and live metrics.
#[utoipa::path(
    get,
    path = "",
    tag = "system",
    operation_id = "get_system_anomalies",
    responses(
        (status = 200, description = "Current anomaly detection snapshot.", body = SystemAnomaliesSnapshot),
        (status = 401, description = "Unauthorized.")
    )
)]
pub async fn get_snapshot(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<SystemAnomaliesSnapshot>, AppError> {
    caller.authorize(Permission::Read).map_err(AppError::from)?;

    let config_dir = {
        let data = state.data.lock().await;
        data.app_info.config_path.clone()
    };
    let monitor = get_or_init_state(&config_dir);
    let rules = monitor.get_rules();
    let metrics = calculate_realtime_metrics(monitor, Instant::now());
    let active = monitor.active_anomalies.lock().unwrap().clone();
    let recent = monitor
        .recent_anomalies
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .collect();

    Ok(Json(SystemAnomaliesSnapshot {
        rules,
        active_anomalies: active,
        recent_anomalies: recent,
        p99_latency_ms: metrics.p99_latency_ms,
        error_rate_5xx_percent: metrics.error_rate_5xx_percent,
        current_rps: metrics.current_rps,
        event_loop_lag_ms: metrics.event_loop_lag_ms,
    }))
}

/// Retrieves current anomaly detection alert rules.
#[utoipa::path(
    get,
    path = "/rules",
    tag = "system",
    operation_id = "get_anomaly_rules",
    responses(
        (status = 200, description = "Current anomaly alert rules.", body = AnomalyAlertRules),
        (status = 401, description = "Unauthorized.")
    )
)]
pub async fn get_rules(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<AnomalyAlertRules>, AppError> {
    caller.authorize(Permission::Read).map_err(AppError::from)?;
    let config_dir = {
        let data = state.data.lock().await;
        data.app_info.config_path.clone()
    };
    let monitor = get_or_init_state(&config_dir);
    Ok(Json(monitor.get_rules()))
}

/// Updates anomaly detection alert rules.
#[utoipa::path(
    put,
    path = "/rules",
    tag = "system",
    operation_id = "update_anomaly_rules",
    request_body = AnomalyAlertRules,
    responses(
        (status = 200, description = "Anomaly alert rules updated.", body = AnomalyAlertRules),
        (status = 400, description = "Invalid anomaly alert rules."),
        (status = 401, description = "Unauthorized.")
    )
)]
pub async fn update_rules(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(rules): Json<AnomalyAlertRules>,
) -> Result<Json<AnomalyAlertRules>, AppError> {
    caller.authorize(Permission::Edit).map_err(AppError::from)?;
    let config_dir = {
        let data = state.data.lock().await;
        data.app_info.config_path.clone()
    };
    let monitor = get_or_init_state(&config_dir);
    monitor.save_rules(rules)?;

    let record = AuditRecord::new(AuditAction::UpdateNodeAlertRules).summary(format!(
        "Updated anomaly alert rules (enabled: {})",
        rules.enabled
    ));
    state
        .record_audit(&caller.username, caller.client, record)
        .await;

    Ok(Json(rules))
}

/// Dismisses all active anomaly alerts.
#[utoipa::path(
    post,
    path = "/dismiss",
    tag = "system",
    operation_id = "dismiss_anomalies",
    responses(
        (status = 200, description = "Active anomalies dismissed."),
        (status = 401, description = "Unauthorized.")
    )
)]
pub async fn dismiss(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<StatusCodeJson, AppError> {
    caller.authorize(Permission::Edit).map_err(AppError::from)?;
    let config_dir = {
        let data = state.data.lock().await;
        data.app_info.config_path.clone()
    };
    let monitor = get_or_init_state(&config_dir);
    monitor.dismiss_all();
    Ok(StatusCodeJson)
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct StatusCodeJson;

impl axum::response::IntoResponse for StatusCodeJson {
    fn into_response(self) -> axum::response::Response {
        axum::http::StatusCode::OK.into_response()
    }
}
