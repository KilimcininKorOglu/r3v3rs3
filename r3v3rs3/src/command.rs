use crate::{
    accounts::Caller,
    cdn::CdnRanges,
    certs::{
        acme::{AcmeOrder, AcmeTarget},
        Cert,
    },
    cluster::layout::StateKind,
    discovery::DiscoverySnapshot,
    server::rpc::ErasedRpcMethod,
};
use r3v3rs3_api::cluster::ClusterStatus;
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
        caller: Caller,
    },
    SetCdnRanges {
        ranges: CdnRanges,
    },
    /// Replaces the state and the proxies of a discovery provider.
    SetDiscovery {
        snapshot: DiscoverySnapshot,
    },
    /// The cluster store changed these parts of the state.
    ClusterChanged {
        kinds: Vec<StateKind>,
    },
    SetClusterStatus {
        status: ClusterStatus,
    },
    /// This node gained or lost the leader lock.
    SetLeader {
        leader: bool,
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
            Self::CallMethod { id, caller, .. } => f
                .debug_struct("CallMethod")
                .field("id", id)
                .field("caller", &caller.username)
                .finish(),
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
            Self::ClusterChanged { kinds } => f
                .debug_struct("ClusterChanged")
                .field("kinds", kinds)
                .finish(),
            Self::SetClusterStatus { status } => f
                .debug_struct("SetClusterStatus")
                .field("status", status)
                .finish(),
            Self::SetLeader { leader } => {
                f.debug_struct("SetLeader").field("leader", leader).finish()
            }
        }
    }
}
