//! IP brute-force ban tracker: fail2ban-like layer for login attempts.
//! Bans an IP address when repeated failures occur within a reset window,
//! independent of the attempted username.

use super::openapi::{ErrorResponses, NotFoundResponse};
use super::{AppError, AppState};
use crate::accounts::{Caller, Permission};
use crate::audit::AuditRecord;
use crate::clock::unix_ms;
use axum::{
    Extension, Json,
    extract::{Path, State},
};
use r3v3rs3_api::{audit::AuditAction, auth::BannedIpEntry, error::Error};
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct IpAttempt {
    failures: u32,
    window_start: Instant,
    banned_until: Option<Instant>,
    banned_until_unix_ms: u64,
}

#[derive(Default)]
pub struct IpBanTracker {
    entries: HashMap<IpAddr, IpAttempt>,
}

impl IpBanTracker {
    pub fn check_banned(&mut self, ip: &IpAddr, now: Instant) -> Result<(), Error> {
        self.cleanup(now);
        if let Some(attempt) = self.entries.get(ip)
            && let Some(until) = attempt.banned_until
            && until > now
        {
            let remaining = until.duration_since(now).as_secs();
            return Err(Error::IpBanned {
                seconds: remaining.max(1),
            });
        }
        Ok(())
    }

    pub fn record_failure(
        &mut self,
        ip: IpAddr,
        max_attempts: u32,
        reset: Duration,
        ban_duration: Duration,
        now: Instant,
    ) {
        let attempt = self.entries.entry(ip).or_insert_with(|| IpAttempt {
            failures: 0,
            window_start: now,
            banned_until: None,
            banned_until_unix_ms: 0,
        });

        if now.duration_since(attempt.window_start) >= reset && attempt.banned_until.is_none() {
            attempt.failures = 0;
            attempt.window_start = now;
        }

        attempt.failures += 1;
        if attempt.failures >= max_attempts.max(1) {
            attempt.banned_until = Some(now + ban_duration);
            let millis = u64::try_from(ban_duration.as_millis()).unwrap_or(u64::MAX);
            attempt.banned_until_unix_ms = unix_ms().saturating_add(millis);
        }
    }

    pub fn clear(&mut self, ip: &IpAddr) {
        self.entries.remove(ip);
    }

    pub fn unban(&mut self, ip: &IpAddr) -> bool {
        self.entries.remove(ip).is_some()
    }

    pub fn banned_ips(&mut self, now: Instant) -> Vec<BannedIpEntry> {
        self.cleanup(now);
        let mut list = Vec::new();
        for (ip, attempt) in &self.entries {
            if let Some(until) = attempt.banned_until
                && until > now
            {
                let remaining = until.duration_since(now).as_secs().max(1);
                list.push(BannedIpEntry {
                    ip: *ip,
                    failures: attempt.failures,
                    banned_until: attempt.banned_until_unix_ms,
                    remaining_seconds: remaining,
                });
            }
        }
        list.sort_by_key(|e| e.banned_until);
        list
    }

    fn cleanup(&mut self, now: Instant) {
        self.entries
            .retain(|_, attempt| match attempt.banned_until {
                Some(until) => until > now,
                None => now.duration_since(attempt.window_start) < Duration::from_secs(3600),
            });
    }
}

/// Lists the currently banned client IP addresses.
#[utoipa::path(
    get,
    path = "/",
    tag = "auth",
    operation_id = "list_banned_ips",
    responses((status = 200, description = "The banned IP addresses.", body = Vec<BannedIpEntry>), ErrorResponses)
)]
pub async fn list_banned_ips(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<BannedIpEntry>>, AppError> {
    caller.authorize(Permission::Admin)?;
    let mut data = state.data.lock().await;
    let list = data.ban_tracker.banned_ips(Instant::now());
    Ok(Json(list))
}

/// Unbans a client IP address.
#[utoipa::path(
    delete,
    path = "/{ip}",
    tag = "auth",
    operation_id = "unban_ip",
    params(("ip" = String, Path, description = "IP address to unban.")),
    responses((status = 200, description = "The IP address was unbanned."), NotFoundResponse, ErrorResponses)
)]
pub async fn unban_ip(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(ip_str): Path<String>,
) -> Result<Json<()>, AppError> {
    caller.authorize(Permission::Admin)?;
    let ip: IpAddr = ip_str
        .parse()
        .map_err(|_| Error::InvalidCidr { cidr: ip_str })?;
    let mut data = state.data.lock().await;
    let was_banned = data.ban_tracker.unban(&ip);
    drop(data);
    if was_banned {
        let record = AuditRecord::new(AuditAction::UnbanIp).id(ip.to_string());
        state
            .record_audit(&caller.username, caller.client, record)
            .await;
    }
    Ok(Json(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn ban_tracker_bans_ip_across_different_usernames() {
        let mut tracker = IpBanTracker::default();
        let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
        let now = Instant::now();
        let reset = Duration::from_secs(60);
        let ban_dur = Duration::from_secs(300);

        assert!(tracker.check_banned(&ip, now).is_ok());
        tracker.record_failure(ip, 2, reset, ban_dur, now);
        assert!(tracker.check_banned(&ip, now).is_ok());
        tracker.record_failure(ip, 2, reset, ban_dur, now);
        assert!(tracker.check_banned(&ip, now).is_err());

        assert_eq!(tracker.banned_ips(now).len(), 1);
        assert!(tracker.unban(&ip));
        assert!(tracker.check_banned(&ip, now).is_ok());
    }
}
