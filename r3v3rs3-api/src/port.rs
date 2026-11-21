use crate::{
    id::ShortId,
    multiaddr::Multiaddr,
    proxy_protocol::ProxyProtocolReceive,
    tls::{TlsState, TlsTermination},
    upstream::{DEFAULT_WEIGHT, default_weight, is_default_weight},
};
use serde_derive::{Deserialize, Serialize};
use std::{
    net::IpAddr,
    time::{Duration, SystemTime},
};
use utoipa::ToSchema;

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    ToSchema,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SocketState {
    Listening,
    Inactive,
    AddressAlreadyInUse,
    PermissionDenied,
    AddressNotAvailable,
    Error,
    #[default]
    Unknown,
}

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    ToSchema,
    schemars::JsonSchema,
)]
pub struct PortStatus {
    pub state: PortState,
    #[serde(
        serialize_with = "serialize_started_at",
        deserialize_with = "deserialize_started_at"
    )]
    #[schema(value_type = Option<u64>)]
    #[schemars(with = "Option<u64>")]
    pub started_at: Option<SystemTime>,
    /// Whether the host firewall table opens the port; absent when r3v3rs3 does not manage it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firewall: Option<crate::firewall::PortFirewall>,
}

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    ToSchema,
    schemars::JsonSchema,
)]
pub struct PortState {
    pub socket: SocketState,
    pub tls: Option<TlsState>,
}

fn serialize_started_at<S>(
    started_at: &Option<SystemTime>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if let Some(started_at) = started_at {
        // A time before the epoch serializes as zero instead of failing the response.
        let started_at = started_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        serializer.serialize_some(&started_at)
    } else {
        serializer.serialize_none()
    }
}

fn deserialize_started_at<'de, D>(deserializer: D) -> Result<Option<SystemTime>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let started_at = Option::<u64>::deserialize(deserializer)?;
    if let Some(started_at) = started_at {
        Ok(Some(
            SystemTime::UNIX_EPOCH + Duration::from_secs(started_at),
        ))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
pub struct UpstreamServer {
    #[schema(value_type = String, example = "/dns/example.com/tcp/8080")]
    pub addr: Multiaddr,
    /// Share of the traffic compared with the other servers. `0` sends no new traffic to the
    /// server.
    #[serde(default = "default_weight", skip_serializing_if = "is_default_weight")]
    #[schema(example = 1)]
    pub weight: u16,
    /// Optional geographical region or preset (e.g. "eu", "na", "tr", "apac") for geo-steering.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Optional ISO 3166-1 alpha-2 country codes assigned to this server for geo-steering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub countries: Vec<String>,
}

impl UpstreamServer {
    pub fn new(addr: Multiaddr) -> Self {
        Self {
            addr,
            weight: DEFAULT_WEIGHT,
            region: None,
            countries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
pub struct PortEntry {
    pub id: ShortId,
    #[schema(inline)]
    #[serde(flatten)]
    pub port: Port,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PortStatus>,
}

impl From<(ShortId, Port)> for PortEntry {
    fn from((id, port): (ShortId, Port)) -> Self {
        Self {
            id,
            port,
            status: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
pub struct Port {
    #[serde(default = "default_active", skip_serializing_if = "is_true")]
    pub active: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[schema(value_type = String, example = "/ip4/127.0.0.1/tcp/8080")]
    pub listen: Multiaddr,
    #[serde(flatten, default)]
    pub opts: PortOptions,
}

fn default_active() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

impl From<PortEntry> for (ShortId, Port) {
    fn from(entry: PortEntry) -> Self {
        (entry.id, entry.port)
    }
}

#[derive(
    Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema,
)]
pub struct PortOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_termination: Option<TlsTermination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_protocol: Option<ProxyProtocolReceive>,
    /// Overrides the global HTTP/2 flood limits of the config for this port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http2_limits: Option<crate::app::Http2Limits>,
    /// Overrides the global HTTP/3 stream limits of the config for this port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http3_limits: Option<crate::app::Http3Limits>,
    /// Enables adaptive stream priority optimization for HTTP/2 and HTTP/3 on this port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_priority: Option<crate::priority::StreamPriorityConfig>,
    /// The upstream sees the address of the client instead of the address of r3v3rs3. Only a
    /// TCP or UDP port can be transparent, and its upstream must listen on a non-loopback
    /// address, because the kernel refuses a loopback reply to an outside client.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transparent: bool,
}

impl Port {
    /// Checks the options that depend on the kind of the port: an HTTP or QUIC port serves
    /// its own clients, so it cannot be transparent.
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        if self.opts.transparent && (self.listen.is_http() || self.listen.is_quic()) {
            return Err(crate::error::Error::TransparentNotSupported {
                addr: self.listen.to_string(),
            });
        }
        Ok(())
    }
}

/// Request to turn the transparent option of a TCP or UDP port on or off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SetPortTransparentRequest {
    /// Whether the upstream servers see the client address.
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
pub struct NetworkInterface {
    pub name: String,
    pub addrs: Vec<NetworkAddr>,
    pub mac: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, schemars::JsonSchema)]
pub struct NetworkAddr {
    #[schema(value_type = String, example = "127.0.0.1")]
    pub ip: IpAddr,
    #[schema(value_type = String, example = "255.255.255.0")]
    pub mask: Option<IpAddr>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(listen: &str, transparent: bool) -> Port {
        Port {
            active: true,
            name: String::new(),
            listen: listen.parse().unwrap(),
            opts: PortOptions {
                transparent,
                ..Default::default()
            },
        }
    }

    /// An HTTP or QUIC port answers its clients itself, so only a TCP or UDP port can hand the
    /// client address to an upstream.
    #[test]
    fn only_a_tcp_or_udp_port_can_be_transparent() {
        for ok in [
            "/ip4/0.0.0.0/tcp/5349",
            "/ip4/0.0.0.0/tcp/5349/tls",
            "/ip4/0.0.0.0/udp/49160-49240",
        ] {
            assert!(port(ok, true).validate().is_ok(), "{ok}");
        }
        for bad in [
            "/ip4/0.0.0.0/tcp/80/http",
            "/ip4/0.0.0.0/tcp/443/https",
            "/ip4/0.0.0.0/udp/443/quic/http",
        ] {
            assert!(matches!(
                port(bad, true).validate(),
                Err(crate::error::Error::TransparentNotSupported { .. })
            ));
            assert!(port(bad, false).validate().is_ok(), "{bad}");
        }
    }

    /// A port saved before the field existed reads as not transparent, and the field is left
    /// out of the stored form while it is off.
    #[test]
    fn transparent_is_off_unless_set() {
        let old: Port = serde_json::from_str(r#"{"listen":"/ip4/0.0.0.0/udp/3478"}"#).unwrap();
        assert!(!old.opts.transparent);
        assert!(!serde_json::to_string(&old).unwrap().contains("transparent"));
        let on = port("/ip4/0.0.0.0/udp/3478", true);
        assert!(
            serde_json::to_string(&on)
                .unwrap()
                .contains(r#""transparent":true"#)
        );
    }
}
