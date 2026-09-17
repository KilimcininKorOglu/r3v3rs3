use crate::cache::CacheConfig;
use crate::client_ip::ClientIpConfig;
use crate::compression::Compression;
use crate::discovery::DiscoverySource;
use crate::error::Error;
use crate::fixed_response::FixedResponse;
use crate::header_rules::HeaderRules;
use crate::mirror::Mirror;
use crate::policy::{AuthPolicy, IpFilter, RateLimit};
use crate::proxy_protocol::ProxyProtocolVersion;
use crate::redirect::RedirectRule;
use crate::rewrite::PathRewrite;
use crate::upstream::{
    CircuitBreaker, DEFAULT_WEIGHT, HealthCheck, LoadBalancing, RetryPolicy, SrvStatus,
    StickyCookie, UpstreamHealth, UpstreamTimeouts, default_connect_timeout,
    default_session_idle_timeout, default_weight, is_default_connect_timeout,
    is_default_session_idle_timeout, is_default_weight, validate_timeout, validate_weights,
};
use crate::vhost::VirtualHost;
use crate::{id::ShortId, port::UpstreamServer};
use serde_default::DefaultFromSerde;
use serde_derive::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::time::Duration;
use url::Url;
use utoipa::ToSchema;

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Proxy {
    #[serde(default = "default_active", skip_serializing_if = "is_true")]
    pub active: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default)]
    #[schema(example = json!(["c56yqmqcvpmp49n14s2lexxl"]))]
    pub ports: Vec<ShortId>,
    #[serde(flatten, default = "default_kind")]
    #[schema(inline)]
    pub kind: ProxyKind,
}

fn default_active() -> bool {
    true
}

fn default_kind() -> ProxyKind {
    ProxyKind::Http(Box::default())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum ProxyKind {
    Tcp(TcpProxy),
    /// Boxed, because the HTTP policies make this variant much larger than the others.
    Http(Box<HttpProxy>),
    Udp(UdpProxy),
}

impl ProxyKind {
    /// Returns the client certificate that the proxy sends to the upstream servers.
    pub fn client_cert(&self) -> Option<ShortId> {
        match self {
            Self::Tcp(tcp) => tcp.client_cert,
            Self::Http(http) => http.client_cert,
            Self::Udp(_) => None,
        }
    }

    /// The access lists that an HTTP proxy and its routes use.
    pub fn access_lists(&self) -> Vec<ShortId> {
        let Self::Http(http) = self else {
            return Vec::new();
        };
        let routes = http.routes.iter().filter_map(|route| route.access_list);
        http.access_list.into_iter().chain(routes).collect()
    }

    /// Rejects a zero connect timeout, session idle timeout, fail timeout or check timeout, an
    /// invalid health check path, a server list in which every server has weight 0, an invalid
    /// circuit breaker, invalid retry attempts, an invalid sticky cookie, an invalid path prefix,
    /// an invalid redirect target, an invalid route target and an access list next to the IP
    /// filter or the authentication that it replaces.
    pub fn validate_upstream(&self) -> Result<(), Error> {
        match self {
            Self::Tcp(tcp) => {
                validate_timeout(tcp.connect_timeout)?;
                validate_weights(tcp.upstream_servers.iter().map(|server| server.weight))?;
                tcp.circuit_breaker.validate()?;
                tcp.health_check.validate(false)
            }
            Self::Udp(udp) => {
                validate_timeout(udp.session_idle_timeout)?;
                validate_weights(udp.upstream_servers.iter().map(|server| server.weight))?;
                udp.health_check.validate(false)
            }
            Self::Http(http) => {
                validate_access_lists(http)?;
                http.health_check.validate(true)?;
                http.circuit_breaker.validate()?;
                http.sticky.validate()?;
                http.redirects.iter().try_for_each(RedirectRule::validate)?;
                http.routes.iter().try_for_each(|route| {
                    validate_route_target(route)?;
                    validate_weights(route.servers.iter().map(|server| server.weight))?;
                    route.mirror.iter().try_for_each(Mirror::validate)?;
                    route.rewrite.validate()
                })?;
                http.routes
                    .iter()
                    .filter_map(|route| route.retry.as_ref())
                    .chain([&http.retry])
                    .try_for_each(RetryPolicy::validate)?;
                http.routes
                    .iter()
                    .filter_map(|route| route.timeouts.as_ref())
                    .chain([&http.timeouts])
                    .try_for_each(UpstreamTimeouts::validate)
            }
        }
    }
}

/// Rejects an access list next to the IP filter or the authentication of the same proxy or route.
fn validate_access_lists(http: &HttpProxy) -> Result<(), Error> {
    let proxy_conflict = !http.ip_filter.is_empty() || !http.auth.is_none();
    let route_conflict = http.routes.iter().any(|route| {
        route.access_list.is_some() && (route.ip_filter.is_some() || route.auth.is_some())
    });
    if (http.access_list.is_some() && proxy_conflict) || route_conflict {
        return Err(Error::AccessListConflict);
    }
    Ok(())
}

/// Rejects a route with both servers and a fixed response, and an invalid fixed response. A route
/// without servers stays valid, so a discovered backend without endpoints keeps its route.
fn validate_route_target(route: &Route) -> Result<(), Error> {
    match &route.response {
        Some(_) if !route.servers.is_empty() => Err(Error::RouteResponseConflict),
        Some(response) => response.validate(),
        None => Ok(()),
    }
}

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TcpProxy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_servers: Vec<UpstreamServer>,
    /// Client certificate that r3v3rs3 sends to the TLS upstream servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub client_cert: Option<ShortId>,
    /// Time limit for the DNS lookup, the TCP connection and the TLS handshake of an upstream
    /// connection.
    #[serde(
        default = "default_connect_timeout",
        with = "humantime_serde",
        skip_serializing_if = "is_default_connect_timeout"
    )]
    #[schema(value_type = String, example = "10s")]
    pub connect_timeout: Duration,
    #[serde(default, skip_serializing_if = "LoadBalancing::is_default")]
    pub load_balancing: LoadBalancing,
    #[serde(default, skip_serializing_if = "HealthCheck::is_default")]
    pub health_check: HealthCheck,
    #[serde(default, skip_serializing_if = "CircuitBreaker::is_default")]
    pub circuit_breaker: CircuitBreaker,
    /// Sends the client address in a PROXY protocol header before the data of each upstream
    /// connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_protocol: Option<ProxyProtocolVersion>,
}

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UdpProxy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstream_servers: Vec<UpstreamServer>,
    /// A client session closes when no packet passes in either direction for this time.
    #[serde(
        default = "default_session_idle_timeout",
        with = "humantime_serde",
        skip_serializing_if = "is_default_session_idle_timeout"
    )]
    #[schema(value_type = String, example = "60s")]
    pub session_idle_timeout: Duration,
    #[serde(default, skip_serializing_if = "LoadBalancing::is_default")]
    pub load_balancing: LoadBalancing,
    #[serde(default, skip_serializing_if = "HealthCheck::is_default")]
    pub health_check: HealthCheck,
}

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HttpProxy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = [String], example = json!(["example.com"]))]
    pub vhosts: Vec<VirtualHost>,
    pub routes: Vec<Route>,
    #[serde(default = "upgrade_insecure_default", skip_serializing_if = "is_true")]
    pub upgrade_insecure: bool,
    #[serde(default, skip_serializing_if = "ClientIpConfig::is_default")]
    pub client_ip: ClientIpConfig,
    /// Default client IP filter for every route of this proxy.
    #[serde(default, skip_serializing_if = "IpFilter::is_empty")]
    pub ip_filter: IpFilter,
    /// Default request rate limit of each client for every route of this proxy.
    #[serde(default, skip_serializing_if = "RateLimit::is_disabled")]
    pub rate_limit: RateLimit,
    /// Default authentication for every route of this proxy.
    #[serde(default, skip_serializing_if = "AuthPolicy::is_none")]
    pub auth: AuthPolicy,
    /// The access list whose IP filter and authentication replace `ip_filter` and `auth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub access_list: Option<ShortId>,
    /// Default header rules for every route of this proxy.
    #[serde(default, skip_serializing_if = "HeaderRules::is_empty")]
    pub headers: HeaderRules,
    /// Response compression for every route of this proxy.
    #[serde(default, skip_serializing_if = "Compression::is_disabled")]
    pub compression: Compression,
    /// In-memory response cache shared by every route of this proxy.
    #[serde(default, skip_serializing_if = "CacheConfig::is_disabled")]
    pub cache: CacheConfig,
    /// Sends HTTP/2 with prior knowledge (h2c) to the plain HTTP upstream servers of this proxy.
    #[serde(default, skip_serializing_if = "is_false")]
    pub h2c: bool,
    /// Client certificate that r3v3rs3 sends to the HTTPS upstream servers and the forward auth
    /// service of this proxy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub client_cert: Option<ShortId>,
    /// Default upstream timeouts for every route of this proxy.
    #[serde(default, skip_serializing_if = "UpstreamTimeouts::is_default")]
    pub timeouts: UpstreamTimeouts,
    /// Selects the upstream server of each request in every route of this proxy.
    #[serde(default, skip_serializing_if = "LoadBalancing::is_default")]
    pub load_balancing: LoadBalancing,
    /// Health check of the upstream servers in every route of this proxy.
    #[serde(default, skip_serializing_if = "HealthCheck::is_default")]
    pub health_check: HealthCheck,
    /// Circuit breaker of the upstream servers in every route of this proxy.
    #[serde(default, skip_serializing_if = "CircuitBreaker::is_default")]
    pub circuit_breaker: CircuitBreaker,
    /// Default retry policy for every route of this proxy.
    #[serde(default, skip_serializing_if = "RetryPolicy::is_default")]
    pub retry: RetryPolicy,
    /// Sticky sessions for every route of this proxy.
    #[serde(default, skip_serializing_if = "StickyCookie::is_default")]
    pub sticky: StickyCookie,
    /// The largest request body in bytes for every route of this proxy. `0` has no limit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_body_size: u64,
    /// Redirects that answer the matching requests before authentication.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redirects: Vec<RedirectRule>,
}

impl HttpProxy {
    /// The DNS SRV names of the servers of every route, sorted and without duplicates.
    pub fn srv_names(&self) -> Vec<String> {
        let names = self
            .routes
            .iter()
            .flat_map(|route| &route.servers)
            .filter_map(|server| server.url.srv_name())
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        names.into_iter().collect()
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn upgrade_insecure_default() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProxyState {
    Active,
    Inactive,
    #[default]
    Unknown,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProxyStatus {
    pub state: ProxyState,
    /// The health of the upstream servers. An HTTP proxy lists the servers of each route in route
    /// order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub upstreams: Vec<UpstreamHealth>,
    /// The DNS SRV lookups of the `http+srv` and `https+srv` servers of an HTTP proxy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub srv: Vec<SrvStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProxyEntry {
    pub id: ShortId,
    #[schema(inline)]
    #[serde(flatten)]
    pub proxy: Proxy,
    /// The service discovery provider that created the proxy. A proxy without a source was added
    /// manually. A discovered proxy is read-only and is not saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DiscoverySource>,
}

impl ProxyEntry {
    pub fn is_discovered(&self) -> bool {
        self.source.is_some()
    }
}

impl From<(ShortId, Proxy)> for ProxyEntry {
    fn from((id, proxy): (ShortId, Proxy)) -> Self {
        Self {
            id,
            proxy,
            source: None,
        }
    }
}

impl From<ProxyEntry> for (ShortId, Proxy) {
    fn from(entry: ProxyEntry) -> Self {
        (entry.id, entry.proxy)
    }
}

#[derive(Debug, DefaultFromSerde, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Route {
    #[schema(example = "/")]
    #[serde(default = "default_route_path")]
    pub path: String,
    #[serde(default)]
    pub servers: Vec<Server>,
    /// Replaces the proxy client IP filter for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip_filter: Option<IpFilter>,
    /// Replaces the proxy rate limit for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
    /// Replaces the proxy authentication for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthPolicy>,
    /// The access list whose IP filter and authentication replace those of the proxy for this
    /// route. A route with an access list has no `ip_filter` and no `auth`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<String>)]
    pub access_list: Option<ShortId>,
    /// Replaces the proxy header rules for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeaderRules>,
    /// Replaces the proxy upstream timeouts for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeouts: Option<UpstreamTimeouts>,
    /// Replaces the proxy retry policy for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
    /// Replaces the proxy request body limit for this route. `0` has no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_body_size: Option<u64>,
    /// Changes the request path before the request goes to an upstream server.
    #[serde(default, skip_serializing_if = "PathRewrite::is_default")]
    pub rewrite: PathRewrite,
    /// Sends a copy of the requests of this route to other servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<Mirror>,
    /// Answers every request of this route with a redirect or a status, instead of sending it to
    /// `servers`. A route with a fixed response has no servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<FixedResponse>,
}

fn default_route_path() -> String {
    "/".to_owned()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Server {
    #[schema(value_type = String, example = "https://example.com/api")]
    pub url: ServerUrl,
    /// Share of the traffic compared with the other servers. `0` sends no new traffic to the
    /// server.
    #[serde(default = "default_weight", skip_serializing_if = "is_default_weight")]
    #[schema(example = 1)]
    pub weight: u16,
}

impl Server {
    pub fn new(url: ServerUrl) -> Self {
        Self {
            url,
            weight: DEFAULT_WEIGHT,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct ServerUrl(pub Url);

/// The URL schemes whose host is a DNS SRV name. The scheme before `+srv` is the scheme of the
/// resolved targets.
const SRV_SCHEMES: [&str; 2] = ["http+srv", "https+srv"];

impl ServerUrl {
    pub fn hostname(&self) -> Option<&str> {
        self.0.host_str()
    }

    pub fn authority(&self) -> Option<String> {
        Some(format!(
            "{}:{}",
            self.hostname()?,
            self.0.port_or_known_default().unwrap_or_default()
        ))
    }

    /// The DNS SRV name of an `http+srv` or `https+srv` URL. `None` for another scheme.
    pub fn srv_name(&self) -> Option<&str> {
        SRV_SCHEMES
            .contains(&self.0.scheme())
            .then(|| self.0.host_str())
            .flatten()
    }

    /// The URL of one target of the SRV name: the scheme before `+srv`, the target `host:port`,
    /// and the path and query of this URL.
    pub fn with_srv_target(&self, host: &str, port: u16) -> Option<ServerUrl> {
        let scheme = self.0.scheme().strip_suffix("+srv")?;
        let host = if host.contains(':') {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        let query = self.0.query().map(|q| format!("?{q}")).unwrap_or_default();
        let url = format!("{scheme}://{host}:{port}{}{query}", self.0.path());
        Url::parse(&url).ok().map(ServerUrl)
    }

    /// An SRV URL has a name and no port, because the SRV records give the ports.
    fn is_valid_srv(&self) -> bool {
        self.0.host_str().is_some_and(|host| !host.is_empty()) && self.0.port().is_none()
    }
}

impl<'de> serde::Deserialize<'de> for ServerUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let url = String::deserialize(deserializer)?;
        ServerUrl::from_str(&url).map_err(serde::de::Error::custom)
    }
}

impl From<ServerUrl> for Url {
    fn from(url: ServerUrl) -> Self {
        url.0
    }
}

impl FromStr for ServerUrl {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Url::from_str(s)
            .ok()
            .map(ServerUrl)
            .filter(|url| match url.0.scheme().strip_suffix("+srv") {
                Some(_) => url.srv_name().is_some() && url.is_valid_srv(),
                None => url.authority().is_some(),
            })
            .ok_or_else(|| Error::InvalidServerUrl { url: s.into() })
    }
}

impl TryFrom<Url> for ServerUrl {
    type Error = url::ParseError;

    fn try_from(url: Url) -> Result<Self, Self::Error> {
        Ok(ServerUrl(url))
    }
}

impl fmt::Display for ServerUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_url_accepts_the_srv_schemes_without_a_port() {
        let url = ServerUrl::from_str("http+srv://_http._tcp.api.example.com/api").unwrap();
        assert_eq!(url.srv_name(), Some("_http._tcp.api.example.com"));
        let secure = ServerUrl::from_str("https+srv://_https._tcp.api.example.com/").unwrap();
        assert_eq!(secure.srv_name(), Some("_https._tcp.api.example.com"));
        assert_eq!(
            ServerUrl::from_str("http://api.example.com/")
                .unwrap()
                .srv_name(),
            None
        );

        for invalid in [
            "dns+srv://_http._tcp.api.example.com/",
            "http+srv://_http._tcp.api.example.com:8080/",
            "http+srv:///api",
        ] {
            assert!(
                matches!(
                    ServerUrl::from_str(invalid),
                    Err(Error::InvalidServerUrl { .. })
                ),
                "{invalid} was accepted"
            );
        }
    }

    #[test]
    fn an_srv_url_builds_the_url_of_each_target() {
        let url = ServerUrl::from_str("https+srv://_https._tcp.api.example.com/api?v=1").unwrap();
        let target = url.with_srv_target("10.0.0.1", 8443).unwrap();
        assert_eq!(target.to_string(), "https://10.0.0.1:8443/api?v=1");
        let ipv6 = url.with_srv_target("::1", 8443).unwrap();
        assert_eq!(ipv6.to_string(), "https://[::1]:8443/api?v=1");
        let named = url.with_srv_target("api-1.internal", 8443).unwrap();
        assert_eq!(named.authority(), Some("api-1.internal:8443".into()));
        assert!(
            ServerUrl::from_str("http://a/")
                .unwrap()
                .with_srv_target("b", 80)
                .is_none()
        );
    }

    #[test]
    fn srv_names_are_sorted_and_unique() {
        let route = |url: &str| Route {
            servers: vec![Server::new(url.parse().unwrap())],
            ..Default::default()
        };
        let http = HttpProxy {
            routes: vec![
                route("http+srv://_b._tcp.example.com/"),
                route("http://static.example.com/"),
                route("http+srv://_a._tcp.example.com/"),
                route("http+srv://_b._tcp.example.com/other"),
            ],
            ..Default::default()
        };
        assert_eq!(
            http.srv_names(),
            ["_a._tcp.example.com", "_b._tcp.example.com"]
        );
    }
}
