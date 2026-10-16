//! An in-memory [`Storage`] for a worker process, which receives its state from the master over
//! IPC instead of reading it from files or a cluster store.

use crate::cdn::CdnRanges;
use crate::certs::Cert;
use r3v3rs3_api::access_list::AccessListEntry;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::{Account, LoginRequest, Role};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::port::PortEntry;
use r3v3rs3_api::proxy::ProxyEntry;
use r3v3rs3_api::store::StoreCatalog;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Default)]
struct MemoryData {
    app: AppConfig,
    ports: Vec<PortEntry>,
    proxies: Vec<ProxyEntry>,
    access_lists: Vec<AccessListEntry>,
    certs: Vec<Arc<Cert>>,
    cdn: Option<CdnRanges>,
    app_store: Option<StoreCatalog>,
}

/// The storage of a worker process. A write keeps the value in memory only.
pub struct MemoryStorage {
    data: Mutex<MemoryData>,
}

impl MemoryStorage {
    /// A storage that answers the first loads with the state that the master sent.
    pub fn new(
        app: AppConfig,
        ports: Vec<PortEntry>,
        proxies: Vec<ProxyEntry>,
        access_lists: Vec<AccessListEntry>,
        certs: Vec<Arc<Cert>>,
    ) -> Self {
        Self {
            data: Mutex::new(MemoryData {
                app,
                ports,
                proxies,
                access_lists,
                certs,
                ..Default::default()
            }),
        }
    }

    /// Replaces the ports, the proxies and the access lists of a new configuration.
    pub async fn replace(
        &self,
        ports: Vec<PortEntry>,
        proxies: Vec<ProxyEntry>,
        access_lists: Vec<AccessListEntry>,
        certs: Vec<Arc<Cert>>,
    ) {
        let mut data = self.data.lock().await;
        data.ports = ports;
        data.proxies = proxies;
        data.access_lists = access_lists;
        data.certs = certs;
    }
}

#[async_trait::async_trait]
impl crate::config::storage::Storage for MemoryStorage {
    async fn save_app_config(&self, config: &AppConfig) -> Result<(), Error> {
        self.data.lock().await.app = config.clone();
        Ok(())
    }

    async fn load_app_config(&self) -> AppConfig {
        self.data.lock().await.app.clone()
    }

    async fn save_ports(&self, entries: &[PortEntry]) -> Result<(), Error> {
        self.data.lock().await.ports = entries.to_vec();
        Ok(())
    }

    async fn load_ports(&self) -> Vec<PortEntry> {
        self.data.lock().await.ports.clone()
    }

    async fn load_proxies(&self) -> Vec<ProxyEntry> {
        self.data.lock().await.proxies.clone()
    }

    async fn save_proxies(&self, proxies: &[ProxyEntry]) -> Result<(), Error> {
        self.data.lock().await.proxies = proxies.to_vec();
        Ok(())
    }

    async fn load_access_lists(&self) -> Vec<AccessListEntry> {
        self.data.lock().await.access_lists.clone()
    }

    async fn save_access_lists(&self, lists: &[AccessListEntry]) -> Result<(), Error> {
        self.data.lock().await.access_lists = lists.to_vec();
        Ok(())
    }

    async fn save_cert(&self, cert: &Cert) -> Result<(), Error> {
        let mut data = self.data.lock().await;
        data.certs.retain(|item| item.id != cert.id);
        data.certs.push(Arc::new(cert.clone()));
        Ok(())
    }

    async fn delete_cert(&self, id: ShortId) -> Result<(), Error> {
        self.data.lock().await.certs.retain(|cert| cert.id != id);
        Ok(())
    }

    async fn load_certs(&self) -> Vec<Arc<Cert>> {
        self.data.lock().await.certs.clone()
    }

    async fn save_acme(&self, _acme: &crate::certs::acme::AcmeEntry) -> Result<(), Error> {
        // A worker never orders certificates, so it keeps no ACME entries.
        Ok(())
    }

    async fn delete_acme(&self, _id: ShortId) -> Result<(), Error> {
        Ok(())
    }

    async fn load_acmes(&self) -> Vec<crate::certs::acme::AcmeEntry> {
        Vec::new()
    }

    async fn load_dns_credentials(
        &self,
    ) -> Result<Vec<r3v3rs3_api::acme::DnsCredentialInfo>, Error> {
        // The master owns the credentials, and a worker orders no certificates.
        Ok(Vec::new())
    }

    async fn save_dns_credential(
        &self,
        _id: ShortId,
        _name: &str,
        _provider: &r3v3rs3_api::acme::DnsProvider,
    ) -> Result<(), Error> {
        Err(Error::FailedToSaveConfig)
    }

    async fn delete_dns_credential(&self, _id: ShortId) -> Result<(), Error> {
        Err(Error::FailedToSaveConfig)
    }

    async fn dns_provider_of(
        &self,
        _id: ShortId,
    ) -> Result<Option<r3v3rs3_api::acme::DnsProvider>, Error> {
        Ok(None)
    }

    async fn add_account(
        &self,
        _name: &str,
        _password: &str,
        _totp: bool,
        _role: Role,
    ) -> Result<Account, Error> {
        // The master owns the accounts, so a worker never adds one.
        Err(Error::FailedToCreateAccount)
    }

    async fn verify_account(
        &self,
        _request: LoginRequest,
    ) -> Result<r3v3rs3_api::auth::LoginResponse, Error> {
        // The master authenticates the accounts, so a worker accepts none.
        Err(Error::InvalidLoginCredentials)
    }

    async fn load_accounts(&self) -> Result<HashMap<String, Account>, Error> {
        Ok(HashMap::new())
    }

    async fn save_accounts(&self, _accounts: &HashMap<String, Account>) -> Result<(), Error> {
        Err(Error::FailedToSaveConfig)
    }

    async fn save_cdn_ranges(&self, ranges: &CdnRanges) -> Result<(), Error> {
        self.data.lock().await.cdn = Some(ranges.clone());
        Ok(())
    }

    async fn load_cdn_ranges(&self) -> Option<CdnRanges> {
        self.data.lock().await.cdn.clone()
    }

    async fn save_app_store(&self, catalog: &StoreCatalog) -> Result<(), Error> {
        self.data.lock().await.app_store = Some(catalog.clone());
        Ok(())
    }

    async fn load_app_store(&self) -> Option<StoreCatalog> {
        self.data.lock().await.app_store.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::storage::Storage;
    use r3v3rs3_api::port::Port;

    #[tokio::test]
    async fn the_state_survives_a_load_and_a_replace() {
        let storage = MemoryStorage::new(
            AppConfig::default(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        storage
            .save_ports(&[PortEntry {
                id: "p".parse().unwrap(),
                port: Port {
                    active: true,
                    name: String::new(),
                    listen: "/ip4/127.0.0.1/tcp/0".parse().unwrap(),
                    opts: Default::default(),
                },
            }])
            .await
            .unwrap();
        assert_eq!(storage.load_ports().await.len(), 1);

        storage
            .replace(Vec::new(), Vec::new(), Vec::new(), Vec::new())
            .await;
        assert!(storage.load_ports().await.is_empty());
        assert!(storage.load_accounts().await.unwrap().is_empty());
    }
}
