use crate::proxy::registry::WeakRegistry;
use governor::{clock::Clock, DefaultKeyedRateLimiter, Quota, RateLimiter};
use r3v3rs3_api::{id::ShortId, policy::RateLimit};
use std::{
    fmt,
    net::IpAddr,
    num::NonZeroU32,
    sync::{Arc, OnceLock},
    time::Duration,
};
use tracing::debug;

const CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

/// Identifies a limiter: the proxy, and the route index when the route overrides the proxy limit.
pub type LimiterKey = (ShortId, Option<usize>);

/// Limits the request rate of each client IP address.
pub struct ClientRateLimiter {
    config: RateLimit,
    limiter: DefaultKeyedRateLimiter<IpAddr>,
}

impl fmt::Debug for ClientRateLimiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientRateLimiter")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ClientRateLimiter {
    /// `None` when the limit is disabled.
    fn quota(config: RateLimit) -> Option<Quota> {
        Some(
            Quota::with_period(config.replenish_interval()?)?
                .allow_burst(NonZeroU32::new(config.burst_size())?),
        )
    }

    /// Records a request of the client. Returns the time to wait when the client is over the limit.
    pub fn check(&self, ip: IpAddr) -> Result<(), Duration> {
        self.limiter
            .check_key(&ip.to_canonical())
            .map_err(|not_until| not_until.wait_time_from(self.limiter.clock().now()))
    }
}

type Limiters = WeakRegistry<LimiterKey, ClientRateLimiter>;

/// The rate limiters of one server.
#[derive(Debug, Default)]
pub struct LimiterRegistry {
    limiters: Arc<Limiters>,
    cleanup_task: OnceLock<()>,
}

impl LimiterRegistry {
    /// Returns the limiter for `key`, or `None` when the limit is disabled. The existing limiter
    /// is reused while its configuration is unchanged, so a configuration reload keeps the client
    /// counters.
    pub fn limiter(&self, key: LimiterKey, config: RateLimit) -> Option<Arc<ClientRateLimiter>> {
        let quota = ClientRateLimiter::quota(config)?;
        self.start_cleanup_task();
        Some(self.limiters.get_or_create(
            key,
            |existing| existing.config == config,
            || {
                Arc::new(ClientRateLimiter {
                    config,
                    limiter: RateLimiter::keyed(quota),
                })
            },
        ))
    }

    /// Starts the cleanup task once. The task stops when the registry is dropped.
    fn start_cleanup_task(&self) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        self.cleanup_task.get_or_init(|| {
            let limiters = Arc::downgrade(&self.limiters);
            handle.spawn(async move {
                let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
                loop {
                    interval.tick().await;
                    let Some(limiters) = limiters.upgrade() else {
                        break;
                    };
                    retain_recent(&limiters);
                }
            });
        });
    }
}

/// Drops the counters of clients that are back under their limit.
fn retain_recent(limiters: &Limiters) {
    for (_, limiter) in limiters.live() {
        limiter.limiter.retain_recent();
        limiter.limiter.shrink_to_fit();
        debug!(clients = limiter.limiter.len(), "rate limiter cleanup");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use r3v3rs3_api::policy::RatePeriod;

    fn per_minute(requests: u32) -> RateLimit {
        RateLimit {
            requests,
            per: RatePeriod::Minute,
            burst: 0,
        }
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().unwrap()
    }

    #[test]
    fn disabled_limit_has_no_limiter() {
        let registry = LimiterRegistry::default();
        assert!(registry
            .limiter(("rldis".parse().unwrap(), None), RateLimit::default())
            .is_none());
    }

    #[test]
    fn limiter_rejects_clients_over_the_burst() {
        let registry = LimiterRegistry::default();
        let limiter = registry
            .limiter(("rlburst".parse().unwrap(), None), per_minute(2))
            .unwrap();
        assert!(limiter.check(ip("203.0.113.1")).is_ok());
        assert!(limiter.check(ip("203.0.113.1")).is_ok());
        let wait = limiter.check(ip("203.0.113.1")).unwrap_err();
        assert!(wait > Duration::ZERO && wait <= Duration::from_secs(30));

        assert!(limiter.check(ip("203.0.113.2")).is_ok());
        assert!(limiter.check(ip("::ffff:203.0.113.2")).is_ok());
        assert!(limiter.check(ip("203.0.113.2")).is_err());
    }

    #[test]
    fn unchanged_config_reuses_the_limiter() {
        let registry = LimiterRegistry::default();
        let key = ("rlreuse".parse().unwrap(), Some(0));
        let first = registry.limiter(key, per_minute(5)).unwrap();
        let second = registry.limiter(key, per_minute(5)).unwrap();
        assert!(Arc::ptr_eq(&first, &second));

        let changed = registry.limiter(key, per_minute(6)).unwrap();
        assert!(!Arc::ptr_eq(&first, &changed));
    }

    #[test]
    fn two_servers_do_not_share_limiters() {
        let key = ("rlshare".parse().unwrap(), None);
        let (first, second) = (LimiterRegistry::default(), LimiterRegistry::default());
        let first = first.limiter(key, per_minute(1)).unwrap();
        let second = second.limiter(key, per_minute(1)).unwrap();
        assert!(first.check(ip("203.0.113.9")).is_ok());
        assert!(first.check(ip("203.0.113.9")).is_err());
        assert!(second.check(ip("203.0.113.9")).is_ok());
    }
}
