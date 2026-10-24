use super::affinity::Affinity;
use super::auth::{Authenticator, SessionService};
use super::cache::HttpCache;
use super::client_ip::ClientIpResolver;
use super::concurrency_limit::ConcurrencyLimiter;
use super::filter::{FilterResult, MatchRank, RequestFilter};
use super::header_rules::CompiledHeaderRules;
use super::mirror::Mirror;
use super::pool::{ConnectionPool, Upstream, UpstreamClients};
use super::rate_limit::ClientRateLimiter;
use super::rewrite::Rewrite;
use crate::proxy::ProxyRegistries;
use crate::proxy::health::{self, GroupKey, GroupRegistry, Probe};
use hyper::{Request, Uri};
use r3v3rs3_api::redirect::RedirectRule;
use r3v3rs3_api::{
    compression::Compression,
    fixed_response::FixedResponse,
    id::ShortId,
    policy::{ConcurrencyLimit, IpFilter, RateLimit},
    proxy::{CustomErrorPage, HttpProxy, ProxyEntry, ProxyKind, Route, Server},
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
        for (id, http, entry_source) in
            proxies
                .into_iter()
                .filter_map(|entry| match entry.proxy.kind {
                    ProxyKind::Http(http) => Some((entry.id, *http, entry.source)),
                    _ => None,
                })
        {
            let (tls_client_config, _) = upstream.get(http.client_cert, http.timeouts.connect);
            let proxy_upstream = ProxyUpstream::new(&http);
            let client_ip = Arc::new(ClientIpResolver::new(&http.client_ip));
            let proxy_ip_filter = Arc::new(http.ip_filter);
            let proxy_rate_limiter = registries.limiters.limiter((id, None), http.rate_limit);
            let proxy_concurrency_limiter = registries
                .concurrency_limiters
                .limiter((id, None), http.concurrency_limit);
            let proxy_auth = Authenticator::new(http.auth, &tls_client_config, sessions, id);
            let proxy_header_rules = Arc::new(CompiledHeaderRules::new(&http.headers));
            let compression = (!http.compression.is_disabled()).then(|| Arc::new(http.compression));
            let proxy_cache = registries.caches.cache_for(id, &http.cache);
            let redirects: Arc<[RedirectRule]> = http.redirects.clone().into();
            let maintenance = Arc::new(http.maintenance);
            let effective_error_pages = resolve_effective_error_pages(
                http.error_page_theme,
                &http.error_pages,
                &registries.error_page_themes.load(),
            );
            let error_pages: Arc<[CustomErrorPage]> = effective_error_pages.into();
            for (index, route) in http.routes.into_iter().enumerate() {
                let filter = RequestFilter::new(&http.vhosts, &route);
                let base_path: String = filter
                    .path
                    .iter()
                    .map(|segment| format!("/{segment}"))
                    .collect();
                let servers = registries.srv.expand(&route.servers);
                let upstream = proxy_upstream.route(
                    (id, Some(index)),
                    &route,
                    &servers,
                    &base_path,
                    upstream,
                    &registries.groups,
                );
                let (rate_limiter, concurrency_limiter) = resolve_route_limiters(
                    registries,
                    (id, Some(index)),
                    route.rate_limit,
                    route.concurrency_limit,
                    &proxy_rate_limiter,
                    &proxy_concurrency_limiter,
                );
                let ip_filter = route
                    .ip_filter
                    .map(Arc::new)
                    .unwrap_or_else(|| proxy_ip_filter.clone());
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
                    metric_key: entry_source.as_ref().map(|source| source.resource.clone()),
                    filter,
                    base_path,
                    https_port,
                    quic_port,
                    upgrade_insecure: http.upgrade_insecure,
                    redirects: redirects.clone(),
                    client_ip: client_ip.clone(),
                    ip_filter,
                    rate_limiter,
                    concurrency_limiter,
                    auth,
                    header_rules,
                    compression: compression.clone(),
                    cache: proxy_cache.clone(),
                    h2c: http.h2c,
                    upstream,
                    response: route.response.map(Arc::new),
                    maintenance: maintenance.clone(),
                    error_pages: error_pages.clone(),
                    require_client_cert: route.require_client_cert,
                    geoip: registries.geoip.clone(),
                    bot_blocker: route.bot_blocker.unwrap_or(http.bot_blocker),
                    grpc_web: route.grpc_web.unwrap_or(http.grpc_web),
                });
            }
        }
        Self { routes }
    }

    /// Returns the custom error pages of the first route matching the host, or empty if none.
    pub fn find_error_pages(&self, host: Option<&str>) -> Arc<[CustomErrorPage]> {
        for route in &self.routes {
            if route.filter.matches_host(host) && !route.error_pages.is_empty() {
                return route.error_pages.clone();
            }
        }
        Arc::from([])
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

fn resolve_route_limiters(
    registries: &ProxyRegistries,
    key: (ShortId, Option<usize>),
    route_rate: Option<RateLimit>,
    route_concurrency: Option<ConcurrencyLimit>,
    proxy_rate: &Option<Arc<ClientRateLimiter>>,
    proxy_concurrency: &Option<Arc<ConcurrencyLimiter>>,
) -> (
    Option<Arc<ClientRateLimiter>>,
    Option<Arc<ConcurrencyLimiter>>,
) {
    let rate_limiter = match route_rate {
        Some(config) => registries.limiters.limiter(key, config),
        None => proxy_rate.clone(),
    };
    let concurrency_limiter = match route_concurrency {
        Some(config) => registries.concurrency_limiters.limiter(key, config),
        None => proxy_concurrency.clone(),
    };
    (rate_limiter, concurrency_limiter)
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
    /// invalid, so the route cannot reach its upstream servers. `servers` are the servers of the
    /// route with each SRV server replaced by its targets. `base_path` is the route path, which
    /// the sticky cookie and the path rewrite use.
    fn route(
        &self,
        key: GroupKey,
        route: &Route,
        servers: &[Server],
        base_path: &str,
        clients: &mut UpstreamClients<'_>,
        groups: &GroupRegistry,
    ) -> Option<Upstream> {
        let timeouts = route.timeouts.unwrap_or(self.timeouts);
        let (_, pool) = clients.get(self.client_cert, timeouts.connect);
        let pool = pool?;
        let members = servers
            .iter()
            .map(|server| health::GroupServer {
                addr: server.url.to_string(),
                weight: server.weight,
            })
            .collect();
        let probe = http_probe(servers, &self.health_check.path, &pool);
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
            servers: servers.into(),
            group,
            retry: route.retry.clone().unwrap_or_else(|| self.retry.clone()),
            affinity: route
                .sticky
                .as_ref()
                .unwrap_or(&self.sticky)
                .enabled
                .then(|| {
                    Affinity::new(
                        route.sticky.as_ref().unwrap_or(&self.sticky),
                        key,
                        servers,
                        base_path,
                    )
                })
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
    /// The resource name of a discovered proxy, for example the app that a platform proxy
    /// serves. The request counters of the collector count under it.
    pub metric_key: Option<String>,
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
    pub concurrency_limiter: Option<Arc<ConcurrencyLimiter>>,
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
    /// The redirect or the status that answers the route instead of an upstream server.
    pub response: Option<Arc<FixedResponse>>,
    /// Maintenance mode settings of the proxy.
    pub maintenance: Arc<r3v3rs3_api::proxy::MaintenanceConfig>,
    /// Custom error pages of the proxy.
    pub error_pages: Arc<[CustomErrorPage]>,
    /// Requires a verified TLS client certificate.
    pub require_client_cert: bool,
    pub geoip: Arc<crate::proxy::geoip::GeoIpResolver>,
    pub bot_blocker: bool,
    pub grpc_web: bool,
}

fn resolve_effective_error_pages(
    theme_id: Option<ShortId>,
    overrides: &[CustomErrorPage],
    themes: &[r3v3rs3_api::error_page_theme::ErrorPageTheme],
) -> Vec<CustomErrorPage> {
    let mut pages = Vec::new();
    if let Some(theme) = theme_id.and_then(|id| themes.iter().find(|t| t.id == id)) {
        pages = theme.pages.clone();
    }
    for override_page in overrides {
        if let Some(existing) = pages.iter_mut().find(|p| p.status == override_page.status) {
            *existing = override_page.clone();
        } else {
            pages.push(override_page.clone());
        }
    }
    pages
}
