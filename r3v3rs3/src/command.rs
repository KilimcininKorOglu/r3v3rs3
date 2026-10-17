use crate::{
    accounts::Caller,
    cdn::CdnRanges,
    certs::{
        Cert,
        acme::{AcmeOrder, AcmeTarget},
    },
    discovery::DiscoverySnapshot,
    notify::Notification,
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
    /// An ACME order ended. `error` describes the failure of a failed order.
    AcmeOrderFinished {
        target: AcmeTarget,
        error: Option<String>,
    },
    CallMethod {
        id: usize,
        arg: Box<dyn ErasedRpcMethod>,
        caller: Caller,
    },
    SetCdnRanges {
        ranges: CdnRanges,
    },
    /// The app store refresh downloaded a newer catalog.
    SetAppStore {
        catalog: r3v3rs3_api::store::StoreCatalog,
    },
    /// The engine catalog refresh downloaded a newer catalog.
    SetEngines {
        catalog: r3v3rs3_api::platform::EnginesCatalog,
    },
    /// The update check learned the newest release, or that this build is current again.
    SetUpdateAvailable {
        tag: Option<String>,
    },
    /// Replaces the state and the proxies of a discovery provider.
    SetDiscovery {
        snapshot: DiscoverySnapshot,
    },
    /// The targets of a DNS SRV name changed, so the proxies must be reloaded.
    SrvUpdated {
        name: String,
    },
    /// A deployment platform event for the webhook of the settings.
    Notify {
        notification: Notification,
    },
    /// The worker wrote the new configuration into its storage, so the ports, the proxies, the
    /// access lists and the certificates must reload from it.
    WorkerApply,
    /// This process stops accepting connections, because the workers serve the ports.
    SuspendListeners,
    /// A supervisor asks for the configuration that a worker serves.
    WorkerConfigSnapshot {
        reply: tokio::sync::mpsc::Sender<crate::server::ipc::WorkerConfig>,
    },
    /// A supervisor reports what the worker with this pid serves. A `None` status marks the
    /// worker as unreachable.
    SetWorkerStatus {
        pid: u32,
        status: Option<crate::server::ipc::WorkerStatus>,
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
            Self::AcmeOrderFinished { target, error } => f
                .debug_struct("AcmeOrderFinished")
                .field("target", target)
                .field("error", error)
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
            Self::SetAppStore { catalog } => f
                .debug_struct("SetAppStore")
                .field("updated_at", &catalog.updated_at)
                .field("apps", &catalog.apps.len())
                .finish(),
            Self::SetEngines { catalog } => f
                .debug_struct("SetEngines")
                .field("updated_at", &catalog.updated_at)
                .field("engines", &catalog.engines.len())
                .finish(),
            Self::SetUpdateAvailable { tag } => f
                .debug_struct("SetUpdateAvailable")
                .field("tag", tag)
                .finish(),
            // The definitions can hold plain text passwords, so only the counts are printed.
            Self::SetDiscovery { snapshot } => f
                .debug_struct("SetDiscovery")
                .field("provider", &snapshot.provider)
                .field("state", &snapshot.state)
                .field("proxies", &snapshot.proxies.as_ref().map(Vec::len))
                .finish(),
            Self::SrvUpdated { name } => f.debug_struct("SrvUpdated").field("name", name).finish(),
            Self::Notify { notification } => f
                .debug_struct("Notify")
                .field("event", &notification.event)
                .finish(),
            Self::WorkerApply => f.debug_struct("WorkerApply").finish(),
            Self::SuspendListeners => f.debug_struct("SuspendListeners").finish(),
            Self::WorkerConfigSnapshot { .. } => f.debug_struct("WorkerConfigSnapshot").finish(),
            Self::SetWorkerStatus { pid, status } => f
                .debug_struct("SetWorkerStatus")
                .field("pid", pid)
                .field(
                    "open_connections",
                    &status.as_ref().map(|s| s.open_connections),
                )
                .finish(),
        }
    }
}
