use super::affinity::Affinity;
use super::auth::{Authenticator, SessionService};
use super::cache::HttpCache;
use super::client_ip::ClientIpResolver;
use super::filter::{FilterResult, MatchRank, RequestFilter};
use super::header_rules::CompiledHeaderRules;
use super::mirror::Mirror;
use super::pool::{ConnectionPool, Upstream, UpstreamClients};
use super::rate_limit::ClientRateLimiter;
use super::rewrite::Rewrite;
use crate::proxy::health::{self, GroupKey, GroupRegistry, Probe};
use crate::proxy::ProxyRegistries;
use hyper::{Request, Uri};
use r3v3rs3_api::redirect::RedirectRule;
use r3v3rs3_api::{
    compression::Compression,
    id::ShortId,
    policy::IpFilter,
    proxy::{HttpProxy, ProxyEntry, ProxyKind, Route, Server},
    upstream::{
        CircuitBreaker, HealthCheck, LoadBalancing, RetryPolicy, StickyCookie, UpstreamTimeouts,
    },
};
use std::{str::FromStr, sync::Arc};
use url::{Host, Url};

#[derive(Default, Debug)]
pub struct Router {
    routes: Vec<FilteredRoute>,
}

impl Router {
    pub fn new(
        proxies: Vec<ProxyEntry>,
        https_port: Option<u16>,
        quic_port: Option<u16>,
        upstream: &mut UpstreamClients<'_>,
        sessions: &Arc<SessionService>,
        registries: &ProxyRegistries,
    ) -> Self {
        let mut routes = vec![];
        for (id, http) in proxies
            .into_iter()
            .filter_map(|entry| match entry.proxy.kind {
                ProxyKind::Http(http) => Some((entry.id, *http)),
                _ => None,
            })
        {
            let (tls_client_config, _) = upstream.get(http.client_cert, http.timeouts.connect);
            let proxy_upstream = ProxyUpstream::new(&http);
            let client_ip = Arc::new(ClientIpResolver::new(&http.client_ip));
            let proxy_ip_filter = Arc::new(http.ip_filter);
            let proxy_rate_limiter = registries.limiters.limiter((id, None), http.rate_limit);
            let proxy_auth = Authenticator::new(http.auth, &tls_client_config, sessions, id);
            let proxy_header_rules = Arc::new(CompiledHeaderRules::new(&http.headers));
            let compression = (!http.compression.is_disabled()).then(|| Arc::new(http.compression));
            let proxy_cache = registries.caches.cache_for(id, &http.cache);
            let redirects: Arc<[RedirectRule]> = http.redirects.clone().into();
            for (index, route) in http.routes.into_iter().enumerate() {
                let filter = RequestFilter::new(&http.vhosts, &route);
                let base_path: String = filter
                    .path
                    .iter()
                    .map(|segment| format!("/{segment}"))
                    .collect();
                let upstream = proxy_upstream.route(
                    (id, Some(index)),
                    &route,
                    &base_path,
                    upstream,
                    &registries.groups,
                );
                let ip_filter = route
                    .ip_filter
                    .map(Arc::new)
                    .unwrap_or_else(|| proxy_ip_filter.clone());
                let rate_limiter = match route.rate_limit {
                    Some(config) => registries.limiters.limiter((id, Some(index)), config),
                    None => proxy_rate_limiter.clone(),
                };
                let auth = route.auth.map_or_else(
                    || proxy_auth.clone(),
                    |policy| Authenticator::new(policy, &tls_client_config, sessions, id),
                );
                let header_rules = route.headers.as_ref().map_or_else(
                    || proxy_header_rules.clone(),
                    |rules| Arc::new(CompiledHeaderRules::new(rules)),
                );
                routes.push(FilteredRoute {
                    resource_id: id,
                    filter,
                    base_path,
                    https_port,
                    quic_port,
                    upgrade_insecure: http.upgrade_insecure,
                    redirects: redirects.clone(),
                    client_ip: client_ip.clone(),
                    ip_filter,
                    rate_limiter,
                    auth,
                    header_rules,
                    compression: compression.clone(),
                    cache: proxy_cache.clone(),
                    h2c: http.h2c,
                    upstream,
                });
            }
        }
        Self { routes }
    }

    /// Returns the most specific route for the request: the most specific host match, then the
    /// longest path. Of routes with the same rank, the first route wins.
    pub fn get_route<T>(
        &self,
        req: &Request<T>,
        host: Option<&str>,
    ) -> Option<(FilterResult, &FilteredRoute)> {
        let mut best: Option<(MatchRank, &FilteredRoute)> = None;
        for route in &self.routes {
            let Some(rank) = route.filter.rank(req, host) else {
                continue;
            };
            if best.is_none_or(|(best_rank, _)| rank > best_rank) {
                best = Some((rank, route));
            }
        }
        best.map(|(_, route)| (route.filter.result(req), route))
    }
}

/// The upstream settings of an HTTP proxy that its routes share.
struct ProxyUpstream {
    client_cert: Option<ShortId>,
    timeouts: UpstreamTimeouts,
    load_balancing: LoadBalancing,
    health_check: HealthCheck,
    circuit_breaker: CircuitBreaker,
    retry: RetryPolicy,
    sticky: StickyCookie,
    max_body_size: u64,
}

impl ProxyUpstream {
    fn new(http: &HttpProxy) -> Self {
        Self {
            client_cert: http.client_cert,
            timeouts: http.timeouts,
            load_balancing: http.load_balancing,
            health_check: http.health_check.clone(),
            circuit_breaker: http.circuit_breaker,
            retry: http.retry.clone(),
            sticky: http.sticky.clone(),
            max_body_size: http.max_body_size,
        }
    }

    /// Builds the upstream servers of a route. `None` when the client certificate of the proxy is
    /// invalid, so the route cannot reach its upstream servers. `base_path` is the route path,
    /// which the sticky cookie and the path rewrite use.
    fn route(
        &self,
        key: GroupKey,
        route: &Route,
        base_path: &str,
        clients: &mut UpstreamClients<'_>,
        groups: &GroupRegistry,
    ) -> Option<Upstream> {
        let timeouts = route.timeouts.unwrap_or(self.timeouts);
        let (_, pool) = clients.get(self.client_cert, timeouts.connect);
        let pool = pool?;
        let members = route
            .servers
            .iter()
            .map(|server| health::GroupServer {
                addr: server.url.to_string(),
                weight: server.weight,
            })
            .collect();
        let probe = http_probe(&route.servers, &self.health_check.path, &pool);
        let mirror = route
            .mirror
            .as_ref()
            .map(|config| Arc::new(Mirror::new(config, pool.clone(), timeouts.request)));
        let group = groups.group(
            key,
            members,
            self.load_balancing,
            self.health_check.clone(),
            self.circuit_breaker,
            probe,
        );
        Some(Upstream {
            pool,
            request_timeout: timeouts.request,
            servers: route.servers.clone().into(),
            group,
            retry: route.retry.clone().unwrap_or_else(|| self.retry.clone()),
            affinity: self
                .sticky
                .enabled
                .then(|| Affinity::new(&self.sticky, key, &route.servers, base_path))
                .flatten()
                .map(Arc::new),
            max_body_size: route.max_body_size.unwrap_or(self.max_body_size),
            rewrite: Arc::new(Rewrite::new(&route.rewrite, base_path)),
            mirror,
        })
    }
}

/// The active health check of the servers of a route: `GET` to the path from the root of each
/// server, or a TCP connection when the path is empty.
fn http_probe(servers: &[Server], path: &str, pool: &Arc<ConnectionPool>) -> Probe {
    if path.is_empty() {
        return Probe::Connect(
            servers
                .iter()
                .map(|server| url_target(&server.url.0))
                .collect(),
        );
    }
    let uris = servers
        .iter()
        .map(|server| {
            let url = server.url.0.join(path).ok()?;
            Uri::from_str(url.as_str()).ok()
        })
        .collect();
    Probe::Http {
        uris,
        pool: pool.clone(),
    }
}

fn url_target(url: &Url) -> Option<(String, u16)> {
    let host = match url.host()? {
        Host::Domain(domain) => domain.to_string(),
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => ip.to_string(),
    };
    Some((host, url.port_or_known_default()?))
}

#[derive(Debug)]
pub struct FilteredRoute {
    pub resource_id: ShortId,
    pub filter: RequestFilter,
    /// Path of the route without a trailing slash, e.g. `/admin`. Empty for `/`.
    pub base_path: String,
    pub https_port: Option<u16>,
    pub quic_port: Option<u16>,
    pub upgrade_insecure: bool,
    /// The redirect rules of the proxy. Every route of the proxy shares them.
    pub redirects: Arc<[RedirectRule]>,
    pub client_ip: Arc<ClientIpResolver>,
    pub ip_filter: Arc<IpFilter>,
    pub rate_limiter: Option<Arc<ClientRateLimiter>>,
    pub auth: Option<Arc<Authenticator>>,
    pub header_rules: Arc<CompiledHeaderRules>,
    /// `None` when the proxy has no compression algorithm.
    pub compression: Option<Arc<Compression>>,
    /// `None` when the proxy cache is disabled. Every route of the proxy shares the cache.
    pub cache: Option<Arc<HttpCache>>,
    /// Sends requests to plain HTTP upstream servers with HTTP/2 prior knowledge.
    pub h2c: bool,
    /// The upstream servers, their connections and the request timeout of the route. `None` when
    /// the client certificate of the proxy is invalid, so the route cannot reach its upstream
    /// servers.
    pub upstream: Option<Upstream>,
}
