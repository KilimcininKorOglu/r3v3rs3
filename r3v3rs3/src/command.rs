use crate::{
    cdn::CdnRanges,
    certs::{acme::AcmeOrder, Cert},
    server::rpc::ErasedRpcMethod,
};
use r3v3rs3_api::discovery::DiscoveryProvider;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::proxy::ProxyEntry;
use std::sync::Arc;

pub enum ServerCommand {
    AddCert {
        cert: Arc<Cert>,
    },
    SetBroadcastEvents {
        enabled: bool,
    },
    AddAcmeOrders {
        orders: Vec<AcmeOrder>,
    },
    AcmeOrderFinished {
        id: ShortId,
        succeeded: bool,
    },
    CallMethod {
        id: usize,
        arg: Box<dyn ErasedRpcMethod>,
    },
    SetCdnRanges {
        ranges: CdnRanges,
    },
    /// Replaces every proxy of the provider with these proxies.
    SetDiscoveredProxies {
        provider: DiscoveryProvider,
        entries: Vec<ProxyEntry>,
    },
}

impl std::fmt::Debug for ServerCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddCert { cert } => f.debug_struct("AddCert").field("id", &cert.id()).finish(),
            Self::SetBroadcastEvents { enabled } => f
                .debug_struct("SetBroadcastEvents")
                .field("enabled", enabled)
                .finish(),
            Self::AddAcmeOrders { orders } => f
                .debug_struct("AddAcmeOrders")
                .field("orders", &orders.len())
                .finish(),
            Self::AcmeOrderFinished { id, succeeded } => f
                .debug_struct("AcmeOrderFinished")
                .field("id", id)
                .field("succeeded", succeeded)
                .finish(),
            Self::CallMethod { id, .. } => f.debug_struct("CallMethod").field("id", id).finish(),
            Self::SetCdnRanges { ranges } => f
                .debug_struct("SetCdnRanges")
                .field("updated_at", &ranges.updated_at)
                .finish(),
            Self::SetDiscoveredProxies { provider, entries } => f
                .debug_struct("SetDiscoveredProxies")
                .field("provider", provider)
                .field("entries", &entries.len())
                .finish(),
        }
    }
}
