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

pub const MAX_VIOLATION_ENTRIES: usize = 10_000;

/// Whether `pattern` occurs in `path` on segment boundaries: it starts at a `/` and ends at the
/// end of the path or before `/`, `.`, `?`, `&` or `#`. `/.git` matches `/.git/config` and
/// `/wp-login` matches `/wp-login.php`, but `/.git` does not match `/.github`.
fn matches_segment(path: &str, pattern: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    path.match_indices(pattern).any(|(start, _)| {
        let starts = pattern.starts_with('/') || path[..start].ends_with('/');
        let ends = path[start + pattern.len()..]
            .chars()
            .next()
            .is_none_or(|next| matches!(next, '/' | '.' | '?' | '&' | '#'));
        starts && ends
    })
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

    /// Whether the address is never quarantined: loopback, or an entry of `allowed_ips`.
    pub fn is_exempt(&self, ip: &IpAddr) -> bool {
        ip.is_loopback() || self.config.load().allows(ip)
    }

    /// Checks whether an IP address is currently quarantined. An address that the allowed list
    /// gained after its ban is not quarantined.
    pub fn is_quarantined(&self, ip: &IpAddr) -> bool {
        if self.is_exempt(ip) {
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

    /// Checks if the path and the query of a request URI match any scanner patterns. The scheme
    /// and the host of an absolute URI never count, so a host such as `phpmyadmin.example.com`
    /// does not match the pattern `/phpmyadmin`. A pattern matches whole path segments only (see
    /// [`matches_segment`]).
    pub fn check_scanner_uri(&self, uri: &str) -> Option<String> {
        let config = self.config.load();
        if !config.enabled {
            return None;
        }
        let path = uri
            .parse::<http::Uri>()
            .ok()
            .and_then(|parsed| parsed.path_and_query().map(|p| p.as_str().to_owned()));
        let lower = path.as_deref().unwrap_or(uri).to_ascii_lowercase();
        config
            .scanner_patterns
            .iter()
            .find(|pattern| matches_segment(&lower, &pattern.to_ascii_lowercase()))
            .cloned()
    }

    /// Checks if an HTTP response status code is considered a violation.
    pub fn is_violating_status(&self, status: u16) -> bool {
        let config = self.config.load();
        config.enabled && config.block_status_codes.contains(&status)
    }

    fn prune_violations(
        &self,
        map: &mut HashMap<IpAddr, IpViolationState>,
        now: Instant,
        window_seconds: u64,
    ) {
        map.retain(|_, state| now.duration_since(state.window_start).as_secs() < window_seconds);
        if map.len() >= MAX_VIOLATION_ENTRIES {
            let excess = map.len() - MAX_VIOLATION_ENTRIES + 1;
            let mut entries: Vec<(IpAddr, Instant)> =
                map.iter().map(|(ip, s)| (*ip, s.window_start)).collect();
            entries.sort_by_key(|(_, t)| *t);
            for (ip, _) in entries.into_iter().take(excess) {
                map.remove(&ip);
            }
        }
    }

    /// Prunes expired violation records from memory.
    pub fn cleanup_violations(&self, now: Instant) {
        let config = self.config.load();
        let mut map = match self.violations.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        self.prune_violations(&mut map, now, config.window_seconds);
    }

    /// Records violations for an IP address. If the threshold is exceeded, quarantines the IP.
    pub fn record_violation(
        &self,
        ip: IpAddr,
        reason: String,
        weight: u32,
    ) -> Option<QuarantinedIpEntry> {
        let config = self.config.load();
        if !config.enabled || self.is_exempt(&ip) || self.is_quarantined(&ip) {
            return None;
        }

        let now = Instant::now();
        let mut map = match self.violations.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        self.prune_violations(&mut map, now, config.window_seconds);

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

    /// Applies a quarantine that another process of this server decided, without a notification,
    /// so the change does not travel back to that process.
    pub fn restore(&self, entry: &QuarantinedIpEntry) {
        self.insert_ban(
            entry.ip,
            entry.reason.clone(),
            entry.remaining_seconds,
            entry.violations,
            Instant::now(),
        );
    }

    /// Applies a quarantine of another process of this server and notifies about it, for the
    /// webhook of the master.
    pub fn record(&self, entry: &QuarantinedIpEntry) {
        self.restore(entry);
        self.notify_quarantine(entry);
    }

    fn quarantine_internal(
        &self,
        ip: IpAddr,
        reason: String,
        duration_seconds: u64,
        violations: u32,
        now: Instant,
    ) -> QuarantinedIpEntry {
        let result = self.insert_ban(ip, reason, duration_seconds, violations, now);
        tracing::warn!(
            %ip,
            reason = %result.reason,
            violations,
            seconds = result.remaining_seconds,
            "quarantined an address"
        );
        self.notify_quarantine(&result);
        result
    }

    fn insert_ban(
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

        QuarantinedIpEntry {
            ip,
            reason,
            violations,
            banned_at,
            banned_until: banned_until_ms,
            remaining_seconds: duration_seconds.max(1),
        }
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
            allowed_ips: Vec::new(),
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
    fn a_restored_quarantine_blocks_without_a_notification() {
        let tracker = AutoQuarantineTracker::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tracker.set_notifier(tx);
        let entry = QuarantinedIpEntry {
            ip: "203.0.113.70".parse().unwrap(),
            reason: "Worker".to_string(),
            violations: 5,
            banned_at: 0,
            banned_until: 0,
            remaining_seconds: 600,
        };
        tracker.restore(&entry);
        assert!(tracker.is_quarantined(&entry.ip));
        assert!(rx.try_recv().is_err());
        tracker.record(&entry);
        assert_eq!(rx.try_recv().unwrap().ip, entry.ip);
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
    fn a_scanner_pattern_matches_whole_path_segments_only() {
        let tracker = AutoQuarantineTracker::default();
        assert_eq!(
            tracker.check_scanner_uri("/.git/config"),
            Some("/.git".to_string())
        );
        assert_eq!(
            tracker.check_scanner_uri("/wp-login.php"),
            Some("/wp-login".to_string())
        );
        assert_eq!(
            tracker.check_scanner_uri("/.env.local"),
            Some("/.env".to_string())
        );
        assert_eq!(tracker.check_scanner_uri("/.github/workflows/ci.yml"), None);
        assert_eq!(tracker.check_scanner_uri("/docs/.environment"), None);
        assert_eq!(tracker.check_scanner_uri("/app/config.jsonp"), None);
    }

    #[test]
    fn a_scanner_pattern_ignores_the_host_of_an_absolute_uri() {
        let tracker = AutoQuarantineTracker::default();
        assert_eq!(
            tracker.check_scanner_uri("https://phpmyadmin.example.com/"),
            None
        );
        assert_eq!(
            tracker.check_scanner_uri("https://phpmyadmin.example.com/index.php?route=/"),
            None
        );
        assert_eq!(
            tracker.check_scanner_uri("https://app.example.com/phpmyadmin/index.php"),
            Some("/phpmyadmin".to_string())
        );
        assert_eq!(
            tracker.check_scanner_uri("https://app.example.com/?next=/.env"),
            Some("/.env".to_string())
        );
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

    #[test]
    fn an_allowed_address_is_never_quarantined() {
        let tracker = AutoQuarantineTracker::new(AutoQuarantineConfig {
            allowed_ips: vec!["198.51.100.0/24".parse().unwrap()],
            ..Default::default()
        });
        let allowed: IpAddr = "198.51.100.9".parse().unwrap();
        assert_eq!(
            tracker.record_violation(allowed, "Probe".to_string(), 100),
            None
        );
        assert!(!tracker.is_quarantined(&allowed));
    }

    #[test]
    fn allowing_a_banned_address_lifts_its_ban() {
        let tracker = AutoQuarantineTracker::default();
        let ip: IpAddr = "198.51.100.10".parse().unwrap();
        tracker.quarantine(ip, "Manual".to_string(), 3600);
        assert!(tracker.is_quarantined(&ip));
        tracker.update_config(AutoQuarantineConfig {
            allowed_ips: vec!["198.51.100.10/32".parse().unwrap()],
            ..Default::default()
        });
        assert!(!tracker.is_quarantined(&ip));
    }

    #[test]
    fn violations_retention_pruning_and_bounded_capacity() {
        let config = AutoQuarantineConfig {
            enabled: true,
            max_violations: 10,
            window_seconds: 60,
            ban_duration_seconds: 120,
            allowed_ips: Vec::new(),
            block_status_codes: vec![404],
            scanner_patterns: vec![],
        };
        let tracker = AutoQuarantineTracker::new(config);
        let now = Instant::now();

        // 1. Insert a stale entry older than window_seconds
        {
            let mut map = tracker.violations.lock().unwrap();
            let old_ip: IpAddr = "198.51.100.1".parse().unwrap();
            map.insert(
                old_ip,
                IpViolationState {
                    count: 1,
                    window_start: now - Duration::from_secs(120),
                },
            );
            assert_eq!(map.len(), 1);
        }

        // 2. Recording violation prunes the stale entry
        let new_ip: IpAddr = "198.51.100.2".parse().unwrap();
        tracker.record_violation(new_ip, "Probe".to_string(), 1);

        {
            let map = tracker.violations.lock().unwrap();
            assert_eq!(map.len(), 1);
            let old_ip: IpAddr = "198.51.100.1".parse().unwrap();
            assert!(!map.contains_key(&old_ip));
            assert!(map.contains_key(&new_ip));
        }
    }
}
