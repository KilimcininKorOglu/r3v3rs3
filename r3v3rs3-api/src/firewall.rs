//! Host ports that the r3v3rs3 firewall table opens.

use crate::error::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};
use utoipa::ToSchema;

/// The `[firewall]` settings. Off by default, because the table drops every port that r3v3rs3
/// does not know, such as a VPN port of the host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FirewallConfig {
    /// Whether r3v3rs3 keeps the `inet r3v3rs3` nftables table. The environment variable
    /// `R3V3RS3_FIREWALL_ENABLED` overrides it.
    #[serde(default)]
    pub enabled: bool,
    /// Ports that the table opens besides the ports of r3v3rs3 and SSH, such as `udp/51820`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(value_type = Vec<String>, example = json!(["udp/51820"]))]
    pub extra_ports: Vec<HostPort>,
}

/// The state of the firewall table, as the last update left it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FirewallStatus {
    /// Whether r3v3rs3 manages the table.
    pub enabled: bool,
    /// The ports that the table opens.
    #[schema(value_type = Vec<String>)]
    pub ports: Vec<HostPort>,
    /// Other firewalls of the host that can still drop the open ports.
    pub warnings: Vec<String>,
    /// The error of the last update; the table keeps its previous ports then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// When the last update finished, in Unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<u64>,
}

/// The transport protocol of a host port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Transport {
    Tcp,
    Udp,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
        }
    }
}

/// Whether the host firewall lets clients outside the host reach a port.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PortFirewall {
    /// The table opens the port and no other firewall was found.
    Open,
    /// The table opens the port, but another firewall of the host can still drop it.
    OpenWithWarnings,
    /// The table does not open the port yet, for example after a failed update.
    Closed,
}

impl FirewallStatus {
    /// The state of a port that listens on `listen`; `None` when r3v3rs3 does not manage the
    /// firewall or the port listens only on a loopback address.
    pub fn port_state(&self, listen: &crate::multiaddr::Multiaddr) -> Option<PortFirewall> {
        if !self.enabled || listen.ip_addr().is_ok_and(|ip| ip.is_loopback()) {
            return None;
        }
        let transport = match listen.is_udp() || listen.is_quic() {
            true => Transport::Udp,
            false => Transport::Tcp,
        };
        let span = listen.port_range().ok()?;
        let open = self
            .ports
            .iter()
            .any(|p| p.transport == transport && p.first <= span.first && span.last <= p.last);
        Some(match (open, self.warnings.is_empty()) {
            (false, _) => PortFirewall::Closed,
            (true, true) => PortFirewall::Open,
            (true, false) => PortFirewall::OpenWithWarnings,
        })
    }
}

/// One port or an inclusive port range of one protocol, written `tcp/443` or `udp/49160-49240`.
/// A field of this type names `#[schema(value_type = String)]`, because it travels as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, schemars::JsonSchema)]
#[schemars(with = "String")]
pub struct HostPort {
    pub transport: Transport,
    pub first: u16,
    pub last: u16,
}

impl HostPort {
    pub fn single(transport: Transport, port: u16) -> Self {
        Self {
            transport,
            first: port,
            last: port,
        }
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.transport.as_str(), self.first)?;
        if self.last != self.first {
            write!(f, "-{}", self.last)?;
        }
        Ok(())
    }
}

/// A port number from 1 to 65535 in plain decimal digits.
fn parse_port(text: &str) -> Option<u16> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|port| *port != 0)
}

impl FromStr for HostPort {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let invalid = || Error::InvalidHostPort {
            value: value.to_string(),
        };
        let (proto, ports) = value.split_once('/').ok_or_else(invalid)?;
        let transport = match proto {
            "tcp" => Transport::Tcp,
            "udp" => Transport::Udp,
            _ => return Err(invalid()),
        };
        let (first, last) = ports.split_once('-').unwrap_or((ports, ports));
        let first = parse_port(first).ok_or_else(invalid)?;
        let last = parse_port(last).ok_or_else(invalid)?;
        if first > last {
            return Err(invalid());
        }
        Ok(Self {
            transport,
            first,
            last,
        })
    }
}

impl Serialize for HostPort {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for HostPort {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The written form names the protocol and one port or an inclusive range of ports
    /// from 1 to 65535; every other text is refused.
    #[test]
    fn a_host_port_is_a_protocol_and_a_port_or_range() {
        let range: HostPort = "udp/49160-49240".parse().unwrap();
        assert_eq!(
            range,
            HostPort {
                transport: Transport::Udp,
                first: 49160,
                last: 49240
            }
        );
        assert_eq!(range.to_string(), "udp/49160-49240");
        let single: HostPort = "tcp/3478".parse().unwrap();
        assert_eq!(single, HostPort::single(Transport::Tcp, 3478));
        assert_eq!(single.to_string(), "tcp/3478");
        for bad in [
            "",
            "tcp",
            "tcp/",
            "tcp/0",
            "udp/70000",
            "x/1",
            "udp/5-3",
            "udp/1-",
            "udp/-1",
            "tcp/+80",
            "tcp/ 80",
            "TCP/80",
            "udp/1-2-3",
            "tcp/80/",
            "sctp/80",
        ] {
            assert!(bad.parse::<HostPort>().is_err(), "{bad:?}");
        }
    }

    /// A port is open only when the table holds its protocol and number; another firewall
    /// turns open into a warning, and an unmanaged firewall or a loopback port reports nothing.
    #[test]
    fn a_port_is_open_only_for_its_own_protocol_and_number() {
        let addr = |text: &str| text.parse::<crate::multiaddr::Multiaddr>().unwrap();
        let mut status = FirewallStatus {
            enabled: true,
            ports: vec![
                "tcp/443".parse().unwrap(),
                "udp/49160-49240".parse().unwrap(),
            ],
            ..Default::default()
        };
        let https = addr("/ip4/0.0.0.0/tcp/443/https");
        assert_eq!(status.port_state(&https), Some(PortFirewall::Open));
        let quic = addr("/ip4/0.0.0.0/udp/443/quic/http");
        assert_eq!(status.port_state(&quic), Some(PortFirewall::Closed));
        let relay = addr("/ip4/0.0.0.0/udp/49200");
        assert_eq!(status.port_state(&relay), Some(PortFirewall::Open));
        let outside = addr("/ip4/0.0.0.0/udp/49241");
        assert_eq!(status.port_state(&outside), Some(PortFirewall::Closed));
        // A range is open only when every one of its ports is.
        let inside = addr("/ip4/0.0.0.0/udp/49160-49240");
        assert_eq!(status.port_state(&inside), Some(PortFirewall::Open));
        let wider = addr("/ip4/0.0.0.0/udp/49160-49241");
        assert_eq!(status.port_state(&wider), Some(PortFirewall::Closed));
        assert_eq!(status.port_state(&addr("/ip4/127.0.0.1/tcp/443")), None);
        status.warnings.push("firewalld is active".into());
        assert_eq!(
            status.port_state(&https),
            Some(PortFirewall::OpenWithWarnings)
        );
        status.enabled = false;
        assert_eq!(status.port_state(&https), None);
    }

    #[test]
    fn a_host_port_travels_as_its_written_form() {
        let ports: Vec<HostPort> = serde_json::from_str(r#"["udp/3478","tcp/1-2"]"#).unwrap();
        assert_eq!(
            serde_json::to_string(&ports).unwrap(),
            r#"["udp/3478","tcp/1-2"]"#
        );
        assert!(serde_json::from_str::<Vec<HostPort>>(r#"["udp/0"]"#).is_err());
    }
}
