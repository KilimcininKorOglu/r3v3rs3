//! [`Storage`] in etcd or Consul. The storage remembers the version and the digest of each key that
//! it read or wrote, and writes only the keys whose value changed. Each write needs the remembered
//! version, so a change of another node in between is a conflict.

use super::crypto::ClusterKeys;
use super::layout::{last_segment, Layout};
use super::{key_file, store};
use crate::cdn::CdnRanges;
use crate::certs::acme::{AcmeAccount, AcmeEntry};
use crate::certs::alpn::TlsAlpnChallenge;
use crate::certs::challenges::ServedChallenges;
use crate::certs::Cert;
use crate::config::{account, storage::Storage};
use crate::kv::{Condition, KvItem, KvStore, Txn, TxnOutcome, Write};
use crate::proxy::http::cache_share::SharedCacheStore;
use crate::proxy::http::rate_share::RateCountExchange;
use crate::sessions::SessionBackend;
use anyhow::Context as _;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::{Account, LoginRequest, LoginResponse};
use r3v3rs3_api::cert::CertKind;
use r3v3rs3_api::cluster::ClusterConfig;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::port::{Port, PortEntry};
use r3v3rs3_api::proxy::{Proxy, ProxyEntry};
use serde::de::DeserializeOwned;
use serde_derive::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::{error, warn};

/// The changes of one transaction. Each change has a condition, and a Consul transaction has at
/// most 64 operations.
const MAX_CHANGES: usize = 32;
/// The value of the schema key.
const SCHEMA: &[u8] = b"1";
/// The longest wait of the leader for the nodes to serve new ACME challenges.
const CHALLENGE_WAIT: Duration = Duration::from_secs(10);
/// The interval of the checks for the acks of the nodes.
const ACK_INTERVAL: Duration = Duration::from_millis(200);

type Digest32 = [u8; 32];

#[derive(Debug, Clone, Copy)]
struct Known {
    version: u64,
    digest: Digest32,
}

enum Change {
    /// The key and the unencrypted value.
    Put(String, Vec<u8>),
    Delete(String),
}

impl Change {
    fn key(&self) -> &str {
        match self {
            Self::Put(key, _) | Self::Delete(key) => key,
        }
    }
}

/// A list entry with its position in the list.
#[derive(Serialize, Deserialize)]
struct Positioned<T> {
    position: usize,
    value: T,
}

#[derive(Serialize, Deserialize)]
struct StoredCert {
    kind: CertKind,
    chain: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key: Option<String>,
}

pub struct KvStorage {
    store: Arc<dyn KvStore>,
    keys: ClusterKeys,
    layout: Layout,
    /// The settings of `config.toml`. The settings that only the file sets come from here.
    local: AppConfig,
    known: Mutex<HashMap<String, Known>>,
    /// False while the node lost the store. The storage then rejects changes.
    healthy: AtomicBool,
}

impl KvStorage {
    pub fn new(store: Arc<dyn KvStore>, keys: ClusterKeys, local: AppConfig) -> Self {
        Self {
            store,
            keys,
            layout: Layout::new(&local.cluster.prefix),
            local,
            known: Mutex::default(),
            healthy: AtomicBool::new(true),
        }
    }

    pub fn cluster_config(&self) -> &ClusterConfig {
        &self.local.cluster
    }

    pub fn set_healthy(&self, healthy: bool) {
        self.healthy.store(healthy, Ordering::Relaxed);
    }

    /// Connects to the store that `config.toml` names, without a check of the data.
    pub async fn connect(local: AppConfig) -> anyhow::Result<Self> {
        store::validate(&local.cluster)?;
        let keys = key_file::load_keys(&local.cluster.encryption_key_files).await?;
        let store = store::connect(&local.cluster).await?;
        Ok(Self::new(store, keys, local))
    }

    /// Connects to the store and checks that `cluster import` wrote the data. Both steps finish in
    /// `cluster.startup_timeout`.
    pub async fn open(local: AppConfig) -> anyhow::Result<Self> {
        let timeout = local.cluster.startup_timeout;
        let open = async {
            let storage = Self::connect(local).await?;
            anyhow::ensure!(
                storage.is_imported().await?,
                "the cluster store has no data below {}. Run `r3v3rs3 cluster import` first.",
                storage.layout.data()
            );
            Ok(storage)
        };
        match tokio::time::timeout(timeout, open).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!("the cluster store did not answer in cluster.startup_timeout"),
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn store(&self) -> &dyn KvStore {
        self.store.as_ref()
    }

    pub async fn is_imported(&self) -> anyhow::Result<bool> {
        Ok(self.store.get(&self.layout.schema()).await?.is_some())
    }

    /// Writes the schema key, which marks the end of an import.
    pub async fn write_schema(&self) -> anyhow::Result<()> {
        let key = self.layout.schema();
        let txn = Txn {
            conditions: vec![Condition::Absent(key.clone())],
            writes: vec![Write::Put {
                key,
                value: SCHEMA.to_vec(),
                lease: None,
            }],
        };
        match self.store.commit(&txn).await? {
            TxnOutcome::Committed => Ok(()),
            TxnOutcome::Conflict => anyhow::bail!("another import wrote the cluster data"),
        }
    }

    /// Saves an account with its password hash.
    pub async fn put_account(&self, name: &str, account: &Account) -> Result<(), Error> {
        self.apply(vec![json_change(self.layout.account(name), account)?])
            .await
    }

    /// Writes a key without a condition. Only one node writes such a key, so the write cannot
    /// replace the change of another node.
    pub async fn put_unconditional(
        &self,
        key: String,
        plaintext: &[u8],
        lease: Option<String>,
    ) -> anyhow::Result<()> {
        let value = self.encode(&key, plaintext)?;
        self.commit_unconditional(vec![Write::Put { key, value, lease }])
            .await
    }

    /// Deletes keys without a condition.
    pub async fn delete_unconditional(&self, keys: Vec<String>) -> anyhow::Result<()> {
        let deletes = keys.into_iter().map(Write::Delete).collect();
        self.commit_unconditional(deletes).await
    }

    /// Deletes the keys below `prefix` whose items `stale` selects, without a condition.
    pub async fn delete_stale(
        &self,
        prefix: &str,
        stale: impl Fn(&KvItem) -> bool,
    ) -> anyhow::Result<()> {
        let list = self.store.list(prefix).await?;
        let keys = list
            .items
            .into_iter()
            .filter(|item| stale(item))
            .map(|item| item.key)
            .collect();
        self.delete_unconditional(keys).await
    }

    async fn commit_unconditional(&self, writes: Vec<Write>) -> anyhow::Result<()> {
        for chunk in writes.chunks(MAX_CHANGES) {
            let txn = Txn {
                conditions: Vec::new(),
                writes: chunk.to_vec(),
            };
            self.store.commit(&txn).await?;
        }
        Ok(())
    }

    /// The value to write under a key: sealed, or plain for the keys that the cluster does not
    /// encrypt.
    pub fn encode(&self, key: &str, plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        if self.layout.is_plain(key) {
            return Ok(plaintext.to_vec());
        }
        self.keys.seal(key, plaintext)
    }

    /// An unencrypted value under a key that the cluster encrypts is an error, so a writer without
    /// the encryption key cannot set it.
    /// The JSON value of an item.
    pub fn decode_json<T: DeserializeOwned>(&self, item: &KvItem) -> anyhow::Result<T> {
        Ok(serde_json::from_slice(&self.decode(item)?)?)
    }

    pub fn decode(&self, item: &KvItem) -> anyhow::Result<Vec<u8>> {
        if self.layout.is_plain(&item.key) {
            return Ok(item.value.clone());
        }
        self.keys.open(&item.key, &item.value)
    }

    /// Reads one key and remembers its version and digest.
    async fn read_key(&self, key: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let item = self.store.get(key).await?;
        let mut known = self.known.lock().await;
        let Some(item) = item else {
            known.remove(key);
            return Ok(None);
        };
        let plaintext = self.decode(&item)?;
        known.insert(key.to_string(), known_value(&item, &plaintext));
        Ok(Some(plaintext))
    }

    /// Reads the keys below `prefix` and remembers their versions and digests. A value that does
    /// not open is left out.
    async fn read_prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
        let list = self.store.list(prefix).await?;
        let mut known = self.known.lock().await;
        known.retain(|key, _| !key.starts_with(prefix));
        let mut values = Vec::with_capacity(list.items.len());
        for item in list.items {
            match self.decode(&item) {
                Ok(plaintext) => {
                    known.insert(item.key.clone(), known_value(&item, &plaintext));
                    values.push((item.key, plaintext));
                }
                Err(err) => error!(key = item.key, "failed to load: {err:#}"),
            }
        }
        Ok(values)
    }

    async fn load_value<T: DeserializeOwned>(&self, key: &str) -> anyhow::Result<Option<T>> {
        let Some(bytes) = self.read_key(key).await? else {
            return Ok(None);
        };
        let value = serde_json::from_slice(&bytes).with_context(|| format!("invalid {key}"))?;
        Ok(Some(value))
    }

    /// The values below `prefix` with the last segment of their keys.
    async fn load_values<T: DeserializeOwned>(&self, prefix: &str) -> Vec<(String, T)> {
        let values = match self.read_prefix(prefix).await {
            Ok(values) => values,
            Err(err) => {
                error!(prefix, "failed to load: {err:#}");
                return Vec::new();
            }
        };
        values
            .into_iter()
            .filter_map(|(key, bytes)| match serde_json::from_slice(&bytes) {
                Ok(value) => Some((last_segment(&key).to_string(), value)),
                Err(err) => {
                    error!(key, "failed to load: {err}");
                    None
                }
            })
            .collect()
    }

    /// Loads a list that [`Self::save_list`] saved, in the order of the saved list.
    async fn load_list<T, E>(&self, prefix: &str) -> Vec<E>
    where
        T: DeserializeOwned,
        E: From<(ShortId, T)>,
    {
        let mut values = self
            .load_values::<Positioned<T>>(prefix)
            .await
            .into_iter()
            .filter_map(|(name, value)| match name.parse::<ShortId>() {
                Ok(id) => Some((value.position, id, value.value)),
                Err(_) => {
                    error!(prefix, name, "invalid id");
                    None
                }
            })
            .collect::<Vec<_>>();
        values.sort_by_key(|(position, _, _)| *position);
        values
            .into_iter()
            .map(|(_, id, value)| E::from((id, value)))
            .collect()
    }

    /// Saves a list below `prefix` with one key for each entry, and deletes the keys that the
    /// list does not have.
    async fn save_list<T: serde::Serialize + Sync>(
        &self,
        prefix: String,
        entries: Vec<(ShortId, &T)>,
    ) -> Result<(), Error> {
        let puts = entries
            .into_iter()
            .enumerate()
            .map(|(position, (id, value))| {
                json_change(format!("{prefix}{id}"), &Positioned { position, value })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.replace_prefix(&prefix, puts).await
    }

    /// Writes the puts and deletes the remembered keys below `prefix` that the puts do not have.
    async fn replace_prefix(&self, prefix: &str, puts: Vec<Change>) -> Result<(), Error> {
        let keys = puts.iter().map(Change::key).collect::<HashSet<_>>();
        let stale = self
            .known
            .lock()
            .await
            .keys()
            .filter(|key| key.starts_with(prefix) && !keys.contains(key.as_str()))
            .map(|key| Change::Delete(key.clone()))
            .collect::<Vec<_>>();
        self.apply(puts.into_iter().chain(stale).collect()).await
    }

    /// Waits until every node that is present acknowledged the challenges. False when the time
    /// ends first.
    pub async fn wait_for_nodes(
        &self,
        challenges: &ServedChallenges,
        timeout: Duration,
    ) -> anyhow::Result<bool> {
        let digest = challenges.digest();
        let deadline = Instant::now() + timeout;
        while !self.all_nodes_acked(&digest).await? {
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(ACK_INTERVAL).await;
        }
        Ok(true)
    }

    async fn all_nodes_acked(&self, digest: &str) -> anyhow::Result<bool> {
        let nodes = self.store.list(&self.layout.nodes()).await?;
        for node in nodes.items {
            let ack = self.store.get(&self.layout.ack_of(&node.key)).await?;
            if ack.is_none_or(|ack| ack.value != digest.as_bytes()) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Writes the changes whose value differs from the remembered value.
    async fn apply(&self, changes: Vec<Change>) -> Result<(), Error> {
        let mut known = self.known.lock().await;
        let changes = changes
            .into_iter()
            .filter(|change| is_needed(&known, change))
            .collect::<Vec<_>>();
        for chunk in changes.chunks(MAX_CHANGES) {
            let txn = self.txn(&known, chunk).map_err(|err| {
                error!("failed to encrypt the cluster data: {err:#}");
                Error::FailedToSaveConfig
            })?;
            let outcome = self.store.commit(&txn).await.map_err(unavailable)?;
            self.refresh(&mut known, chunk).await.map_err(unavailable)?;
            if outcome == TxnOutcome::Conflict {
                return Err(Error::ClusterWriteConflict);
            }
        }
        Ok(())
    }

    fn txn(&self, known: &HashMap<String, Known>, changes: &[Change]) -> anyhow::Result<Txn> {
        let mut txn = Txn::default();
        for change in changes {
            let key = change.key().to_string();
            txn.conditions.push(match known.get(&key) {
                Some(known) => Condition::Version(key.clone(), known.version),
                None => Condition::Absent(key.clone()),
            });
            txn.writes.push(match change {
                Change::Put(_, plaintext) => Write::Put {
                    value: self.encode(&key, plaintext)?,
                    key,
                    lease: None,
                },
                Change::Delete(_) => Write::Delete(key),
            });
        }
        Ok(txn)
    }

    /// Reads the versions of the changed keys again, after a commit or a conflict.
    async fn refresh(
        &self,
        known: &mut HashMap<String, Known>,
        changes: &[Change],
    ) -> anyhow::Result<()> {
        for change in changes {
            let key = change.key();
            let item = self.store.get(key).await?;
            match item.map(|item| self.decode(&item).map(|value| known_value(&item, &value))) {
                Some(Ok(value)) => known.insert(key.to_string(), value),
                Some(Err(err)) => {
                    error!(key, "failed to load: {err:#}");
                    known.remove(key)
                }
                None => known.remove(key),
            };
        }
        Ok(())
    }
}

fn digest(value: &[u8]) -> Digest32 {
    Sha256::digest(value).into()
}

fn known_value(item: &KvItem, plaintext: &[u8]) -> Known {
    Known {
        version: item.version,
        digest: digest(plaintext),
    }
}

fn is_needed(known: &HashMap<String, Known>, change: &Change) -> bool {
    match change {
        Change::Put(key, plaintext) => known
            .get(key)
            .is_none_or(|known| known.digest != digest(plaintext)),
        Change::Delete(key) => known.contains_key(key),
    }
}

fn json_change<T: serde::Serialize + ?Sized>(key: String, value: &T) -> Result<Change, Error> {
    match serde_json::to_vec(value) {
        Ok(bytes) => Ok(Change::Put(key, bytes)),
        Err(err) => {
            error!(key, "failed to encode: {err}");
            Err(Error::FailedToSaveConfig)
        }
    }
}

pub(super) fn unavailable(err: anyhow::Error) -> Error {
    error!("the cluster store failed: {err:#}");
    Error::ClusterUnavailable
}

fn pem_text(key: &str, pem: &[u8]) -> Result<String, Error> {
    String::from_utf8(pem.to_vec()).map_err(|_| {
        error!(key, "the certificate is not PEM text");
        Error::FailedToSaveConfig
    })
}

fn stored_cert(cert: StoredCert) -> Result<Arc<Cert>, Error> {
    let key = cert.key.map(String::into_bytes);
    Cert::new(cert.kind, cert.chain.into_bytes(), key).map(Arc::new)
}

fn challenge_changes(layout: &Layout, challenges: &ServedChallenges) -> Vec<Change> {
    let http = challenges.http.iter().map(|(token, authorization)| {
        let value = authorization.clone().into_bytes();
        Change::Put(layout.http_challenge(token), value)
    });
    let tls_alpn = challenges.tls_alpn.iter().map(|challenge| {
        let value = hex::encode(challenge.digest).into_bytes();
        Change::Put(layout.tls_alpn_challenge(&challenge.domain), value)
    });
    http.chain(tls_alpn).collect()
}

fn read_challenge(
    layout: &Layout,
    challenges: &mut ServedChallenges,
    key: &str,
    value: Vec<u8>,
) -> anyhow::Result<()> {
    if let Some(token) = key.strip_prefix(&layout.http_challenges()) {
        let token = String::from_utf8(hex::decode(token)?)?;
        challenges.http.insert(token, String::from_utf8(value)?);
        return Ok(());
    }
    let domain = key
        .strip_prefix(&layout.tls_alpn_challenges())
        .context("not a challenge key")?;
    let digest = <[u8; 32]>::try_from(hex::decode(value)?)
        .map_err(|_| anyhow::anyhow!("the digest does not have 32 bytes"))?;
    challenges.tls_alpn.push(TlsAlpnChallenge {
        domain: String::from_utf8(hex::decode(domain)?)?,
        digest,
    });
    Ok(())
}

#[async_trait::async_trait]
impl Storage for KvStorage {
    async fn ensure_writable(&self) -> Result<(), Error> {
        if self.healthy.load(Ordering::Relaxed) {
            return Ok(());
        }
        Err(Error::ClusterUnavailable)
    }

    async fn save_app_config(&self, config: &AppConfig) -> Result<(), Error> {
        let mut shared = config.clone();
        shared.keep_file_only(&AppConfig::default());
        self.apply(vec![json_change(self.layout.config(), &shared)?])
            .await
    }

    async fn load_app_config(&self) -> AppConfig {
        let key = self.layout.config();
        match self.load_value::<AppConfig>(&key).await {
            Ok(Some(mut config)) => {
                config.keep_file_only(&self.local);
                config
            }
            Ok(None) => self.local.clone(),
            Err(err) => {
                error!(key, "failed to load: {err:#}");
                self.local.clone()
            }
        }
    }

    async fn save_ports(&self, entries: &[PortEntry]) -> Result<(), Error> {
        let ports = entries
            .iter()
            .map(|entry| (entry.id, &entry.port))
            .collect();
        self.save_list(self.layout.ports(), ports).await
    }

    async fn load_ports(&self) -> Vec<PortEntry> {
        self.load_list::<Port, _>(&self.layout.ports()).await
    }

    async fn load_proxies(&self) -> Vec<ProxyEntry> {
        self.load_list::<Proxy, _>(&self.layout.proxies()).await
    }

    async fn save_proxies(&self, proxies: &[ProxyEntry]) -> Result<(), Error> {
        let proxies = proxies
            .iter()
            .map(|entry| (entry.id, &entry.proxy))
            .collect();
        self.save_list(self.layout.proxies(), proxies).await
    }

    async fn save_cert(&self, cert: &Cert) -> Result<(), Error> {
        let key = self.layout.cert(cert.kind, cert.id());
        let stored = StoredCert {
            kind: cert.kind,
            chain: pem_text(&key, &cert.pem_chain)?,
            key: cert
                .pem_key
                .as_deref()
                .map(|pem| pem_text(&key, pem))
                .transpose()?,
        };
        self.apply(vec![json_change(key, &stored)?]).await
    }

    async fn save_acme(&self, acme: &AcmeEntry) -> Result<(), Error> {
        let (id, account): (ShortId, AcmeAccount) = acme.clone().into();
        self.apply(vec![json_change(self.layout.acme(id), &account)?])
            .await
    }

    async fn delete_acme(&self, id: ShortId) -> Result<(), Error> {
        self.apply(vec![Change::Delete(self.layout.acme(id))]).await
    }

    async fn delete_cert(&self, id: ShortId) -> Result<(), Error> {
        let certs = self.layout.certs();
        let id = id.to_string();
        let deletes = self
            .known
            .lock()
            .await
            .keys()
            .filter(|key| key.starts_with(&certs) && last_segment(key) == id)
            .map(|key| Change::Delete(key.clone()))
            .collect();
        self.apply(deletes).await
    }

    async fn load_acmes(&self) -> Vec<AcmeEntry> {
        let accounts = self.load_values::<AcmeAccount>(&self.layout.acmes()).await;
        accounts
            .into_iter()
            .filter_map(|(name, account)| {
                let id = name.parse::<ShortId>().ok()?;
                Some(AcmeEntry::from((id, account)))
            })
            .collect()
    }

    async fn load_certs(&self) -> Vec<Arc<Cert>> {
        let stored = self.load_values::<StoredCert>(&self.layout.certs()).await;
        stored
            .into_iter()
            .filter_map(|(name, cert)| match stored_cert(cert) {
                Ok(cert) => Some(cert),
                Err(err) => {
                    error!(name, "failed to load the certificate: {err}");
                    None
                }
            })
            .collect()
    }

    async fn add_account(&self, name: &str, password: &str, totp: bool) -> Result<Account, Error> {
        let account =
            account::new_account(password, totp).map_err(|_| Error::FailedToCreateAccount)?;
        self.put_account(name, &account).await?;
        Ok(account)
    }

    async fn verify_account(&self, request: LoginRequest) -> Result<LoginResponse, Error> {
        let key = self.layout.account(&request.username);
        let account = self.load_value::<Account>(&key).await.map_err(|err| {
            error!("failed to load the account: {err:#}");
            Error::ClusterUnavailable
        })?;
        account::verify(account.as_ref(), request)
    }

    async fn save_cdn_ranges(&self, ranges: &CdnRanges) -> Result<(), Error> {
        self.apply(vec![json_change(self.layout.cdn(), ranges)?])
            .await
    }

    async fn load_cdn_ranges(&self) -> Option<CdnRanges> {
        let key = self.layout.cdn();
        self.load_value(&key).await.unwrap_or_else(|err| {
            error!(key, "failed to load: {err:#}");
            None
        })
    }

    async fn save_challenges(&self, challenges: &ServedChallenges) -> Result<(), Error> {
        let prefix = self.layout.challenges();
        // An earlier leader can leave keys in the store, so the storage reads them first.
        self.read_prefix(&prefix).await.map_err(unavailable)?;
        let puts = challenge_changes(&self.layout, challenges);
        self.replace_prefix(&prefix, puts).await
    }

    async fn load_challenges(&self) -> Option<ServedChallenges> {
        let values = match self.read_prefix(&self.layout.challenges()).await {
            Ok(values) => values,
            Err(err) => {
                error!("failed to load the ACME challenges: {err:#}");
                return None;
            }
        };
        let mut challenges = ServedChallenges::default();
        for (key, value) in values {
            if let Err(err) = read_challenge(&self.layout, &mut challenges, &key, value) {
                error!(key, "invalid ACME challenge: {err:#}");
            }
        }
        Some(challenges)
    }

    async fn ack_challenges(&self, challenges: &ServedChallenges) -> Result<(), Error> {
        let key = self.layout.ack(&self.local.cluster.node_name);
        self.put_unconditional(key, challenges.digest().as_bytes(), None)
            .await
            .map_err(unavailable)
    }

    async fn wait_for_challenges(&self, challenges: &ServedChallenges) {
        if challenges.is_empty() {
            return;
        }
        match self.wait_for_nodes(challenges, CHALLENGE_WAIT).await {
            Ok(true) => {}
            Ok(false) => {
                warn!("not every node serves the ACME challenges after {CHALLENGE_WAIT:?}")
            }
            Err(err) => error!("failed to wait for the nodes to serve the challenges: {err:#}"),
        }
    }

    fn session_backend(self: Arc<Self>) -> Arc<dyn SessionBackend> {
        self
    }

    fn rate_count_exchange(self: Arc<Self>) -> Option<Arc<dyn RateCountExchange>> {
        Some(self)
    }

    fn shared_cache_store(self: Arc<Self>) -> Option<Arc<dyn SharedCacheStore>> {
        if self.local.cluster.share_cache {
            Some(self)
        } else {
            None
        }
    }

    async fn purge_shared_cache(&self, proxy: ShortId, at: u64) -> Result<(), Error> {
        self.purge_cache(proxy, at).await.map_err(unavailable)
    }

    async fn load_cache_purges(&self) -> HashMap<ShortId, u64> {
        self.cache_purges().await.unwrap_or_else(|err| {
            error!("failed to load the cache purges: {err:#}");
            HashMap::new()
        })
    }

    async fn remove_expired_shared_responses(&self) {
        if !self.local.cluster.share_cache {
            return;
        }
        if let Err(err) = self.remove_expired_responses(crate::clock::unix_ms()).await {
            error!("failed to remove the expired shared responses: {err:#}");
        }
    }
}
