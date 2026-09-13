//! Proxy definitions in labels, tags and keys: `r3v3rs3.<protocol>.<name>.<field>=<value>`.
//! The fields follow the proxy model of the admin API.

use super::tree::{from_node, Node};
use super::ProxyDefinition;
use r3v3rs3_api::proxy::{HttpProxy, ProxyKind, TcpProxy, UdpProxy};
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;

pub const PREFIX: &str = "r3v3rs3";

pub const ENABLE: &str = "r3v3rs3.enable";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Protocol {
    Http,
    Tcp,
    Udp,
}

impl FromStr for Protocol {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "http" => Ok(Self::Http),
            "tcp" => Ok(Self::Tcp),
            "udp" => Ok(Self::Udp),
            _ => Err(()),
        }
    }
}

impl fmt::Display for Protocol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Http => "http",
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        })
    }
}

/// The definitions of one resource, and one message for each definition that is not valid.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub definitions: Vec<ProxyDefinition>,
    pub issues: Vec<String>,
}

/// Returns the value of the `r3v3rs3.enable` label.
pub fn enabled<'a>(labels: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<bool> {
    labels
        .into_iter()
        .find(|(key, _)| *key == ENABLE)
        .map(|(_, value)| value.trim().eq_ignore_ascii_case("true"))
}

/// Reads the proxy definitions from the labels. `host` is the address of the resource, which the
/// `port` fields use. Labels without the `r3v3rs3.` prefix are skipped.
pub fn parse<'a>(
    labels: impl IntoIterator<Item = (&'a str, &'a str)>,
    host: Option<&str>,
) -> Parsed {
    let mut parsed = Parsed::default();
    let mut groups = BTreeMap::<(Protocol, String), Node>::new();
    for (key, value) in labels {
        if let Err(issue) = add_label(&mut groups, key, value) {
            parsed.issues.push(issue);
        }
    }
    for ((protocol, name), node) in groups {
        match definition(protocol, &name, node, host) {
            Ok(definition) => parsed.definitions.push(definition),
            Err(issue) => parsed.issues.push(format!("{protocol}.{name}: {issue}")),
        }
    }
    parsed
}

fn add_label(
    groups: &mut BTreeMap<(Protocol, String), Node>,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let Some(rest) = key
        .strip_prefix(PREFIX)
        .and_then(|rest| rest.strip_prefix('.'))
    else {
        return Ok(());
    };
    if key == ENABLE {
        return Ok(());
    }
    let unknown = || format!("unknown label: {key}");
    let mut parts = rest.split('.');
    let protocol = parts
        .next()
        .and_then(|protocol| protocol.parse::<Protocol>().ok())
        .ok_or_else(unknown)?;
    let name = parts
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(unknown)?;
    let path = parts.collect::<Vec<_>>();
    if path.is_empty() || path.iter().any(|part| part.is_empty()) {
        return Err(unknown());
    }
    let node = groups.entry((protocol, name.to_string())).or_default();
    if node.insert(&path, value) {
        Ok(())
    } else {
        Err(format!("label conflicts with another label: {key}"))
    }
}

fn definition(
    protocol: Protocol,
    name: &str,
    mut node: Node,
    host: Option<&str>,
) -> Result<ProxyDefinition, String> {
    let ports = take::<Vec<String>>(&mut node, "ports")?.unwrap_or_default();
    if ports.is_empty() {
        return Err("ports is required".into());
    }
    let active = take::<bool>(&mut node, "active")?.unwrap_or(true);
    let display_name = take::<String>(&mut node, "name")?.unwrap_or_else(|| name.to_string());
    let kind = match protocol {
        Protocol::Http => {
            expand_http_ports(&mut node, host)?;
            ProxyKind::Http(Box::new(read::<HttpProxy>(node)?))
        }
        Protocol::Tcp => {
            expand_stream_port(&mut node, host, "tcp")?;
            ProxyKind::Tcp(read::<TcpProxy>(node)?)
        }
        Protocol::Udp => {
            expand_stream_port(&mut node, host, "udp")?;
            ProxyKind::Udp(read::<UdpProxy>(node)?)
        }
    };
    Ok(ProxyDefinition {
        key: format!("{protocol}.{name}"),
        name: display_name,
        ports,
        active,
        kind,
    })
}

fn read<T: DeserializeOwned>(node: Node) -> Result<T, String> {
    let (value, unknown) = from_node::<T>(node).map_err(|err| err.to_string())?;
    if unknown.is_empty() {
        Ok(value)
    } else {
        Err(format!("unknown keys: {}", unknown.join(", ")))
    }
}

fn take<T: DeserializeOwned>(node: &mut Node, key: &str) -> Result<Option<T>, String> {
    node.remove(key)
        .map(|value| read::<T>(value).map_err(|err| format!("{key}: {err}")))
        .transpose()
}

/// `port` and `scheme` of the proxy define one route to `/`. `port` and `scheme` of a route
/// define its only server.
fn expand_http_ports(node: &mut Node, host: Option<&str>) -> Result<(), String> {
    if let Some(url) = take_upstream_url(node, host)? {
        if node.contains("routes") {
            return Err("port cannot be used together with routes".into());
        }
        node.insert(&["routes", "0", "servers", "0", "url"], &url);
    }
    let routes = node
        .child_mut("routes")
        .into_iter()
        .flat_map(Node::children_mut);
    for route in routes {
        if let Some(url) = take_upstream_url(route, host)? {
            if route.contains("servers") {
                return Err("port cannot be used together with servers".into());
            }
            route.insert(&["servers", "0", "url"], &url);
        }
    }
    Ok(())
}

fn take_upstream_url(node: &mut Node, host: Option<&str>) -> Result<Option<String>, String> {
    let scheme = take::<String>(node, "scheme")?;
    let Some(port) = take::<u16>(node, "port")? else {
        return match scheme {
            Some(_) => Err("scheme needs port".into()),
            None => Ok(None),
        };
    };
    let host = host.ok_or("port needs the address of the resource")?;
    let host = match host.parse::<IpAddr>() {
        Ok(IpAddr::V6(ip)) => format!("[{ip}]"),
        _ => host.to_string(),
    };
    let scheme = scheme.as_deref().unwrap_or("http");
    Ok(Some(format!("{scheme}://{host}:{port}")))
}

/// `port` of a TCP or UDP proxy defines its only upstream server.
fn expand_stream_port(node: &mut Node, host: Option<&str>, transport: &str) -> Result<(), String> {
    let Some(port) = take::<u16>(node, "port")? else {
        return Ok(());
    };
    if node.contains("upstream_servers") {
        return Err("port cannot be used together with upstream_servers".into());
    }
    let host = host.ok_or("port needs the address of the resource")?;
    let family = match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(_)) => "ip4",
        Ok(IpAddr::V6(_)) => "ip6",
        Err(_) => "dns",
    };
    let addr = format!("/{family}/{host}/{transport}/{port}");
    node.insert(&["upstream_servers", "0", "addr"], &addr);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(pairs: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        pairs.to_vec()
    }

    fn http(definition: &ProxyDefinition) -> &HttpProxy {
        let ProxyKind::Http(http) = &definition.kind else {
            panic!("expected an HTTP proxy");
        };
        http
    }

    #[test]
    fn a_port_defines_the_route_and_the_server() {
        let parsed = parse(
            labels(&[
                ("r3v3rs3.enable", "true"),
                ("r3v3rs3.http.app.ports", "https"),
                ("r3v3rs3.http.app.vhosts", "app.example.com"),
                ("r3v3rs3.http.app.port", "8080"),
                ("com.docker.compose.service", "app"),
            ]),
            Some("172.18.0.5"),
        );
        assert_eq!(parsed.issues, Vec::<String>::new());
        let definition = &parsed.definitions[0];
        assert_eq!(definition.key, "http.app");
        assert_eq!(definition.name, "app");
        assert_eq!(definition.ports, ["https"]);
        assert!(definition.active);
        let http = http(definition);
        assert_eq!(http.routes[0].path, "/");
        assert_eq!(
            http.routes[0].servers[0].url.to_string(),
            "http://172.18.0.5:8080/"
        );
    }

    #[test]
    fn routes_can_use_their_own_ports() {
        let parsed = parse(
            labels(&[
                ("r3v3rs3.http.app.ports", "http,https"),
                ("r3v3rs3.http.app.routes.0.path", "/"),
                ("r3v3rs3.http.app.routes.0.port", "8080"),
                ("r3v3rs3.http.app.routes.1.path", "/api"),
                ("r3v3rs3.http.app.routes.1.port", "9090"),
                ("r3v3rs3.http.app.routes.1.scheme", "https"),
                ("r3v3rs3.http.app.name", "My App"),
                ("r3v3rs3.http.app.active", "false"),
            ]),
            Some("fd00::5"),
        );
        assert_eq!(parsed.issues, Vec::<String>::new());
        let definition = &parsed.definitions[0];
        assert_eq!(definition.ports, ["http", "https"]);
        assert_eq!(definition.name, "My App");
        assert!(!definition.active);
        let urls = http(definition)
            .routes
            .iter()
            .map(|route| route.servers[0].url.to_string())
            .collect::<Vec<_>>();
        assert_eq!(urls, ["http://[fd00::5]:8080/", "https://[fd00::5]:9090/"]);
    }

    #[test]
    fn tcp_and_udp_ports_define_the_upstream_server() {
        let parsed = parse(
            labels(&[
                ("r3v3rs3.tcp.db.ports", "postgres"),
                ("r3v3rs3.tcp.db.port", "5432"),
                ("r3v3rs3.udp.dns.ports", "dns"),
                ("r3v3rs3.udp.dns.port", "53"),
                ("r3v3rs3.udp.dns.session_idle_timeout", "30s"),
            ]),
            Some("db.internal"),
        );
        assert_eq!(parsed.issues, Vec::<String>::new());
        let addrs = parsed
            .definitions
            .iter()
            .map(|definition| match &definition.kind {
                ProxyKind::Tcp(tcp) => tcp.upstream_servers[0].addr.to_string(),
                ProxyKind::Udp(udp) => udp.upstream_servers[0].addr.to_string(),
                ProxyKind::Http(_) => String::new(),
            })
            .collect::<Vec<_>>();
        assert_eq!(
            addrs,
            ["/dns/db.internal/tcp/5432", "/dns/db.internal/udp/53"]
        );
    }

    #[test]
    fn invalid_definitions_become_issues() {
        let parsed = parse(
            labels(&[
                ("r3v3rs3.http.noports.port", "80"),
                ("r3v3rs3.http.typo.ports", "http"),
                ("r3v3rs3.http.typo.port", "80"),
                ("r3v3rs3.http.typo.vhost", "app"),
                ("r3v3rs3.http.both.ports", "http"),
                ("r3v3rs3.http.both.port", "80"),
                ("r3v3rs3.http.both.routes.0.path", "/"),
                ("r3v3rs3.http.nohost.ports", "http"),
                ("r3v3rs3.http.nohost.port", "80"),
                ("r3v3rs3.grpc.app.ports", "http"),
                ("r3v3rs3.http", "x"),
                ("traefik.enable", "true"),
            ]),
            None,
        );
        assert!(parsed.definitions.is_empty());
        assert_eq!(
            parsed.issues,
            [
                "unknown label: r3v3rs3.grpc.app.ports",
                "unknown label: r3v3rs3.http",
                "http.both: port needs the address of the resource",
                "http.nohost: port needs the address of the resource",
                "http.noports: ports is required",
                "http.typo: port needs the address of the resource",
            ]
        );

        let parsed = parse(
            labels(&[
                ("r3v3rs3.http.both.ports", "http"),
                ("r3v3rs3.http.both.port", "80"),
                ("r3v3rs3.http.both.routes.0.path", "/"),
                ("r3v3rs3.http.typo.ports", "http"),
                ("r3v3rs3.http.typo.port", "80"),
                ("r3v3rs3.http.typo.vhost", "app"),
            ]),
            Some("10.0.0.1"),
        );
        assert_eq!(
            parsed.issues,
            [
                "http.both: port cannot be used together with routes",
                "http.typo: unknown keys: vhost",
            ]
        );
    }

    #[test]
    fn enable_reads_the_enable_label() {
        assert_eq!(enabled(labels(&[("r3v3rs3.enable", "True")])), Some(true));
        assert_eq!(enabled(labels(&[("r3v3rs3.enable", "no")])), Some(false));
        assert_eq!(enabled(labels(&[("traefik.enable", "true")])), None);
    }
}
