use crate::cache::CacheConfig;
use crate::client_ip::ClientIpConfig;
use crate::compression::Compression;
use crate::discovery::DiscoverySource;
use crate::error::Error;
use crate::header_rules::HeaderRules;
use crate::policy::{AuthPolicy, IpFilter, RateLimit};
use crate::upstream::{
    default_connect_timeout, default_session_idle_timeout, default_weight,
    is_default_connect_timeout, is_default_session_idle_timeout, is_default_weight,
    validate_timeout, validate_weights, CircuitBreaker, HealthCheck, LoadBalancing, RetryPolicy,
    UpstreamHealth, UpstreamTimeouts, DEFAULT_WEIGHT,
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

    /// Rejects a zero connect timeout, session idle timeout, fail timeout or check timeout, an
    /// invalid health check path, a server list in which every server has weight 0, an invalid
    /// circuit breaker and invalid retry attempts.
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
                http.health_check.validate(true)?;
                http.circuit_breaker.validate()?;
                http.routes.iter().try_for_each(|route| {
                    validate_weights(route.servers.iter().map(|server| server.weight))
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
    /// Replaces the proxy header rules for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeaderRules>,
    /// Replaces the proxy upstream timeouts for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeouts: Option<UpstreamTimeouts>,
    /// Replaces the proxy retry policy for this route.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryPolicy>,
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
            .filter(|url| url.authority().is_some())
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
