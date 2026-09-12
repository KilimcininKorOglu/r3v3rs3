use super::auth::{Authenticator, SessionService};
use super::client_ip::ClientIpResolver;
use super::filter::{FilterResult, RequestFilter};
use super::header_rules::CompiledHeaderRules;
use super::rate_limit::{self, ClientRateLimiter};
use hyper::Request;
use r3v3rs3_api::{
    compression::Compression,
    id::ShortId,
    policy::IpFilter,
    proxy::{ProxyEntry, ProxyKind, Server},
};
use std::sync::Arc;
use tokio_rustls::rustls::ClientConfig;

#[derive(Default, Debug)]
pub struct Router {
    routes: Vec<FilteredRoute>,
}

impl Router {
    pub fn new(
        proxies: Vec<ProxyEntry>,
        https_port: Option<u16>,
        quic_port: Option<u16>,
        tls_client_config: &Arc<ClientConfig>,
        sessions: &Arc<SessionService>,
    ) -> Self {
        let mut routes = vec![];
        for (id, http) in proxies
            .into_iter()
            .filter_map(|entry| match entry.proxy.kind {
                ProxyKind::Http(http) => Some((entry.id, *http)),
                _ => None,
            })
        {
            let client_ip = Arc::new(ClientIpResolver::new(&http.client_ip));
            let proxy_ip_filter = Arc::new(http.ip_filter);
            let proxy_rate_limiter = rate_limit::limiter((id, None), http.rate_limit);
            let proxy_auth = Authenticator::new(http.auth, tls_client_config, sessions);
            let proxy_header_rules = Arc::new(CompiledHeaderRules::new(&http.headers));
            let compression = (!http.compression.is_disabled()).then(|| Arc::new(http.compression));
            for (index, route) in http.routes.into_iter().enumerate() {
                let filter = RequestFilter::new(&http.vhosts, &route);
                let base_path = filter
                    .path
                    .iter()
                    .map(|segment| format!("/{segment}"))
                    .collect();
                let ip_filter = route
                    .ip_filter
                    .map(Arc::new)
                    .unwrap_or_else(|| proxy_ip_filter.clone());
                let rate_limiter = match route.rate_limit {
                    Some(config) => rate_limit::limiter((id, Some(index)), config),
                    None => proxy_rate_limiter.clone(),
                };
                let auth = route.auth.map_or_else(
                    || proxy_auth.clone(),
                    |policy| Authenticator::new(policy, tls_client_config, sessions),
                );
                let header_rules = route.headers.as_ref().map_or_else(
                    || proxy_header_rules.clone(),
                    |rules| Arc::new(CompiledHeaderRules::new(rules)),
                );
                routes.push(FilteredRoute {
                    resource_id: id,
                    filter,
                    base_path,
                    route: ParsedRoute {
                        servers: route.servers,
                    },
                    https_port,
                    quic_port,
                    upgrade_insecure: http.upgrade_insecure,
                    client_ip: client_ip.clone(),
                    ip_filter,
                    rate_limiter,
                    auth,
                    header_rules,
                    compression: compression.clone(),
                });
            }
        }
        Self { routes }
    }

    pub fn get_route<T>(
        &self,
        req: &Request<T>,
        host: Option<&str>,
    ) -> Option<(&ParsedRoute, FilterResult, &FilteredRoute)> {
        self.routes.iter().find_map(|route| {
            route
                .filter
                .test(req, host)
                .map(|res| (&route.route, res, route))
        })
    }
}

#[derive(Debug)]
pub struct FilteredRoute {
    pub resource_id: ShortId,
    pub filter: RequestFilter,
    /// Path of the route without a trailing slash, e.g. `/admin`. Empty for `/`.
    pub base_path: String,
    pub route: ParsedRoute,
    pub https_port: Option<u16>,
    pub quic_port: Option<u16>,
    pub upgrade_insecure: bool,
    pub client_ip: Arc<ClientIpResolver>,
    pub ip_filter: Arc<IpFilter>,
    pub rate_limiter: Option<Arc<ClientRateLimiter>>,
    pub auth: Option<Arc<Authenticator>>,
    pub header_rules: Arc<CompiledHeaderRules>,
    /// `None` when the proxy has no compression algorithm.
    pub compression: Option<Arc<Compression>>,
}

#[derive(Debug)]
pub struct ParsedRoute {
    pub servers: Vec<Server>,
}
