//! Host ports that the r3v3rs3 firewall table opens.

use crate::error::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

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
