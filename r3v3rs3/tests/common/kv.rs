//! Checks that every [`KvStore`] must pass.

use r3v3rs3::cluster::crypto::{sealed_key_id, ClusterKey, ClusterKeys};
use r3v3rs3::cluster::data_prefix;
use r3v3rs3::cluster::rekey::rekey;
use r3v3rs3::kv::http::ApiClient;
use r3v3rs3::kv::{
    Condition, KvEvent, KvItem, KvList, KvStore, KvWatcher, Lease, Txn, TxnOutcome, WatchBatch,
    Write,
};
use r3v3rs3_api::discovery::Endpoint;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use url::Url;

#[derive(Default)]
struct MemoryData {
    /// The value and the version of each key.
    items: BTreeMap<String, (Vec<u8>, u64)>,
    /// Every change with the revision of its commit.
    history: Vec<(u64, KvEvent)>,
    /// The lease of each key that has one.
    owners: BTreeMap<String, String>,
    /// The leases that did not end. A lease ends only when it is revoked.
    leases: BTreeSet<String>,
    revision: u64,
    commits: usize,
    unavailable: bool,
}

impl MemoryData {
    fn next_revision(&mut self) -> u64 {
        self.revision += 1;
        self.revision
    }

    fn put(&mut self, key: &str, value: &[u8], revision: u64) {
        self.items
            .insert(key.to_string(), (value.to_vec(), revision));
        let item = KvItem {
            key: key.to_string(),
            value: value.to_vec(),
            version: revision,
        };
        self.history.push((revision, KvEvent::Put(item)));
    }

    fn delete(&mut self, key: &str, revision: u64) {
        self.items.remove(key);
        self.owners.remove(key);
        self.history
            .push((revision, KvEvent::Delete(key.to_string())));
    }

    fn apply(&mut self, write: &Write, revision: u64) {
        match write {
            Write::Put { key, value, lease } => {
                self.put(key, value, revision);
                match lease {
                    Some(lease) => self.owners.insert(key.clone(), lease.clone()),
                    None => self.owners.remove(key),
                };
            }
            Write::Update { key, value } => self.put(key, value, revision),
            Write::Delete(key) => self.delete(key, revision),
        }
    }

    fn ensure_lease(&self, lease: &Lease) -> anyhow::Result<()> {
        anyhow::ensure!(self.leases.contains(&lease.id), "the lease ended");
        Ok(())
    }
}

/// A [`KvStore`] in memory with keys, versions, conditional commits, watches, leases and locks,
/// for tests of the code that uses the store. A lease ends only when it is revoked.
pub struct MemoryStore {
    data: Arc<Mutex<MemoryData>>,
    /// Wakes the watchers after a commit or a change of the availability.
    changed: tokio::sync::watch::Sender<()>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self {
            data: Arc::default(),
            changed: tokio::sync::watch::Sender::new(()),
        }
    }
}

impl MemoryStore {
    /// The commits that reached the store.
    pub fn commits(&self) -> usize {
        self.data.lock().unwrap().commits
    }

    /// An unavailable store fails every call.
    pub fn set_unavailable(&self, unavailable: bool) {
        self.data.lock().unwrap().unavailable = unavailable;
        self.changed.send_replace(());
    }

    pub fn value(&self, key: &str) -> anyhow::Result<Vec<u8>> {
        let data = self.data.lock().unwrap();
        let item = data.items.get(key);
        item.map(|(value, _)| value.clone())
            .ok_or_else(|| anyhow::anyhow!("{key} is missing"))
    }

    fn data(&self) -> anyhow::Result<MutexGuard<'_, MemoryData>> {
        available(&self.data)
    }
}

fn available(data: &Mutex<MemoryData>) -> anyhow::Result<MutexGuard<'_, MemoryData>> {
    let data = data.lock().unwrap();
    anyhow::ensure!(!data.unavailable, "the store is unavailable");
    Ok(data)
}

fn holds(data: &MemoryData, condition: &Condition) -> bool {
    match condition {
        Condition::Absent(key) => !data.items.contains_key(key),
        Condition::Version(key, version) => data.items.get(key).map(|(_, v)| v) == Some(version),
        Condition::LockHeld { key, lease } => data.owners.get(key) == Some(lease),
    }
}

struct MemoryWatcher {
    data: Arc<Mutex<MemoryData>>,
    changed: tokio::sync::watch::Receiver<()>,
    prefix: String,
    /// The revision of the last reported change.
    after: u64,
}

impl MemoryWatcher {
    /// The changes under the prefix after the last reported revision.
    fn pending(&mut self) -> anyhow::Result<Option<WatchBatch>> {
        let data = available(&self.data)?;
        let events = data
            .history
            .iter()
            .filter(|(revision, event)| {
                *revision > self.after && event.key().starts_with(&self.prefix)
            })
            .map(|(_, event)| event.clone())
            .collect::<Vec<_>>();
        let revision = data.revision;
        drop(data);
        if events.is_empty() {
            return Ok(None);
        }
        self.after = revision;
        Ok(Some(WatchBatch::Events { events, revision }))
    }
}

#[async_trait::async_trait]
impl KvWatcher for MemoryWatcher {
    async fn next(&mut self) -> anyhow::Result<WatchBatch> {
        loop {
            self.changed.mark_unchanged();
            if let Some(batch) = self.pending()? {
                return Ok(batch);
            }
            self.changed.changed().await?;
        }
    }
}

#[async_trait::async_trait]
impl KvStore for MemoryStore {
    async fn list(&self, prefix: &str) -> anyhow::Result<KvList> {
        let data = self.data()?;
        let items = data
            .items
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .map(|(key, (value, version))| KvItem {
                key: key.clone(),
                value: value.clone(),
                version: *version,
            })
            .collect();
        Ok(KvList {
            items,
            revision: data.revision,
        })
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<KvItem>> {
        let data = self.data()?;
        Ok(data.items.get(key).map(|(value, version)| KvItem {
            key: key.to_string(),
            value: value.clone(),
            version: *version,
        }))
    }

    async fn commit(&self, txn: &Txn) -> anyhow::Result<TxnOutcome> {
        let mut data = self.data()?;
        data.commits += 1;
        if !txn
            .conditions
            .iter()
            .all(|condition| holds(&data, condition))
        {
            return Ok(TxnOutcome::Conflict);
        }
        let revision = data.next_revision();
        for write in &txn.writes {
            data.apply(write, revision);
        }
        drop(data);
        self.changed.send_replace(());
        Ok(TxnOutcome::Committed)
    }

    async fn grant_lease(&self, ttl: Duration) -> anyhow::Result<Lease> {
        let mut data = self.data()?;
        let id = data.next_revision().to_string();
        data.leases.insert(id.clone());
        Ok(Lease { id, ttl })
    }

    async fn keep_alive(&self, lease: &Lease) -> anyhow::Result<()> {
        self.data()?.ensure_lease(lease)
    }

    async fn revoke_lease(&self, lease: &Lease) -> anyhow::Result<()> {
        let mut data = self.data()?;
        data.leases.remove(&lease.id);
        let owned = data
            .owners
            .iter()
            .filter(|(_, owner)| **owner == lease.id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        if owned.is_empty() {
            return Ok(());
        }
        let revision = data.next_revision();
        for key in owned {
            data.delete(&key, revision);
        }
        drop(data);
        self.changed.send_replace(());
        Ok(())
    }

    async fn try_lock(&self, key: &str, value: &[u8], lease: &Lease) -> anyhow::Result<bool> {
        let mut data = self.data()?;
        data.ensure_lease(lease)?;
        if let Some(owner) = data.owners.get(key) {
            return Ok(*owner == lease.id);
        }
        if data.items.contains_key(key) {
            return Ok(false);
        }
        let revision = data.next_revision();
        let write = Write::Put {
            key: key.to_string(),
            value: value.to_vec(),
            lease: Some(lease.id.clone()),
        };
        data.apply(&write, revision);
        drop(data);
        self.changed.send_replace(());
        Ok(true)
    }

    fn watch(&self, prefix: &str, from: &KvList) -> Box<dyn KvWatcher> {
        Box::new(MemoryWatcher {
            data: self.data.clone(),
            changed: self.changed.subscribe(),
            prefix: prefix.to_string(),
            after: from.revision,
        })
    }
}

pub fn api_client(url: &Url) -> anyhow::Result<ApiClient> {
    let host = url.host_str().unwrap_or_default();
    let port = url.port().unwrap_or(80);
    let endpoint = format!("http://{host}:{port}").parse::<Endpoint>()?;
    let tls = ClientConfig::builder()
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth();
    Ok(ApiClient::new(vec![endpoint], Arc::new(tls)))
}

pub fn put(key: &str, value: &str) -> Write {
    Write::Put {
        key: key.to_string(),
        value: value.as_bytes().to_vec(),
        lease: None,
    }
}

pub async fn commit(
    store: &dyn KvStore,
    conditions: Vec<Condition>,
    writes: Vec<Write>,
) -> anyhow::Result<TxnOutcome> {
    store.commit(&Txn { conditions, writes }).await
}

/// The next batch of the watcher as `put key=value`, `delete key` and `resync key` lines.
pub async fn next_changes(watcher: &mut dyn KvWatcher) -> anyhow::Result<Vec<String>> {
    let batch = tokio::time::timeout(Duration::from_secs(10), watcher.next()).await??;
    Ok(match batch {
        WatchBatch::Events { events, .. } => events
            .into_iter()
            .map(|event| match event {
                KvEvent::Put(item) => {
                    format!("put {}={}", item.key, String::from_utf8_lossy(&item.value))
                }
                KvEvent::Delete(key) => format!("delete {key}"),
            })
            .collect(),
        WatchBatch::Resync(list) => list
            .items
            .into_iter()
            .map(|item| format!("resync {}", item.key))
            .collect(),
    })
}

/// A watcher reports the puts and the deletes under its prefix, one change at a time. Returns the
/// list that the watcher started from, and the watcher.
pub async fn check_watch_changes(
    store: &dyn KvStore,
) -> anyhow::Result<(KvList, Box<dyn KvWatcher>)> {
    commit(store, vec![], vec![put("w/a", "1")]).await?;
    let from = store.list("w/").await?;
    let mut watcher = store.watch("w/", &from);

    commit(store, vec![], vec![put("w/b", "2"), put("x/other", "3")]).await?;
    assert_eq!(next_changes(watcher.as_mut()).await?, ["put w/b=2"]);
    commit(store, vec![], vec![Write::Delete("w/a".into())]).await?;
    assert_eq!(next_changes(watcher.as_mut()).await?, ["delete w/a"]);
    Ok((from, watcher))
}

/// Rekey encrypts the values of an older key with the first key and keeps the lease of a key.
pub async fn check_rekey(store: &dyn KvStore) -> anyhow::Result<()> {
    let key = |byte: u8| ClusterKey::new(&[byte; 32]);
    let old = ClusterKeys::new(vec![key(1)?])?;
    let rotated = ClusterKeys::new(vec![key(2)?, key(1)?])?;
    let prefix = data_prefix("r3v3rs3");
    let config = format!("{prefix}state/config");
    let leader = format!("{prefix}lock/leader");
    let sealed = |key: &str, value: &str| -> anyhow::Result<Write> {
        Ok(Write::Put {
            key: key.to_string(),
            value: old.seal(key, value.as_bytes())?,
            lease: None,
        })
    };

    let writes = vec![
        sealed(&config, "config")?,
        put(&format!("{prefix}state/ports/web"), "{}"),
        sealed("other/state", "other")?,
    ];
    commit(store, vec![], writes).await?;
    let lease = store.grant_lease(Duration::from_secs(30)).await?;
    assert!(
        store
            .try_lock(&leader, &old.seal(&leader, b"node-a")?, &lease)
            .await?
    );

    let report = rekey(store, &rotated, &prefix).await?;
    assert_eq!((report.resealed, report.plain, report.current), (2, 1, 0));
    let new_only = ClusterKeys::new(vec![key(2)?])?;
    let open = |item: Option<KvItem>, key: &str| -> anyhow::Result<Vec<u8>> {
        let item = item.ok_or_else(|| anyhow::anyhow!("{key} is missing"))?;
        new_only.open(key, &item.value)
    };
    assert_eq!(open(store.get(&config).await?, &config)?, b"config");
    assert_eq!(open(store.get(&leader).await?, &leader)?, b"node-a");
    // The lease still holds the lock. A second `try_lock` would write a plain value on Consul.
    let held = vec![Condition::LockHeld {
        key: leader.clone(),
        lease: lease.id.clone(),
    }];
    let fenced = commit(store, held, vec![put(&format!("{prefix}state/cdn"), "{}")]).await?;
    assert_eq!(fenced, TxnOutcome::Committed);
    let outside = store.get("other/state").await?.map(|item| item.value);
    assert_eq!(
        outside.as_deref().and_then(sealed_key_id),
        Some(old.primary_id())
    );

    let again = rekey(store, &rotated, &prefix).await?;
    assert_eq!((again.resealed, again.current, again.plain), (0, 2, 2));
    Ok(())
}

/// A write with a stale version or on an existing key changes nothing.
pub async fn check_conditional_commits(store: &dyn KvStore) -> anyhow::Result<()> {
    let absent = || vec![Condition::Absent("c/a".into())];
    assert_eq!(
        commit(store, absent(), vec![put("c/a", "1")]).await?,
        TxnOutcome::Committed
    );
    assert_eq!(
        commit(store, absent(), vec![put("c/a", "2")]).await?,
        TxnOutcome::Conflict
    );
    let Some(first) = store.get("c/a").await? else {
        anyhow::bail!("the key c/a is missing");
    };
    assert_eq!(first.value, b"1");

    let version = |version| vec![Condition::Version("c/a".into(), version)];
    let update = vec![put("c/a", "3"), put("c/b", "3")];
    assert_eq!(
        commit(store, version(first.version), update.clone()).await?,
        TxnOutcome::Committed
    );
    assert_eq!(
        commit(store, version(first.version), vec![put("c/a", "4")]).await?,
        TxnOutcome::Conflict
    );

    let list = store.list("c/").await?;
    let values = list
        .items
        .iter()
        .map(|item| (item.key.as_str(), item.value.as_slice()))
        .collect::<Vec<_>>();
    assert_eq!(values, [("c/a", &b"3"[..]), ("c/b", &b"3"[..])]);
    assert!(list.items[0].version > first.version);
    assert!(store.get("c/missing").await?.is_none());
    Ok(())
}

/// Only one lease holds a lock. Revoking the lease deletes the lock and its fenced writes fail.
pub async fn check_locks_and_leases(store: &dyn KvStore) -> anyhow::Result<()> {
    let ttl = Duration::from_secs(30);
    let (first, second) = (store.grant_lease(ttl).await?, store.grant_lease(ttl).await?);
    store.keep_alive(&first).await?;

    assert!(store.try_lock("l/leader", b"first", &first).await?);
    assert!(store.try_lock("l/leader", b"first", &first).await?);
    assert!(!store.try_lock("l/leader", b"second", &second).await?);

    let held = |lease: &str| {
        vec![Condition::LockHeld {
            key: "l/leader".into(),
            lease: lease.to_string(),
        }]
    };
    let fenced = commit(store, held(&second.id), vec![put("l/state", "second")]).await?;
    assert_eq!(fenced, TxnOutcome::Conflict);
    let fenced = commit(store, held(&first.id), vec![put("l/state", "first")]).await?;
    assert_eq!(fenced, TxnOutcome::Committed);

    store.revoke_lease(&first).await?;
    assert!(store.get("l/leader").await?.is_none());
    assert!(store.keep_alive(&first).await.is_err());
    let fenced = commit(store, held(&first.id), vec![put("l/state", "stale")]).await?;
    assert_eq!(fenced, TxnOutcome::Conflict);
    assert!(store.try_lock("l/leader", b"second", &second).await?);
    Ok(())
}
