use crate::error::Error;
use std::{
    fmt,
    net::{IpAddr, SocketAddr},
    str::FromStr,
};

#[derive(Debug, Default, Clone, PartialEq, Eq, schemars::JsonSchema)]
#[schemars(with = "String")]
pub struct Multiaddr {
    protocols: Vec<Protocol>,
}

impl Multiaddr {
    pub fn is_tls(&self) -> bool {
        self.protocols.iter().any(|p| matches!(p, Protocol::Tls))
    }

    pub fn is_http(&self) -> bool {
        self.protocols
            .iter()
            .any(|p| matches!(p, Protocol::Http(_)))
    }

    pub fn is_udp(&self) -> bool {
        self.protocols.iter().any(|p| matches!(p, Protocol::Udp(_)))
    }

    pub fn is_quic(&self) -> bool {
        self.protocols.iter().any(|p| matches!(p, Protocol::Quic))
    }

    pub fn socket_addr(&self) -> Result<SocketAddr, Error> {
        self.ip_addr()
            .and_then(|ip| self.port().map(|port| SocketAddr::new(ip, port)))
    }

    pub fn ip_addr(&self) -> Result<IpAddr, Error> {
        self.protocols
            .iter()
            .find_map(|p| match p {
                Protocol::Ip(addr) => Some(*addr),
                _ => None,
            })
            .ok_or_else(|| Error::InvalidMultiaddr {
                addr: self.to_string(),
            })
    }

    /// The port, or the first port of a range.
    pub fn port(&self) -> Result<u16, Error> {
        self.port_range().map(|span| span.first)
    }

    /// The port or the inclusive port range; one port is a range of one.
    pub fn port_range(&self) -> Result<PortSpan, Error> {
        self.protocols
            .iter()
            .find_map(|p| match p {
                Protocol::Tcp(span) | Protocol::Udp(span) => Some(*span),
                _ => None,
            })
            .ok_or_else(|| Error::InvalidMultiaddr {
                addr: self.to_string(),
            })
    }

    /// Whether the address names more than one port.
    pub fn is_range(&self) -> bool {
        self.port_range().is_ok_and(|span| span.port_count() > 1)
    }

    pub fn host(&self) -> Result<String, Error> {
        self.protocols
            .iter()
            .find_map(|p| match p {
                Protocol::Dns(host) => Some(host.clone()),
                Protocol::Ip(addr) => Some(addr.to_string()),
                _ => None,
            })
            .ok_or_else(|| Error::InvalidMultiaddr {
                addr: self.to_string(),
            })
    }

    pub fn protocol_name(&self) -> &'static str {
        match (self.is_udp(), self.is_http(), self.is_tls(), self.is_quic()) {
            (_, false, _, true) => "QUIC",
            (_, true, _, true) => "HTTP over QUIC",
            (true, _, _, _) => "UDP",
            (false, true, true, _) => "HTTPS",
            (false, true, false, _) => "HTTP",
            (false, false, true, _) => "TCP over TLS",
            (false, false, false, _) => "TCP",
        }
    }
}

/// The most ports one address may name: the size of the default TURN relay range
/// 49152-65535.
pub const MAX_RANGE_PORTS: u32 = 16384;

/// One port, or an inclusive range of ports written `first-last`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortSpan {
    pub first: u16,
    pub last: u16,
}

impl PortSpan {
    pub fn single(port: u16) -> Self {
        Self {
            first: port,
            last: port,
        }
    }

    /// The number of ports; a span always holds at least one.
    pub fn port_count(&self) -> u32 {
        u32::from(self.last) - u32::from(self.first) + 1
    }

    pub fn contains(&self, port: u16) -> bool {
        (self.first..=self.last).contains(&port)
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        self.first <= other.last && other.first <= self.last
    }

    /// The port at the same position of `other`: the upstream port of a client port.
    pub fn map_to(&self, port: u16, other: &Self) -> Option<u16> {
        let offset = port
            .checked_sub(self.first)
            .filter(|_| self.contains(port))?;
        other
            .first
            .checked_add(offset)
            .filter(|p| other.contains(*p))
    }

    /// Parses `443` (any port, as before ranges existed) or `49160-49240`: both ends from 1,
    /// the last above the first, at most [`MAX_RANGE_PORTS`] ports.
    fn parse(text: &str) -> Option<Self> {
        let Some((first, last)) = text.split_once('-') else {
            return text.parse().ok().map(Self::single);
        };
        let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
        if !digits(first) || !digits(last) {
            return None;
        }
        let span = Self {
            first: first.parse().ok().filter(|p| *p != 0)?,
            last: last.parse().ok()?,
        };
        (span.last > span.first && span.port_count() <= MAX_RANGE_PORTS).then_some(span)
    }
}

impl fmt::Display for PortSpan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.first)?;
        if self.last != self.first {
            write!(f, "-{}", self.last)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Protocol {
    Dns(String),
    Ip(IpAddr),
    Tcp(PortSpan),
    Udp(PortSpan),
    Tls,
    Http(String),
    Quic,
}

impl FromStr for Multiaddr {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut protocols = Vec::new();
        let mut rest = s.trim_start_matches('/');
        while !rest.is_empty() {
            let (protocol, next) = rest.split_once('/').unwrap_or((rest, ""));
            match protocol {
                "dns" => {
                    let (host, next) = next.split_once('/').unwrap_or((next, ""));
                    protocols.push(Protocol::Dns(host.to_string()));
                    rest = next;
                }
                "ip4" | "ip6" => {
                    let (addr, next) = next.split_once('/').unwrap_or((next, ""));
                    let addr = addr
                        .parse::<IpAddr>()
                        .map_err(|_| Error::InvalidMultiaddr {
                            addr: s.to_string(),
                        })?;
                    protocols.push(Protocol::Ip(addr));
                    rest = next;
                }
                "tcp" | "udp" => {
                    let (port, next) = next.split_once('/').unwrap_or((next, ""));
                    protocols.push(port_protocol(protocol, port, s)?);
                    rest = next;
                }
                "tls" => {
                    protocols.push(Protocol::Tls);
                    rest = next;
                }
                "quic" => {
                    protocols.push(Protocol::Quic);
                    rest = next;
                }
                "http" => {
                    protocols.push(Protocol::Http(format!("/{next}")));
                    rest = "";
                }
                "https" => {
                    protocols.push(Protocol::Tls);
                    protocols.push(Protocol::Http(format!("/{next}")));
                    rest = "";
                }
                _ => rest = next,
            }
        }
        let addr = Self { protocols };
        // A range is a plain TCP or UDP port: TLS, HTTP and QUIC serve one port.
        if addr.is_range() && (addr.is_tls() || addr.is_http() || addr.is_quic()) {
            return Err(Error::InvalidMultiaddr {
                addr: s.to_string(),
            });
        }
        Ok(addr)
    }
}

/// The `tcp/PORTS` or `udp/PORTS` segment of the address `whole`.
fn port_protocol(name: &str, ports: &str, whole: &str) -> Result<Protocol, Error> {
    let span = PortSpan::parse(ports).ok_or_else(|| Error::InvalidMultiaddr {
        addr: whole.to_string(),
    })?;
    Ok(match name {
        "tcp" => Protocol::Tcp(span),
        _ => Protocol::Udp(span),
    })
}

impl fmt::Display for Multiaddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for protocol in &self.protocols {
            self.write_protocol(f, protocol)?;
        }
        Ok(())
    }
}

impl Multiaddr {
    /// Writes one protocol segment. `/tls` is implicit in `/https`, so a TLS segment of an HTTP
    /// address writes nothing.
    fn write_protocol(&self, f: &mut fmt::Formatter<'_>, protocol: &Protocol) -> fmt::Result {
        match protocol {
            Protocol::Dns(host) => write!(f, "/dns/{}", host),
            Protocol::Ip(addr) if addr.is_ipv4() => write!(f, "/ip4/{}", addr),
            Protocol::Ip(addr) => write!(f, "/ip6/{}", addr),
            Protocol::Tcp(span) => write!(f, "/tcp/{span}"),
            Protocol::Udp(span) => write!(f, "/udp/{span}"),
            Protocol::Tls if self.is_http() => Ok(()),
            Protocol::Tls => write!(f, "/tls"),
            Protocol::Http(path) => self.write_http(f, path),
            Protocol::Quic => write!(f, "/quic"),
        }
    }

    /// Writes the HTTP segment with its scheme. A root path writes no path.
    fn write_http(&self, f: &mut fmt::Formatter<'_>, path: &str) -> fmt::Result {
        let path = if path == "/" { "" } else { path };
        let scheme = if self.is_tls() { "https" } else { "http" };
        write!(f, "/{scheme}{path}")
    }
}

impl serde::Serialize for Multiaddr {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for Multiaddr {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Multiaddr::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_multiaddr() {
        let addr = Multiaddr::from_str("/dns/example.com/tcp/8080").unwrap();
        assert_eq!(addr.to_string(), "/dns/example.com/tcp/8080");
        assert!(!addr.is_http());
        assert!(!addr.is_tls());

        let addr = Multiaddr::from_str("/ip4/127.0.0.1/tcp/8080").unwrap();
        assert_eq!(addr.to_string(), "/ip4/127.0.0.1/tcp/8080");
        assert!(!addr.is_http());
        assert!(!addr.is_tls());

        let addr = Multiaddr::from_str("/ip4/127.0.0.1/tcp/8080/tls").unwrap();
        assert_eq!(addr.to_string(), "/ip4/127.0.0.1/tcp/8080/tls");
        assert!(!addr.is_http());
        assert!(addr.is_tls());

        let addr = Multiaddr::from_str("/ip4/127.0.0.1/tcp/8080/http").unwrap();
        assert_eq!(addr.to_string(), "/ip4/127.0.0.1/tcp/8080/http");
        assert!(addr.is_http());
        assert!(!addr.is_tls());

        let addr = Multiaddr::from_str("/ip6/::/tcp/8080/https/example.com/index.html").unwrap();
        assert_eq!(
            addr.to_string(),
            "/ip6/::/tcp/8080/https/example.com/index.html"
        );
        assert!(addr.is_http());
        assert!(addr.is_tls());

        let addr = Multiaddr::from_str("/ip4/127.0.0.1/udp/8080").unwrap();
        assert_eq!(addr.to_string(), "/ip4/127.0.0.1/udp/8080");
        assert!(!addr.is_http());
        assert!(!addr.is_tls());
        assert!(addr.is_udp());

        let addr = Multiaddr::from_str("/ip4/127.0.0.1/udp/8080/quic/http").unwrap();
        assert_eq!(addr.to_string(), "/ip4/127.0.0.1/udp/8080/quic/http");
        assert!(addr.is_http());
        assert!(!addr.is_tls());
        assert!(addr.is_udp());
        assert!(addr.is_quic());
    }

    /// A TURN relay range is one address: it keeps its written form, starts at its first port
    /// and maps each client port to the port at the same position of the upstream range.
    #[test]
    fn a_plain_tcp_or_udp_address_may_name_a_port_range() {
        let addr = Multiaddr::from_str("/ip4/0.0.0.0/udp/49160-49240").unwrap();
        assert_eq!(addr.to_string(), "/ip4/0.0.0.0/udp/49160-49240");
        assert_eq!(addr.port().unwrap(), 49160);
        let span = addr.port_range().unwrap();
        assert_eq!(span.port_count(), 81);
        assert!(addr.is_range());
        assert!(
            !Multiaddr::from_str("/ip4/0.0.0.0/tcp/443")
                .unwrap()
                .is_range()
        );
        let upstream = Multiaddr::from_str("/dns/turn.example.com/udp/50160-50240").unwrap();
        let up = upstream.port_range().unwrap();
        assert_eq!(span.map_to(49160, &up), Some(50160));
        assert_eq!(span.map_to(49240, &up), Some(50240));
        assert_eq!(span.map_to(49241, &up), None);
        assert_eq!(span.map_to(49159, &up), None);
        let tcp = Multiaddr::from_str("/ip6/::/tcp/8000-8010").unwrap();
        assert_eq!(tcp.to_string(), "/ip6/::/tcp/8000-8010");
        assert_eq!(
            Multiaddr::from_str("/ip4/0.0.0.0/udp/49152-65535")
                .unwrap()
                .port_range()
                .unwrap()
                .port_count(),
            MAX_RANGE_PORTS
        );
    }

    /// Every malformed range is refused, and so is a range on a TLS, HTTP or QUIC address,
    /// because those serve one port.
    #[test]
    fn a_malformed_or_non_plain_range_is_refused() {
        for bad in [
            "/ip4/0.0.0.0/udp/9-1",
            "/ip4/0.0.0.0/udp/5-5",
            "/ip4/0.0.0.0/udp/0-10",
            "/ip4/0.0.0.0/udp/1-",
            "/ip4/0.0.0.0/udp/-1",
            "/ip4/0.0.0.0/udp/1-2-3",
            "/ip4/0.0.0.0/udp/+1-5",
            "/ip4/0.0.0.0/udp/1-70000",
            "/ip4/0.0.0.0/udp/1000-17384",
            "/ip4/0.0.0.0/tcp/8000-8010/tls",
            "/ip4/0.0.0.0/tcp/8000-8010/http",
            "/ip4/0.0.0.0/tcp/8000-8010/https",
            "/ip4/0.0.0.0/udp/8000-8010/quic/http",
        ] {
            assert!(Multiaddr::from_str(bad).is_err(), "{bad}");
        }
    }
}
