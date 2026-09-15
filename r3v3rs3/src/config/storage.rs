use crate::cdn::CdnRanges;
use crate::certs::{acme::AcmeEntry, challenges::ServedChallenges, Cert};
use r3v3rs3_api::{
    app::AppConfig,
    auth::{Account, LoginRequest, LoginResponse},
    error::Error,
    id::ShortId,
    port::PortEntry,
    proxy::ProxyEntry,
};
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
    async fn save_cert(&self, cert: &Cert) -> Result<(), Error>;
    async fn save_acme(&self, acme: &AcmeEntry) -> Result<(), Error>;
    async fn delete_acme(&self, id: ShortId) -> Result<(), Error>;
    async fn delete_cert(&self, id: ShortId) -> Result<(), Error>;
    async fn load_acmes(&self) -> Vec<AcmeEntry>;
    async fn load_certs(&self) -> Vec<Arc<Cert>>;
    async fn add_account(&self, name: &str, password: &str, totp: bool) -> Result<Account, Error>;
    async fn verify_account(&self, request: LoginRequest) -> Result<LoginResponse, Error>;
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
}
