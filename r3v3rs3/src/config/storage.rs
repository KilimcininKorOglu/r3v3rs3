use crate::audit::AuditStore;
use crate::cdn::CdnRanges;
use crate::certs::{acme::AcmeEntry, challenges::ServedChallenges, Cert};
use crate::proxy::http::cache_share::SharedCacheStore;
use crate::proxy::http::rate_share::RateCountExchange;
use crate::sessions::{LocalSessions, SessionBackend};
use r3v3rs3_api::{
    access_list::AccessListEntry,
    app::AppConfig,
    auth::{Account, LoginRequest, LoginResponse, Role},
    error::Error,
    id::ShortId,
    port::PortEntry,
    proxy::ProxyEntry,
};
use std::collections::HashMap;
use std::sync::Arc;

#[async_trait::async_trait]
pub trait Storage: Send + Sync + 'static {
    /// Fails when the storage cannot save a change now. The server calls it before a change
    /// through the admin API, so a change that cannot be saved is not applied.
    async fn ensure_writable(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn save_app_config(&self, config: &AppConfig) -> Result<(), Error>;
    async fn load_app_config(&self) -> AppConfig;
    async fn save_ports(&self, entries: &[PortEntry]) -> Result<(), Error>;
    async fn load_ports(&self) -> Vec<PortEntry>;
    async fn load_proxies(&self) -> Vec<ProxyEntry>;
    async fn save_proxies(&self, proxies: &[ProxyEntry]) -> Result<(), Error>;

    /// The access lists in their saved order. A list that cannot be loaded is left out.
    async fn load_access_lists(&self) -> Vec<AccessListEntry>;
    async fn save_access_lists(&self, lists: &[AccessListEntry]) -> Result<(), Error>;
    async fn save_cert(&self, cert: &Cert) -> Result<(), Error>;
    async fn save_acme(&self, acme: &AcmeEntry) -> Result<(), Error>;
    async fn delete_acme(&self, id: ShortId) -> Result<(), Error>;
    async fn delete_cert(&self, id: ShortId) -> Result<(), Error>;
    async fn load_acmes(&self) -> Vec<AcmeEntry>;
    async fn load_certs(&self) -> Vec<Arc<Cert>>;
    async fn add_account(
        &self,
        name: &str,
        password: &str,
        totp: bool,
        role: Role,
    ) -> Result<Account, Error>;
    async fn verify_account(&self, request: LoginRequest) -> Result<LoginResponse, Error>;

    /// Every account by its user name.
    async fn load_accounts(&self) -> Result<HashMap<String, Account>, Error>;

    /// Replaces every account. In a cluster the save is a conflict when another node changed an
    /// account after the last load, so a check of the loaded accounts holds for the saved ones.
    async fn save_accounts(&self, accounts: &HashMap<String, Account>) -> Result<(), Error>;
    async fn save_cdn_ranges(&self, ranges: &CdnRanges) -> Result<(), Error>;
    async fn load_cdn_ranges(&self) -> Option<CdnRanges>;

    /// Stores the ACME challenges that every node of a cluster serves. A single server keeps them
    /// in memory only.
    async fn save_challenges(&self, _challenges: &ServedChallenges) -> Result<(), Error> {
        Ok(())
    }

    /// The challenges in the storage. `None` when the storage keeps no challenges.
    async fn load_challenges(&self) -> Option<ServedChallenges> {
        None
    }

    /// Records that this node serves the challenges.
    async fn ack_challenges(&self, _challenges: &ServedChallenges) -> Result<(), Error> {
        Ok(())
    }

    /// Waits until the other nodes serve the challenges.
    async fn wait_for_challenges(&self, _challenges: &ServedChallenges) {}

    /// The sessions of the admin API and the proxies. A cluster storage shares them between the
    /// nodes.
    fn session_backend(self: Arc<Self>) -> Arc<dyn SessionBackend> {
        Arc::new(LocalSessions::default())
    }

    /// The exchange of rate limit counts with the other nodes. `None` without a cluster.
    fn rate_count_exchange(self: Arc<Self>) -> Option<Arc<dyn RateCountExchange>> {
        None
    }

    /// The store of the cached responses that the nodes share. `None` without a cluster, and when
    /// `cluster.share_cache` is off.
    fn shared_cache_store(self: Arc<Self>) -> Option<Arc<dyn SharedCacheStore>> {
        None
    }

    /// The audit log that the nodes of a cluster share. `None` keeps the audit log in the log
    /// database of the server.
    fn audit_store(self: Arc<Self>) -> Option<Arc<dyn AuditStore>> {
        None
    }

    /// Purges the cache of the proxy on the other nodes. `at` is the purge time in Unix
    /// milliseconds.
    async fn purge_shared_cache(&self, _proxy: ShortId, _at: u64) -> Result<(), Error> {
        Ok(())
    }

    /// The last cache purge time of each proxy.
    async fn load_cache_purges(&self) -> HashMap<ShortId, u64> {
        HashMap::new()
    }

    /// Deletes the shared responses that expired. Only the leader calls it.
    async fn remove_expired_shared_responses(&self) {}
}
