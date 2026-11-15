use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hickory_resolver::TokioResolver;
use hickory_resolver::config::LookupIpStrategy;
use hickory_resolver::system_conf::read_system_conf;
use hyper_util::client::legacy::connect::dns::Name;
use tower_service::Service;

/// Minimum retry interval when DNS resolution fails.
const DNS_LOOKUP_RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// Minimum TTL floor for cached successful DNS records.
const MIN_DNS_TTL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
enum CacheState {
    Resolved(Arc<[IpAddr]>),
    Failed(String),
}

#[derive(Debug, Clone)]
struct CachedEntry {
    state: CacheState,
    expires_at: Instant,
    counter: Arc<AtomicUsize>,
}

impl CachedEntry {
    fn rotate_addrs(&self, port: u16) -> anyhow::Result<Vec<SocketAddr>> {
        match &self.state {
            CacheState::Resolved(ips) => {
                let count = ips.len();
                if count == 0 {
                    return Err(anyhow::anyhow!("no IP address found"));
                }
                let start = self.counter.fetch_add(1, Ordering::Relaxed) % count;
                let addrs = (0..count)
                    .map(|i| SocketAddr::new(ips[(start + i) % count], port))
                    .collect();
                Ok(addrs)
            }
            CacheState::Failed(err) => Err(anyhow::anyhow!("DNS lookup failed: {err}")),
        }
    }
}

struct DnsResolverInner {
    resolver: TokioResolver,
    cache: RwLock<HashMap<String, CachedEntry>>,
}

/// A DNS resolver with TTL caching and round-robin load balancing across resolved IP addresses.
#[derive(Clone)]
pub struct DnsResolver {
    inner: Arc<DnsResolverInner>,
}

impl fmt::Debug for DnsResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DnsResolver").finish_non_exhaustive()
    }
}

impl DnsResolver {
    /// Creates a new `DnsResolver` wrapping the given `TokioResolver`.
    pub fn new(resolver: TokioResolver) -> Self {
        Self {
            inner: Arc::new(DnsResolverInner {
                resolver,
                cache: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Creates a new `DnsResolver` using system DNS configuration.
    pub fn from_system_conf() -> anyhow::Result<Self> {
        let (conf, mut opts) = read_system_conf().unwrap_or_default();
        opts.ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        let resolver = super::tokio_resolver(conf, opts)?;
        Ok(Self::new(resolver))
    }

    /// Looks up an entry from the internal cache if valid.
    fn get_cached(&self, host: &str) -> Option<CachedEntry> {
        let cache = self.inner.cache.read().ok()?;
        let entry = cache.get(host)?;
        if entry.expires_at > Instant::now() {
            Some(entry.clone())
        } else {
            None
        }
    }

    /// Inserts a resolved entry into the cache, preserving any existing round-robin counter.
    fn insert_cache(&self, host: &str, state: CacheState, expires_at: Instant) -> CachedEntry {
        let mut cache = self.inner.cache.write().unwrap_or_else(|e| e.into_inner());
        let counter = cache
            .get(host)
            .map(|e| e.counter.clone())
            .unwrap_or_else(|| Arc::new(AtomicUsize::new(0)));
        let entry = CachedEntry {
            state,
            expires_at,
            counter,
        };
        cache.insert(host.to_string(), entry.clone());
        entry
    }

    /// Resolves `host` via DNS and stores the result in the cache.
    async fn resolve_and_cache(&self, host: &str) -> CachedEntry {
        let now = Instant::now();
        match self.inner.resolver.lookup_ip(host).await {
            Ok(lookup) => {
                let ips: Arc<[IpAddr]> = lookup.iter().collect::<Vec<_>>().into();
                if ips.is_empty() {
                    let err = "no IP addresses returned by DNS".to_string();
                    let expires = now + DNS_LOOKUP_RETRY_INTERVAL;
                    self.insert_cache(host, CacheState::Failed(err), expires)
                } else {
                    let valid_until = lookup.valid_until();
                    let expires = if valid_until > now {
                        valid_until
                    } else {
                        now + MIN_DNS_TTL
                    };
                    self.insert_cache(host, CacheState::Resolved(ips), expires)
                }
            }
            Err(e) => {
                let err = e.to_string();
                let expires = now + DNS_LOOKUP_RETRY_INTERVAL;
                self.insert_cache(host, CacheState::Failed(err), expires)
            }
        }
    }

    /// Resolves `host` and returns the socket addresses with `port`.
    /// The addresses are rotated in round-robin order so the first address
    /// is the next backend to attempt.
    pub async fn resolve_addrs(&self, host: &str, port: u16) -> anyhow::Result<Vec<SocketAddr>> {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Ok(vec![SocketAddr::new(ip, port)]);
        }
        if let Some(entry) = self.get_cached(host) {
            return entry.rotate_addrs(port);
        }
        let entry = self.resolve_and_cache(host).await;
        entry.rotate_addrs(port)
    }

    /// Resolves `host` and returns the next round-robin socket address.
    pub async fn resolve_addr(&self, host: &str, port: u16) -> anyhow::Result<SocketAddr> {
        let addrs = self.resolve_addrs(host, port).await?;
        addrs
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no IP address found for {host}"))
    }

    /// Number of cached hostnames (for testing/diagnostics).
    #[cfg(test)]
    pub fn cached_count(&self) -> usize {
        self.inner.cache.read().map(|c| c.len()).unwrap_or_default()
    }

    /// Inserts a mock resolved entry for deterministic testing.
    #[cfg(test)]
    pub fn insert_cached_for_test(&self, host: &str, ips: Vec<IpAddr>, ttl: Duration) {
        self.insert_cache(host, CacheState::Resolved(ips.into()), Instant::now() + ttl);
    }

    /// Inserts a mock failed entry for deterministic testing.
    #[cfg(test)]
    pub fn insert_failed_for_test(&self, host: &str, err: &str, ttl: Duration) {
        self.insert_cache(
            host,
            CacheState::Failed(err.to_string()),
            Instant::now() + ttl,
        );
    }
}

impl Default for DnsResolver {
    fn default() -> Self {
        Self::from_system_conf().unwrap_or_else(|_| {
            let (conf, opts) = (Default::default(), Default::default());
            let resolver = super::tokio_resolver(conf, opts)
                .unwrap_or_else(|_| panic!("default resolver config must succeed"));
            Self::new(resolver)
        })
    }
}

impl Service<Name> for DnsResolver {
    type Response = std::vec::IntoIter<SocketAddr>;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        let resolver = self.clone();
        Box::pin(async move {
            let addrs = resolver
                .resolve_addrs(name.as_str(), 0)
                .await
                .map_err(|err| Box::new(std::io::Error::other(err)) as Self::Error)?;
            Ok(addrs.into_iter())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_resolve_ip_literals() {
        let resolver = DnsResolver::default();
        let v4 = resolver.resolve_addr("127.0.0.1", 8080).await.unwrap();
        assert_eq!(v4, SocketAddr::from(([127, 0, 0, 1], 8080)));

        let v6 = resolver.resolve_addr("::1", 9090).await.unwrap();
        assert_eq!(v6, SocketAddr::from_str("[::1]:9090").unwrap());
        assert_eq!(resolver.cached_count(), 0);
    }

    #[tokio::test]
    async fn test_round_robin_rotation() {
        let resolver = DnsResolver::default();
        let ips = vec![
            IpAddr::from([10, 0, 0, 1]),
            IpAddr::from([10, 0, 0, 2]),
            IpAddr::from([10, 0, 0, 3]),
        ];
        resolver.insert_cached_for_test("app.internal", ips, Duration::from_secs(60));

        let a1 = resolver.resolve_addr("app.internal", 80).await.unwrap();
        let a2 = resolver.resolve_addr("app.internal", 80).await.unwrap();
        let a3 = resolver.resolve_addr("app.internal", 80).await.unwrap();
        let a4 = resolver.resolve_addr("app.internal", 80).await.unwrap();

        assert_eq!(a1, SocketAddr::from(([10, 0, 0, 1], 80)));
        assert_eq!(a2, SocketAddr::from(([10, 0, 0, 2], 80)));
        assert_eq!(a3, SocketAddr::from(([10, 0, 0, 3], 80)));
        assert_eq!(a4, SocketAddr::from(([10, 0, 0, 1], 80)));
    }

    #[tokio::test]
    async fn test_resolve_addrs_order() {
        let resolver = DnsResolver::default();
        let ips = vec![
            IpAddr::from([192, 168, 1, 10]),
            IpAddr::from([192, 168, 1, 20]),
        ];
        resolver.insert_cached_for_test("web.internal", ips, Duration::from_secs(60));

        let list1 = resolver.resolve_addrs("web.internal", 443).await.unwrap();
        let list2 = resolver.resolve_addrs("web.internal", 443).await.unwrap();

        let s10 = SocketAddr::from(([192, 168, 1, 10], 443));
        let s20 = SocketAddr::from(([192, 168, 1, 20], 443));
        assert_eq!(list1, vec![s10, s20]);
        assert_eq!(list2, vec![s20, s10]);
    }

    #[tokio::test]
    async fn test_ttl_expiration() {
        let resolver = DnsResolver::default();
        let ips = vec![IpAddr::from([172, 16, 0, 1])];
        resolver.insert_cached_for_test("temp.internal", ips, Duration::from_millis(20));

        assert!(resolver.get_cached("temp.internal").is_some());
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(resolver.get_cached("temp.internal").is_none());
    }

    #[tokio::test]
    async fn test_counter_preserved_after_dns_refresh() {
        let resolver = DnsResolver::default();
        let ips1 = vec![IpAddr::from([10, 0, 0, 1]), IpAddr::from([10, 0, 0, 2])];
        resolver.insert_cached_for_test("svc.internal", ips1, Duration::from_secs(60));

        let first = resolver.resolve_addr("svc.internal", 80).await.unwrap();
        assert_eq!(first, SocketAddr::from(([10, 0, 0, 1], 80)));

        // Re-insert simulated refresh (e.g. DNS re-resolve)
        let ips2 = vec![IpAddr::from([10, 0, 0, 1]), IpAddr::from([10, 0, 0, 2])];
        resolver.insert_cached_for_test("svc.internal", ips2, Duration::from_secs(60));

        let second = resolver.resolve_addr("svc.internal", 80).await.unwrap();
        assert_eq!(second, SocketAddr::from(([10, 0, 0, 2], 80)));
    }

    #[tokio::test]
    async fn test_cached_failure() {
        let resolver = DnsResolver::default();
        resolver.insert_failed_for_test("fail.internal", "nxdomain", Duration::from_secs(60));

        let res = resolver.resolve_addr("fail.internal", 80).await;
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("nxdomain"));
    }

    #[tokio::test]
    async fn test_hyper_service_resolver() {
        let mut resolver = DnsResolver::default();
        let ips = vec![IpAddr::from([10, 1, 1, 1]), IpAddr::from([10, 1, 1, 2])];
        resolver.insert_cached_for_test("api.internal", ips, Duration::from_secs(60));

        let name = Name::from_str("api.internal").unwrap();
        let fut = resolver.call(name);
        let addrs: Vec<SocketAddr> = fut.await.unwrap().collect();
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0], SocketAddr::from(([10, 1, 1, 1], 0)));
        assert_eq!(addrs[1], SocketAddr::from(([10, 1, 1, 2], 0)));
    }
}
