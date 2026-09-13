//! Service discovery. A provider reads proxy definitions from an external system and sends a
//! snapshot to the server. The server turns the definitions into read-only proxies.

pub mod ids;
pub mod labels;
mod tree;

use r3v3rs3_api::discovery::{DiscoveryIssue, DiscoveryProvider, DiscoverySource, DiscoveryState};
use r3v3rs3_api::proxy::ProxyKind;

/// A proxy that a label set or a resource defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyDefinition {
    /// `<protocol>.<name>`, which is unique in one resource.
    pub key: String,
    pub name: String,
    /// Port names or port ids.
    pub ports: Vec<String>,
    pub active: bool,
    pub kind: ProxyKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredProxy {
    /// Identifies the proxy across reads, so the proxy keeps its id.
    pub key: String,
    pub source: DiscoverySource,
    pub definition: ProxyDefinition,
}

/// The result of one read of a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverySnapshot {
    pub provider: DiscoveryProvider,
    pub state: DiscoveryState,
    pub error: Option<String>,
    /// `None` keeps the proxies of the previous snapshot, for example after a connection error.
    pub proxies: Option<Vec<DiscoveredProxy>>,
    pub issues: Vec<DiscoveryIssue>,
}
