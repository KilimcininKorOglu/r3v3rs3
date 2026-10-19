use crate::proxy::http::error::ProxyError;
use crate::proxy::http::rate_limit::LimiterKey;
use crate::proxy::registry::WeakRegistry;
use dashmap::DashMap;
use hyper::HeaderMap;
use hyper::body::{Body, Frame, SizeHint};
use r3v3rs3_api::policy::ConcurrencyLimit;
use std::fmt;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

/// Limits concurrent in-flight requests per client IP or request header (e.g. API key).
pub struct ConcurrencyLimiter {
    config: ConcurrencyLimit,
    active: Arc<DashMap<String, u32>>,
}

impl fmt::Debug for ConcurrencyLimiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConcurrencyLimiter")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl ConcurrencyLimiter {
    pub fn new(config: ConcurrencyLimit) -> Self {
        Self {
            config,
            active: Arc::new(DashMap::new()),
        }
    }

    /// Derives the client identity key: either from the configured header or client IP.
    fn key_for(&self, headers: &HeaderMap, client_ip: IpAddr) -> String {
        let header_key = self.config.header.as_deref().and_then(|name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|s| !s.is_empty())
        });
        if let Some(val) = header_key {
            format!("hdr:{val}")
        } else {
            format!("ip:{}", client_ip.to_canonical())
        }
    }

    /// Attempts to acquire an in-flight permit. Returns an error if the limit is exceeded.
    pub fn acquire(
        &self,
        headers: &HeaderMap,
        client_ip: IpAddr,
    ) -> Result<ConcurrencyGuard, ProxyError> {
        let key = self.key_for(headers, client_ip);
        let mut entry = self.active.entry(key.clone()).or_insert(0);
        if *entry >= self.config.max {
            return Err(ProxyError::TooManyConcurrentRequests);
        }
        *entry += 1;
        drop(entry);
        Ok(ConcurrencyGuard {
            active: self.active.clone(),
            key,
        })
    }
}

/// RAII guard holding an active concurrency permit.
#[derive(Debug)]
pub struct ConcurrencyGuard {
    active: Arc<DashMap<String, u32>>,
    key: String,
}

impl Drop for ConcurrencyGuard {
    fn drop(&mut self) {
        if let Some(mut entry) = self.active.get_mut(&self.key) {
            if *entry <= 1 {
                drop(entry);
                self.active.remove(&self.key);
            } else {
                *entry -= 1;
            }
        }
    }
}

type ConcurrencyLimiters = WeakRegistry<LimiterKey, ConcurrencyLimiter>;

/// Registry managing concurrency limiters across proxies and routes.
#[derive(Debug, Default)]
pub struct ConcurrencyRegistry {
    limiters: Arc<ConcurrencyLimiters>,
}

impl ConcurrencyRegistry {
    pub fn limiter(
        &self,
        key: LimiterKey,
        config: ConcurrencyLimit,
    ) -> Option<Arc<ConcurrencyLimiter>> {
        if config.is_disabled() {
            return None;
        }
        let new_config = config.clone();
        Some(self.limiters.get_or_create(
            key,
            |existing| existing.config == config,
            || Arc::new(ConcurrencyLimiter::new(new_config)),
        ))
    }
}

pin_project_lite::pin_project! {
    /// Wraps a response body and releases the concurrency permit when the body is dropped.
    pub struct GuardedBody<B> {
        #[pin]
        inner: B,
        _guard: Option<ConcurrencyGuard>,
    }
}

impl<B> GuardedBody<B> {
    pub fn new(inner: B, guard: Option<ConcurrencyGuard>) -> Self {
        Self {
            inner,
            _guard: guard,
        }
    }
}

impl<B: Body> Body for GuardedBody<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.project().inner.poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    #[test]
    fn concurrency_limiter_acquires_and_releases() {
        let config = ConcurrencyLimit {
            max: 2,
            header: None,
        };
        let limiter = ConcurrencyLimiter::new(config);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        let headers = HeaderMap::new();

        let guard1 = limiter.acquire(&headers, ip).unwrap();
        assert_eq!(*limiter.active.get("ip:127.0.0.1").unwrap(), 1);

        let guard2 = limiter.acquire(&headers, ip).unwrap();
        assert_eq!(*limiter.active.get("ip:127.0.0.1").unwrap(), 2);

        // 3rd concurrent request fails
        assert!(limiter.acquire(&headers, ip).is_err());

        // Different IP succeeds
        let ip2: IpAddr = "10.0.0.1".parse().unwrap();
        let guard_other = limiter.acquire(&headers, ip2).unwrap();
        assert_eq!(*limiter.active.get("ip:10.0.0.1").unwrap(), 1);

        // Dropping guard1 frees up slot
        drop(guard1);
        assert_eq!(*limiter.active.get("ip:127.0.0.1").unwrap(), 1);

        let guard3 = limiter.acquire(&headers, ip).unwrap();
        assert_eq!(*limiter.active.get("ip:127.0.0.1").unwrap(), 2);

        drop(guard2);
        drop(guard3);
        assert!(limiter.active.get("ip:127.0.0.1").is_none());

        drop(guard_other);
        assert!(limiter.active.is_empty());
    }

    #[test]
    fn concurrency_limiter_by_header() {
        let config = ConcurrencyLimit {
            max: 1,
            header: Some("X-API-Key".into()),
        };
        let limiter = ConcurrencyLimiter::new(config);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();

        let mut headers_alice = HeaderMap::new();
        headers_alice.insert("x-api-key", HeaderValue::from_static("key_alice"));

        let mut headers_bob = HeaderMap::new();
        headers_bob.insert("x-api-key", HeaderValue::from_static("key_bob"));

        let guard_alice = limiter.acquire(&headers_alice, ip).unwrap();
        assert_eq!(*limiter.active.get("hdr:key_alice").unwrap(), 1);

        // Alice 2nd request fails
        assert!(limiter.acquire(&headers_alice, ip).is_err());

        // Bob succeeds
        let guard_bob = limiter.acquire(&headers_bob, ip).unwrap();
        assert_eq!(*limiter.active.get("hdr:key_bob").unwrap(), 1);

        drop(guard_alice);
        assert!(limiter.active.get("hdr:key_alice").is_none());

        // Alice can acquire again
        let guard_alice2 = limiter.acquire(&headers_alice, ip).unwrap();
        drop(guard_alice2);
        drop(guard_bob);
        assert!(limiter.active.is_empty());
    }
}
