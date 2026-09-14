use crate::{
    cdn::CdnRanges,
    certs::{
        acme::{AcmeOrder, AcmeTarget},
        Cert,
    },
    discovery::DiscoverySnapshot,
    server::rpc::ErasedRpcMethod,
};
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
        target: AcmeTarget,
        succeeded: bool,
    },
    CallMethod {
        id: usize,
        arg: Box<dyn ErasedRpcMethod>,
    },
    SetCdnRanges {
        ranges: CdnRanges,
    },
    /// Replaces the state and the proxies of a discovery provider.
    SetDiscovery {
        snapshot: DiscoverySnapshot,
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
            Self::AcmeOrderFinished { target, succeeded } => f
                .debug_struct("AcmeOrderFinished")
                .field("target", target)
                .field("succeeded", succeeded)
                .finish(),
            Self::CallMethod { id, .. } => f.debug_struct("CallMethod").field("id", id).finish(),
            Self::SetCdnRanges { ranges } => f
                .debug_struct("SetCdnRanges")
                .field("updated_at", &ranges.updated_at)
                .finish(),
            // The definitions can hold plain text passwords, so only the counts are printed.
            Self::SetDiscovery { snapshot } => f
                .debug_struct("SetDiscovery")
                .field("provider", &snapshot.provider)
                .field("state", &snapshot.state)
                .field("proxies", &snapshot.proxies.as_ref().map(Vec::len))
                .finish(),
        }
    }
}
