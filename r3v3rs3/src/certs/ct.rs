//! Certificate Transparency (CT) log monitor and rogue certificate detection engine.

use crate::admin::AppState;
use crate::server::rpc::acme::GetAcmeList;
use crate::server::rpc::certs::GetCertList;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::ct::{
    CtAlertStatus, CtEntriesQuery, CtLogEntry, CtMonitorConfig, CtMonitorStatus, parse_crt_sh_entry,
};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::webhook::WebhookEvent;
use serde_derive::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::warn;

static CT_STATES: Mutex<Option<HashMap<PathBuf, Arc<CtMonitorState>>>> = Mutex::new(None);
const MAX_ENTRIES_CAPACITY: usize = 500;
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Serialize, Deserialize)]
struct CtPersistedData {
    config: CtMonitorConfig,
    entries: Vec<CtLogEntry>,
    last_check_at: Option<i64>,
}

pub struct CtMonitorState {
    config_dir: PathBuf,
    config: Mutex<CtMonitorConfig>,
    entries: Mutex<Vec<CtLogEntry>>,
    status: Mutex<CtMonitorStatus>,
    last_runtime_check: Mutex<Option<Instant>>,
}

impl CtMonitorState {
    pub fn new(config_dir: PathBuf) -> Self {
        let file_path = config_dir.join("ct_monitor.json");
        let (config, entries, last_check_at) = load_persisted_data(&file_path);
        let active_rogues = count_active_rogues(&entries);
        let domains = config.monitored_domains.clone();
        let total_count = entries.len();

        let status = CtMonitorStatus {
            enabled: config.enabled,
            last_check_at,
            next_check_at: None,
            monitored_domains: domains,
            total_entries_count: total_count,
            active_rogue_alerts_count: active_rogues,
            last_check_status: "idle".to_string(),
            last_error: None,
        };

        Self {
            config_dir,
            config: Mutex::new(config),
            entries: Mutex::new(entries),
            status: Mutex::new(status),
            last_runtime_check: Mutex::new(None),
        }
    }

    pub fn get_config(&self) -> CtMonitorConfig {
        self.config.lock().unwrap().clone()
    }

    pub fn save_config(&self, new_config: CtMonitorConfig) -> Result<(), Error> {
        let mut cfg = self.config.lock().unwrap();
        *cfg = new_config.clone();
        drop(cfg);

        let mut st = self.status.lock().unwrap();
        st.enabled = new_config.enabled;
        st.monitored_domains = new_config.monitored_domains.clone();
        drop(st);

        self.persist_to_disk();
        Ok(())
    }

    pub fn get_status(&self) -> CtMonitorStatus {
        let mut st = self.status.lock().unwrap().clone();
        let entries = self.entries.lock().unwrap();
        st.active_rogue_alerts_count = count_active_rogues(&entries);
        st.total_entries_count = entries.len();
        st
    }

    pub fn list_entries(&self, query: CtEntriesQuery) -> Vec<CtLogEntry> {
        let entries = self.entries.lock().unwrap();
        let domain_filter = query.domain.as_deref().map(|d| d.to_lowercase());
        let rogue_only = query.rogue_only.unwrap_or(false);
        let limit = query.limit.unwrap_or(100).min(MAX_ENTRIES_CAPACITY);

        entries
            .iter()
            .filter(|e| filter_entry(e, domain_filter.as_deref(), rogue_only))
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn dismiss_alert(&self, id: &str, status: Option<CtAlertStatus>) -> Result<(), Error> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries
            .iter_mut()
            .find(|e| e.id == id)
            .ok_or_else(|| Error::CtEntryNotFound { id: id.to_string() })?;

        entry.status = status.unwrap_or(CtAlertStatus::Dismissed);
        drop(entries);

        self.persist_to_disk();
        Ok(())
    }

    pub async fn run_check(
        &self,
        state: &AppState,
        specific_domain: Option<&str>,
    ) -> Result<usize, Error> {
        let config = self.get_config();
        let domains = collect_target_domains(state, &config, specific_domain).await;
        if domains.is_empty() {
            return Ok(0);
        }

        self.update_status_checking(&domains);
        let client = build_http_client().map_err(|e| Error::CtCheckFailed { reason: e })?;

        let mut newly_discovered_rogues = Vec::new();
        let mut fetch_errors = Vec::new();

        for domain in &domains {
            match fetch_entries_for_domain(&client, domain, &config).await {
                Ok(found) => {
                    let mut rogues = self.integrate_entries(found);
                    newly_discovered_rogues.append(&mut rogues);
                }
                Err(err) => {
                    warn!(domain = %domain, error = %err, "failed to query CT log for domain");
                    fetch_errors.push(format!("{domain}: {err}"));
                }
            }
        }

        let now_unix = current_unix_timestamp();
        self.finalize_status(now_unix, &fetch_errors, &config);
        self.persist_to_disk();

        dispatch_rogue_notifications(state, &newly_discovered_rogues).await;
        if !fetch_errors.is_empty() && newly_discovered_rogues.is_empty() {
            dispatch_failure_notification(state, &fetch_errors.join(", ")).await;
        }

        Ok(newly_discovered_rogues.len())
    }

    fn integrate_entries(&self, found: Vec<CtLogEntry>) -> Vec<CtLogEntry> {
        let mut entries = self.entries.lock().unwrap();
        let mut new_rogues = Vec::new();
        let existing_map: HashMap<String, CtAlertStatus> =
            entries.iter().map(|e| (e.id.clone(), e.status)).collect();

        for mut item in found {
            if let Some(&prev_status) = existing_map.get(&item.id) {
                item.status = prev_status;
            } else if item.is_rogue && item.status == CtAlertStatus::Investigating {
                new_rogues.push(item.clone());
            }
            if !entries.iter().any(|e| e.id == item.id) {
                entries.insert(0, item);
            }
        }

        if entries.len() > MAX_ENTRIES_CAPACITY {
            entries.truncate(MAX_ENTRIES_CAPACITY);
        }
        new_rogues
    }

    fn update_status_checking(&self, domains: &[String]) {
        let mut st = self.status.lock().unwrap();
        st.last_check_status = "checking".to_string();
        st.monitored_domains = domains.to_vec();
        *self.last_runtime_check.lock().unwrap() = Some(Instant::now());
    }

    fn finalize_status(&self, now: i64, errors: &[String], config: &CtMonitorConfig) {
        let mut st = self.status.lock().unwrap();
        st.last_check_at = Some(now);
        st.next_check_at = Some(now + config.check_interval_secs as i64);
        if errors.is_empty() {
            st.last_check_status = "ok".to_string();
            st.last_error = None;
        } else {
            st.last_check_status = "partial_error".to_string();
            st.last_error = Some(errors.join("; "));
        }
    }

    fn persist_to_disk(&self) {
        let data = CtPersistedData {
            config: self.get_config(),
            entries: self.entries.lock().unwrap().clone(),
            last_check_at: self.status.lock().unwrap().last_check_at,
        };
        let file_path = self.config_dir.join("ct_monitor.json");
        if let Ok(bytes) = serde_json::to_vec_pretty(&data) {
            let _ = std::fs::write(file_path, bytes);
        }
    }
}

fn entry_matches_domain(entry: &CtLogEntry, domain: &str) -> bool {
    entry.domain.to_lowercase().contains(domain)
        || entry.common_name.to_lowercase().contains(domain)
        || entry.san.iter().any(|s| s.to_lowercase().contains(domain))
}

fn filter_entry(entry: &CtLogEntry, domain_filter: Option<&str>, rogue_only: bool) -> bool {
    if rogue_only && (!entry.is_rogue || entry.status == CtAlertStatus::Dismissed) {
        return false;
    }
    if let Some(domain) = domain_filter
        && !entry_matches_domain(entry, domain)
    {
        return false;
    }
    true
}

fn count_active_rogues(entries: &[CtLogEntry]) -> usize {
    entries
        .iter()
        .filter(|e| e.is_rogue && e.status != CtAlertStatus::Dismissed)
        .count()
}

fn load_persisted_data(file_path: &Path) -> (CtMonitorConfig, Vec<CtLogEntry>, Option<i64>) {
    if !file_path.exists() {
        return (CtMonitorConfig::default(), Vec::new(), None);
    }
    let Ok(bytes) = std::fs::read(file_path) else {
        return (CtMonitorConfig::default(), Vec::new(), None);
    };
    let Ok(data) = serde_json::from_slice::<CtPersistedData>(&bytes) else {
        return (CtMonitorConfig::default(), Vec::new(), None);
    };
    (data.config, data.entries, data.last_check_at)
}

fn current_unix_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn build_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent("r3v3rs3-ct-monitor/1.0")
        .build()
        .map_err(|e| e.to_string())
}

async fn collect_target_domains(
    state: &AppState,
    config: &CtMonitorConfig,
    specific: Option<&str>,
) -> Vec<String> {
    if let Some(d) = specific {
        let trimmed = d.trim().to_string();
        return if trimmed.is_empty() {
            vec![]
        } else {
            vec![trimmed]
        };
    }

    let mut set = HashSet::new();
    for d in &config.monitored_domains {
        let t = d.trim().to_string();
        if !t.is_empty() {
            set.insert(t);
        }
    }

    if config.auto_discover_domains {
        discover_active_domains(state, &mut set).await;
    }

    set.into_iter().collect()
}

async fn discover_active_domains(state: &AppState, set: &mut HashSet<String>) {
    let dummy_caller = crate::accounts::Caller {
        username: "system".to_string(),
        role: Role::Admin,
        proxies: None,
        client: None,
    };
    if let Ok(certs) = state.call(&dummy_caller, GetCertList).await {
        for c in certs.iter() {
            for san in &c.san {
                add_domain_if_valid(set, &san.to_string());
            }
        }
    }
    if let Ok(acmes) = state.call(&dummy_caller, GetAcmeList).await {
        for a in acmes.iter() {
            for id in &a.identifiers {
                add_domain_if_valid(set, id);
            }
        }
    }
}

fn add_domain_if_valid(set: &mut HashSet<String>, domain: &str) {
    let clean = domain.trim().trim_start_matches("*.").to_string();
    if !clean.is_empty() && clean.contains('.') && !clean.contains(':') {
        set.insert(clean);
    }
}

async fn fetch_entries_for_domain(
    client: &reqwest::Client,
    domain: &str,
    config: &CtMonitorConfig,
) -> Result<Vec<CtLogEntry>, String> {
    let template = config
        .ct_api_url
        .as_deref()
        .unwrap_or("https://crt.sh/?q={domain}&output=json");
    let url = template.replace("{domain}", domain);

    let res = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("request error: {e}"))?;

    if !res.status().is_success() {
        return Err(format!("http status {}", res.status()));
    }

    let json_val = res
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("json parse error: {e}"))?;

    let arr = json_val
        .as_array()
        .ok_or_else(|| "response is not an array".to_string())?;

    let entries: Vec<CtLogEntry> = arr
        .iter()
        .filter_map(|item| {
            parse_crt_sh_entry(
                item,
                domain,
                &config.allowed_issuers,
                config.alert_on_unknown_issuer,
            )
        })
        .collect();

    Ok(entries)
}

async fn dispatch_rogue_notifications(state: &AppState, rogues: &[CtLogEntry]) {
    if rogues.is_empty() {
        return;
    }
    let platform_handle = state.data.lock().await.platform.clone();
    let Ok(platform) = platform_handle.get() else {
        return;
    };

    for rogue in rogues {
        let summary = format!(
            "Rogue or suspicious certificate detected for {}: issuer {}",
            rogue.domain, rogue.issuer_name
        );
        let payload = serde_json::json!({
            "id": rogue.id,
            "domain": rogue.domain,
            "issuer_name": rogue.issuer_name,
            "common_name": rogue.common_name,
            "san": rogue.san,
            "serial_number": rogue.serial_number,
            "logged_at": rogue.logged_at,
            "not_before": rogue.not_before,
            "not_after": rogue.not_after,
            "rogue_reason": rogue.rogue_reason,
            "status": "investigating",
        });
        platform
            .dispatch_webhook_event(WebhookEvent::CertRogueDetected, summary, payload)
            .await;
    }
}

async fn dispatch_failure_notification(state: &AppState, reason: &str) {
    let platform_handle = state.data.lock().await.platform.clone();
    let Ok(platform) = platform_handle.get() else {
        return;
    };
    let summary = format!("Certificate Transparency log check failed: {reason}");
    let payload = serde_json::json!({
        "reason": reason,
    });
    platform
        .dispatch_webhook_event(WebhookEvent::CertCtCheckFailed, summary, payload)
        .await;
}

pub fn get_or_init_state(config_dir: &Path) -> Arc<CtMonitorState> {
    let mut guard = CT_STATES.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(state) = map.get(config_dir) {
        state.clone()
    } else {
        let state = Arc::new(CtMonitorState::new(config_dir.to_path_buf()));
        map.insert(config_dir.to_path_buf(), state.clone());
        state
    }
}

/// Spawns the background monitor loop if not already running.
pub fn ensure_ct_monitor_running(state: AppState, config_dir: PathBuf) {
    let _ = get_or_init_state(&config_dir);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        loop {
            interval.tick().await;
            check_scheduled_tick(&state, &config_dir).await;
        }
    });
}

async fn check_scheduled_tick(state: &AppState, config_dir: &Path) {
    let monitor = get_or_init_state(config_dir);
    let config = monitor.get_config();
    if !config.enabled {
        return;
    }

    let status = monitor.get_status();
    let now = current_unix_timestamp();
    let should_run = match status.next_check_at {
        Some(next) => now >= next,
        None => true,
    };

    if should_run {
        let _ = monitor.run_check(state, None).await;
    }
}
