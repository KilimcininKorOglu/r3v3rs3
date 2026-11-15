//! Login throttling: failed password logins per client IP and username, and failed TOTP codes
//! per session and account. The admin sign-in and the session authentication of proxies share
//! these counters.

use std::{
    collections::HashMap,
    net::IpAddr,
    time::{Duration, Instant},
};

pub type LoginAttemptKey = (IpAddr, String);

pub const MAX_LOGIN_USERNAME_LEN: usize = 128;
pub const MAX_LOGIN_ATTEMPTS_ENTRIES: usize = 10_000;

pub fn bounded_key(key: &LoginAttemptKey) -> LoginAttemptKey {
    if key.1.len() <= MAX_LOGIN_USERNAME_LEN {
        key.clone()
    } else {
        (key.0, key.1[..MAX_LOGIN_USERNAME_LEN].to_string())
    }
}

#[derive(Debug, Clone, Copy)]
struct LoginAttempt {
    failures: u32,
    in_flight: u32,
    window_start: Instant,
    blocked_until: Option<Instant>,
}

/// Counts failed logins per client IP and username.
#[derive(Default)]
pub struct LoginAttempts {
    entries: HashMap<LoginAttemptKey, LoginAttempt>,
}

impl LoginAttempts {
    /// Attempts to acquire an in-flight login attempt slot in O(1) time.
    pub fn acquire(
        &mut self,
        key: &LoginAttemptKey,
        max_attempts: u32,
        reset: Duration,
        now: Instant,
    ) -> bool {
        let key = bounded_key(key);
        let max = max_attempts.max(1);
        let attempt = self.get_or_insert_attempt(key, reset, now);

        if attempt.blocked_until.is_some_and(|until| until > now) {
            return false;
        }
        if attempt.failures.saturating_add(attempt.in_flight) >= max {
            return false;
        }
        attempt.in_flight += 1;
        true
    }

    /// Releases an in-flight attempt slot without recording a failure.
    pub fn release(&mut self, key: &LoginAttemptKey) {
        let key = bounded_key(key);
        if let Some(attempt) = self.entries.get_mut(&key) {
            attempt.in_flight = attempt.in_flight.saturating_sub(1);
        }
    }

    /// Returns true while the key is blocked. O(1) lookup without scanning the whole map.
    pub fn is_blocked(&self, key: &LoginAttemptKey, reset: Duration, now: Instant) -> bool {
        let key = bounded_key(key);
        self.entries.get(&key).is_some_and(|attempt| {
            attempt.is_active(reset, now) && attempt.blocked_until.is_some_and(|until| until > now)
        })
    }

    pub fn record_failure(
        &mut self,
        key: LoginAttemptKey,
        max_attempts: u32,
        reset: Duration,
        now: Instant,
    ) {
        let key = bounded_key(&key);
        let attempt = self.get_or_insert_attempt(key, reset, now);
        attempt.in_flight = attempt.in_flight.saturating_sub(1);
        attempt.failures += 1;
        if attempt.failures >= max_attempts.max(1) {
            attempt.blocked_until = Some(now + reset);
        }
    }

    pub fn clear(&mut self, key: &LoginAttemptKey) {
        let key = bounded_key(key);
        self.entries.remove(&key);
    }

    /// Prunes expired attempt entries outside the request path.
    pub fn prune_expired(&mut self, reset: Duration, now: Instant) {
        self.entries
            .retain(|_, attempt| attempt.is_active(reset, now));
    }

    fn prune_oldest(&mut self) {
        if let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, attempt)| attempt.window_start)
            .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
    }

    fn get_or_insert_attempt(
        &mut self,
        key: LoginAttemptKey,
        reset: Duration,
        now: Instant,
    ) -> &mut LoginAttempt {
        if !self.entries.contains_key(&key) && self.entries.len() >= MAX_LOGIN_ATTEMPTS_ENTRIES {
            self.prune_expired(reset, now);
            if self.entries.len() >= MAX_LOGIN_ATTEMPTS_ENTRIES {
                self.prune_oldest();
            }
        }
        let attempt = self
            .entries
            .entry(key)
            .or_insert_with(|| LoginAttempt::new(now));
        if !attempt.is_active(reset, now) {
            *attempt = LoginAttempt::new(now);
        }
        attempt
    }
}

impl LoginAttempt {
    fn new(now: Instant) -> Self {
        Self {
            failures: 0,
            in_flight: 0,
            window_start: now,
            blocked_until: None,
        }
    }

    fn is_active(&self, reset: Duration, now: Instant) -> bool {
        if self.in_flight > 0 {
            return true;
        }
        match self.blocked_until {
            Some(until) => until > now,
            None => now.duration_since(self.window_start) < reset,
        }
    }
}

pub const MAX_SESSION_TOTP_ATTEMPTS: u32 = 3;

#[derive(Default)]
pub struct TotpAttempts {
    sessions: HashMap<String, u32>,
    accounts: HashMap<String, LoginAttempt>,
}

impl TotpAttempts {
    pub fn record_failure(
        &mut self,
        session_token: &str,
        username: &str,
        max_account_attempts: u32,
        reset: Duration,
        now: Instant,
    ) -> bool {
        let session_fails = self.sessions.entry(session_token.to_string()).or_insert(0);
        *session_fails += 1;
        let session_exhausted = *session_fails >= MAX_SESSION_TOTP_ATTEMPTS;
        if session_exhausted {
            self.sessions.remove(session_token);
        }

        let max = max_account_attempts.max(1);
        let attempt = self
            .accounts
            .entry(username.to_string())
            .or_insert(LoginAttempt {
                failures: 0,
                in_flight: 0,
                window_start: now,
                blocked_until: None,
            });
        attempt.failures += 1;
        if attempt.failures >= max {
            attempt.blocked_until = Some(now + reset);
        }

        session_exhausted
    }

    pub fn is_account_blocked(&mut self, username: &str, reset: Duration, now: Instant) -> bool {
        self.accounts
            .retain(|_, attempt| attempt.is_active(reset, now));
        self.accounts
            .get(username)
            .and_then(|attempt| attempt.blocked_until)
            .is_some_and(|until| until > now)
    }

    pub fn clear(&mut self, session_token: &str, username: &str) {
        self.sessions.remove(session_token);
        self.accounts.remove(username);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::client_ip::canonical_client_ip;
    use std::net::Ipv4Addr;

    fn key(name: &str) -> LoginAttemptKey {
        (IpAddr::V4(Ipv4Addr::LOCALHOST), name.to_string())
    }

    #[test]
    fn blocks_after_max_failures_until_reset() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        attempts.record_failure(key("admin"), 2, reset, now);
        assert!(!attempts.is_blocked(&key("admin"), reset, now));

        attempts.record_failure(key("admin"), 2, reset, now);
        assert!(attempts.is_blocked(&key("admin"), reset, now));
        assert!(!attempts.is_blocked(&key("other"), reset, now));

        let later = now + reset + Duration::from_secs(1);
        assert!(!attempts.is_blocked(&key("admin"), reset, later));
    }

    #[test]
    fn failures_outside_window_are_forgotten() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        attempts.record_failure(key("admin"), 2, reset, now);
        let later = now + reset + Duration::from_secs(1);
        assert!(!attempts.is_blocked(&key("admin"), reset, later));

        attempts.record_failure(key("admin"), 2, reset, later);
        assert!(!attempts.is_blocked(&key("admin"), reset, later));
    }

    #[test]
    fn clear_resets_failures() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        attempts.record_failure(key("admin"), 2, reset, now);
        attempts.clear(&key("admin"));
        attempts.record_failure(key("admin"), 2, reset, now);
        assert!(!attempts.is_blocked(&key("admin"), reset, now));
    }

    #[test]
    fn in_flight_attempts_block_concurrent_burst_exceeding_threshold() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        assert!(attempts.acquire(&key("admin"), 2, reset, now));
        assert!(attempts.acquire(&key("admin"), 2, reset, now));
        assert!(!attempts.acquire(&key("admin"), 2, reset, now));

        attempts.release(&key("admin"));
        assert!(attempts.acquire(&key("admin"), 2, reset, now));
    }

    #[test]
    fn failures_from_same_ipv6_prefix_share_attempts() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        let ip1: IpAddr = "2001:db8::1".parse().unwrap();
        let ip2: IpAddr = "2001:db8::2".parse().unwrap();
        let key1 = (canonical_client_ip(ip1), "admin".to_string());
        let key2 = (canonical_client_ip(ip2), "admin".to_string());

        attempts.record_failure(key1.clone(), 2, reset, now);
        assert!(!attempts.is_blocked(&key1, reset, now));
        assert!(!attempts.is_blocked(&key2, reset, now));

        attempts.record_failure(key2.clone(), 2, reset, now);
        assert!(attempts.is_blocked(&key1, reset, now));
        assert!(attempts.is_blocked(&key2, reset, now));
    }

    #[test]
    fn totp_attempts_limit_and_account_blocking() {
        let mut totp = TotpAttempts::default();
        let reset = Duration::from_secs(60);
        let now = Instant::now();

        // 1st and 2nd failed attempt on session_1 do not exhaust it
        assert!(!totp.record_failure("token_1", "admin", 5, reset, now));
        assert!(!totp.record_failure("token_1", "admin", 5, reset, now));
        assert!(!totp.is_account_blocked("admin", reset, now));

        // 3rd failed attempt on session_1 exhausts the session
        assert!(totp.record_failure("token_1", "admin", 5, reset, now));

        // Another session for the same user fails 2 more times (total 5 for user)
        assert!(!totp.record_failure("token_2", "admin", 5, reset, now));
        assert!(!totp.record_failure("token_2", "admin", 5, reset, now));

        // Now user is blocked across all sessions and source IPs
        assert!(totp.is_account_blocked("admin", reset, now));

        // Clear resets both
        totp.clear("token_2", "admin");
        assert!(!totp.is_account_blocked("admin", reset, now));
    }
}
