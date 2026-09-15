//! Clients of the HTTP APIs of etcd and Consul. Service discovery reads its keys through them, and
//! [`KvStore`] reads and writes shared state with transactions, leases, locks and watches.

pub mod consul;
mod consul_store;
pub mod etcd;
mod etcd_store;
pub mod http;

pub use consul_store::ConsulStore;

use std::time::Duration;

/// A key, its value and the version of its last change: the etcd mod revision or the Consul
/// modify index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvItem {
    pub key: String,
    pub value: Vec<u8>,
    pub version: u64,
}

/// The keys under a prefix and the revision of the read. A watch starts after this revision.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KvList {
    pub items: Vec<KvItem>,
    pub revision: u64,
}

/// A grant with a TTL: an etcd lease or a Consul session. The store deletes the keys of the lease
/// when the lease ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub id: String,
    pub ttl: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// The key does not exist.
    Absent(String),
    /// The last change of the key has this version.
    Version(String, u64),
    /// The lease holds the lock key.
    LockHeld { key: String, lease: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    /// `lease` attaches the key to a lease.
    Put {
        key: String,
        value: Vec<u8>,
        lease: Option<String>,
    },
    /// Changes the value and keeps the lease of the key.
    Update {
        key: String,
        value: Vec<u8>,
    },
    Delete(String),
}

/// Writes that the store applies together, only when every condition holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Txn {
    pub conditions: Vec<Condition>,
    pub writes: Vec<Write>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnOutcome {
    Committed,
    /// A condition did not hold, so the store applied no write.
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvEvent {
    Put(KvItem),
    Delete(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchBatch {
    /// The changes after the previous batch. `revision` is the revision of the last change.
    Events { events: Vec<KvEvent>, revision: u64 },
    /// The store lost the change history, for example after an etcd compaction or a Consul index
    /// that went back. The list is the current state.
    Resync(KvList),
}

#[async_trait::async_trait]
pub trait KvWatcher: Send {
    /// Waits for the next changes under the prefix. After an error, the next call connects again
    /// and continues after the last returned change.
    async fn next(&mut self) -> anyhow::Result<WatchBatch>;
}

#[async_trait::async_trait]
pub trait KvStore: Send + Sync {
    /// The keys that start with `prefix`.
    async fn list(&self, prefix: &str) -> anyhow::Result<KvList>;

    async fn get(&self, key: &str) -> anyhow::Result<Option<KvItem>>;

    async fn commit(&self, txn: &Txn) -> anyhow::Result<TxnOutcome>;

    async fn grant_lease(&self, ttl: Duration) -> anyhow::Result<Lease>;

    /// Renews the lease. An error means that the lease ended or that the store did not answer.
    async fn keep_alive(&self, lease: &Lease) -> anyhow::Result<()>;

    async fn revoke_lease(&self, lease: &Lease) -> anyhow::Result<()>;

    /// Takes the lock key for the lease. True when the lease holds the lock after the call.
    async fn try_lock(&self, key: &str, value: &[u8], lease: &Lease) -> anyhow::Result<bool>;

    /// Watches the keys that start with `prefix`, after the revision of `from`.
    fn watch(&self, prefix: &str, from: &KvList) -> Box<dyn KvWatcher>;
}
