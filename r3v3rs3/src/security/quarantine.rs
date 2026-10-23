//! In-memory and persisted IP threat detection and auto-quarantine tracker.

use crate::clock::unix_ms;
use arc_swap::ArcSwap;
use r3v3rs3_api::quarantine::{AutoQuarantineConfig, QuarantinedIpEntry};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct ActiveBan {
    reason: String,
    violations: u32,
    banned_at_ms: u64,
    banned_until_ms: u64,
    banned_until: Instant,
}

#[derive(Debug, Clone)]
struct IpViolationState {
    count: u32,
    window_start: Instant,
}

/// Thread-safe tracker for client IP violations and quarantine state.
#[derive(Debug)]
pub struct AutoQuarantineTracker {
    config: ArcSwap<AutoQuarantineConfig>,
    active_bans: RwLock<HashMap<IpAddr, ActiveBan>>,
    violations: Mutex<HashMap<IpAddr, IpViolationState>>,
    notifier: RwLock<Option<tokio::sync::mpsc::UnboundedSender<QuarantinedIpEntry>>>,
}

impl Default for AutoQuarantineTracker {
    fn default() -> Self {
        Self::new(AutoQuarantineConfig::default())
    }
}

impl AutoQuarantineTracker {
    pub fn new(config: AutoQuarantineConfig) -> Self {
        Self {
            config: ArcSwap::from_pointee(config),
            active_bans: RwLock::new(HashMap::new()),
            violations: Mutex::new(HashMap::new()),
            notifier: RwLock::new(None),
        }
    }

    /// Sets the sender for quarantine event notifications.
    pub fn set_notifier(&self, tx: tokio::sync::mpsc::UnboundedSender<QuarantinedIpEntry>) {
        if let Ok(mut guard) = self.notifier.write() {
            *guard = Some(tx);
        }
    }

    /// Checks whether an IP address is currently quarantined.
    pub fn is_quarantined(&self, ip: &IpAddr) -> bool {
        if ip.is_loopback() {
            return false;
        }
        let bans = match self.active_bans.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(ban) = bans.get(ip)
            && Instant::now() < ban.banned_until
        {
            return true;
        }
        false
    }

    /// Checks if a request URI matches any scanner patterns.
    pub fn check_scanner_uri(&self, uri: &str) -> Option<String> {
        let config = self.config.load();
        if !config.enabled {
            return None;
        }
        let lower = uri.to_ascii_lowercase();
        for pattern in &config.scanner_patterns {
            if lower.contains(&pattern.to_ascii_lowercase()) {
                return Some(pattern.clone());
            }
        }
        None
    }

    /// Checks if an HTTP response status code is considered a violation.
    pub fn is_violating_status(&self, status: u16) -> bool {
        let config = self.config.load();
        config.enabled && config.block_status_codes.contains(&status)
    }

    /// Records violations for an IP address. If the threshold is exceeded, quarantines the IP.
    pub fn record_violation(
        &self,
        ip: IpAddr,
        reason: String,
        weight: u32,
    ) -> Option<QuarantinedIpEntry> {
        let config = self.config.load();
        if !config.enabled || ip.is_loopback() || self.is_quarantined(&ip) {
            return None;
        }

        let now = Instant::now();
        let mut map = match self.violations.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let entry = map.entry(ip).or_insert_with(|| IpViolationState {
            count: 0,
            window_start: now,
        });

        if now.duration_since(entry.window_start).as_secs() >= config.window_seconds {
            entry.count = 0;
            entry.window_start = now;
        }

        entry.count = entry.count.saturating_add(weight);
        if entry.count >= config.max_violations {
            let total = entry.count;
            map.remove(&ip);
            drop(map);
            return Some(self.quarantine_internal(
                ip,
                reason,
                config.ban_duration_seconds,
                total,
                now,
            ));
        }

        None
    }

    /// Manually or programmatically quarantines an IP address for the specified duration.
    pub fn quarantine(
        &self,
        ip: IpAddr,
        reason: String,
        duration_seconds: u64,
    ) -> QuarantinedIpEntry {
        let now = Instant::now();
        self.quarantine_internal(ip, reason, duration_seconds, 1, now)
    }

    fn quarantine_internal(
        &self,
        ip: IpAddr,
        reason: String,
        duration_seconds: u64,
        violations: u32,
        now: Instant,
    ) -> QuarantinedIpEntry {
        let duration = Duration::from_secs(duration_seconds.max(1));
        let banned_at = unix_ms();
        let banned_until_ms = banned_at.saturating_add(duration_seconds.max(1) * 1000);
        let banned_until = now + duration;

        let entry = ActiveBan {
            reason: reason.clone(),
            violations,
            banned_at_ms: banned_at,
            banned_until_ms,
            banned_until,
        };

        let mut bans = match self.active_bans.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        bans.insert(ip, entry);

        let result = QuarantinedIpEntry {
            ip,
            reason,
            violations,
            banned_at,
            banned_until: banned_until_ms,
            remaining_seconds: duration_seconds.max(1),
        };

        self.notify_quarantine(&result);
        result
    }

    fn notify_quarantine(&self, entry: &QuarantinedIpEntry) {
        if let Ok(guard) = self.notifier.read()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(entry.clone());
        }
    }

    /// Releases an IP address from quarantine.
    pub fn unban(&self, ip: &IpAddr) -> bool {
        let mut bans = match self.active_bans.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let removed = bans.remove(ip).is_some();

        let mut map = match self.violations.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        map.remove(ip);

        removed
    }

    /// Lists all currently active quarantined IP addresses.
    pub fn list_quarantined(&self) -> Vec<QuarantinedIpEntry> {
        let now = Instant::now();
        let mut bans = match self.active_bans.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        bans.retain(|_, ban| ban.banned_until > now);

        let mut list: Vec<QuarantinedIpEntry> = bans
            .iter()
            .map(|(ip, ban)| {
                let remaining = ban.banned_until.duration_since(now).as_secs().max(1);
                QuarantinedIpEntry {
                    ip: *ip,
                    reason: ban.reason.clone(),
                    violations: ban.violations,
                    banned_at: ban.banned_at_ms,
                    banned_until: ban.banned_until_ms,
                    remaining_seconds: remaining,
                }
            })
            .collect();

        list.sort_by_key(|e| e.banned_until);
        list
    }

    /// Gets current auto-quarantine configuration.
    pub fn get_config(&self) -> AutoQuarantineConfig {
        (**self.config.load()).clone()
    }

    /// Updates auto-quarantine configuration.
    pub fn update_config(&self, config: AutoQuarantineConfig) {
        self.config.store(Arc::new(config));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_and_unban_flow() {
        let tracker = AutoQuarantineTracker::default();
        let ip: IpAddr = "203.0.113.88".parse().unwrap();

        assert!(!tracker.is_quarantined(&ip));

        let entry = tracker.quarantine(ip, "Test manual quarantine".to_string(), 3600);
        assert_eq!(entry.ip, ip);
        assert!(tracker.is_quarantined(&ip));

        let list = tracker.list_quarantined();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ip, ip);

        assert!(tracker.unban(&ip));
        assert!(!tracker.is_quarantined(&ip));
        assert_eq!(tracker.list_quarantined().len(), 0);
    }

    #[test]
    fn violation_threshold_quarantines() {
        let config = AutoQuarantineConfig {
            enabled: true,
            max_violations: 3,
            window_seconds: 60,
            ban_duration_seconds: 120,
            block_status_codes: vec![404],
            scanner_patterns: vec!["/.env".to_string()],
        };
        let tracker = AutoQuarantineTracker::new(config);
        let ip: IpAddr = "192.0.2.77".parse().unwrap();

        assert_eq!(
            tracker.record_violation(ip, "Path 404".to_string(), 1),
            None
        );
        assert!(!tracker.is_quarantined(&ip));

        assert_eq!(
            tracker.record_violation(ip, "Path 404".to_string(), 1),
            None
        );
        assert!(!tracker.is_quarantined(&ip));

        let quarantined = tracker.record_violation(ip, "Scanner /.env".to_string(), 1);
        assert!(quarantined.is_some());
        assert!(tracker.is_quarantined(&ip));
    }

    #[test]
    fn scanner_path_detection() {
        let tracker = AutoQuarantineTracker::default();
        assert_eq!(
            tracker.check_scanner_uri("/app/.ENV"),
            Some("/.env".to_string())
        );
        assert_eq!(
            tracker.check_scanner_uri("/WP-ADMIN/index.php"),
            Some("/wp-admin".to_string())
        );
        assert_eq!(tracker.check_scanner_uri("/api/users"), None);
    }

    #[test]
    fn loopback_is_never_quarantined() {
        let tracker = AutoQuarantineTracker::default();
        let loopback_v4: IpAddr = "127.0.0.1".parse().unwrap();
        let loopback_v6: IpAddr = "::1".parse().unwrap();

        assert!(!tracker.is_quarantined(&loopback_v4));
        assert!(!tracker.is_quarantined(&loopback_v6));

        assert_eq!(
            tracker.record_violation(loopback_v4, "Probe".to_string(), 100),
            None
        );
        assert!(!tracker.is_quarantined(&loopback_v4));
    }
}
