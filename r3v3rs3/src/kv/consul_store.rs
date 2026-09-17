//! [`KvStore`] for Consul. A lease is a session with the `delete` behavior, so Consul deletes the
//! keys that the session locks when the session ends.

use super::consul::{self, ConsulClient, KeyValue, WAIT};
use super::http::read_json;
use super::{
    Condition, KvEvent, KvItem, KvList, KvStore, KvWatcher, Lease, Txn, TxnOutcome, WatchBatch,
    Write,
};
use anyhow::{Context as _, bail};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use hyper::StatusCode;
use serde_derive::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

/// The most operations of one Consul transaction.
const MAX_TXN_OPERATIONS: usize = 64;
/// The shortest session TTL that Consul accepts.
const MIN_SESSION_TTL: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct SessionCreated {
    #[serde(rename = "ID")]
    id: String,
}

#[derive(Clone)]
pub struct ConsulStore {
    client: ConsulClient,
    datacenter: String,
}

impl ConsulStore {
    /// An empty `datacenter` uses the datacenter of the agent.
    pub fn new(client: ConsulClient, datacenter: &str) -> Self {
        Self {
            client,
            datacenter: datacenter.trim().to_string(),
        }
    }

    /// The path of a key. A key that ends with `/` is a prefix.
    fn kv_path(&self, key: &str, query: &[(&str, &str)]) -> String {
        let segments = ["v1", "kv"]
            .into_iter()
            .chain(key.split('/'))
            .collect::<Vec<_>>();
        consul::path(&self.datacenter, &segments, query)
    }

    fn path(&self, segments: &[&str]) -> String {
        consul::path(&self.datacenter, segments, &[])
    }

    async fn session_action(&self, action: &str, lease: &Lease) -> anyhow::Result<()> {
        let path = self.path(&["v1", "session", action, &lease.id]);
        self.client
            .put(&path, None, &[])
            .await
            .with_context(|| format!("the session {} failed to {action}", lease.id))?;
        Ok(())
    }

    async fn send_txn(&self, operations: &[Value]) -> anyhow::Result<TxnOutcome> {
        if operations.len() > MAX_TXN_OPERATIONS {
            bail!(
                "a Consul transaction has at most {MAX_TXN_OPERATIONS} operations, not {}",
                operations.len()
            );
        }
        let path = self.path(&["v1", "txn"]);
        let body = json!(operations);
        let response = self
            .client
            .put(&path, Some(&body), &[StatusCode::CONFLICT])
            .await?;
        Ok(if response.status() == StatusCode::CONFLICT {
            TxnOutcome::Conflict
        } else {
            TxnOutcome::Committed
        })
    }
}

fn item(kv: KeyValue) -> anyhow::Result<KvItem> {
    let value = match kv.value {
        Some(value) => BASE64_STANDARD
            .decode(value)
            .with_context(|| format!("invalid Consul value of {}", kv.key))?,
        None => Vec::new(),
    };
    Ok(KvItem {
        key: kv.key,
        value,
        version: kv.modify_index,
    })
}

fn kv_list(kvs: Vec<KeyValue>, revision: u64) -> anyhow::Result<KvList> {
    let items = kvs.into_iter().map(item).collect::<anyhow::Result<_>>()?;
    Ok(KvList { items, revision })
}

fn condition_operation(condition: &Condition) -> Value {
    let kv = match condition {
        Condition::Absent(key) => json!({"Verb": "check-not-exists", "Key": key}),
        Condition::Version(key, version) => {
            json!({"Verb": "check-index", "Key": key, "Index": version})
        }
        Condition::LockHeld { key, lease } => {
            json!({"Verb": "check-session", "Key": key, "Session": lease})
        }
    };
    json!({ "KV": kv })
}

fn write_operation(write: &Write) -> Value {
    let kv = match write {
        Write::Put {
            key,
            value,
            lease: None,
        } => json!({"Verb": "set", "Key": key, "Value": BASE64_STANDARD.encode(value)}),
        Write::Put {
            key,
            value,
            lease: Some(lease),
        } => {
            json!({"Verb": "lock", "Key": key, "Value": BASE64_STANDARD.encode(value), "Session": lease})
        }
        // A `set` keeps the session that locks the key.
        Write::Update { key, value } => {
            json!({"Verb": "set", "Key": key, "Value": BASE64_STANDARD.encode(value)})
        }
        Write::Delete(key) => json!({"Verb": "delete", "Key": key}),
    };
    json!({ "KV": kv })
}

fn versions(items: &[KvItem]) -> BTreeMap<String, u64> {
    items
        .iter()
        .map(|item| (item.key.clone(), item.version))
        .collect()
}

#[async_trait::async_trait]
impl KvStore for ConsulStore {
    async fn list(&self, prefix: &str) -> anyhow::Result<KvList> {
        let path = self.kv_path(prefix, &[("recurse", "true")]);
        let (kvs, index) = self.client.list(&path).await?;
        kv_list(kvs, index)
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<KvItem>> {
        let (kvs, _) = self.client.list(&self.kv_path(key, &[])).await?;
        kvs.into_iter()
            .find(|kv| kv.key == key)
            .map(item)
            .transpose()
    }

    async fn commit(&self, txn: &Txn) -> anyhow::Result<TxnOutcome> {
        let operations = txn
            .conditions
            .iter()
            .map(condition_operation)
            .chain(txn.writes.iter().map(write_operation))
            .collect::<Vec<_>>();
        self.send_txn(&operations).await
    }

    async fn grant_lease(&self, ttl: Duration) -> anyhow::Result<Lease> {
        if ttl < MIN_SESSION_TTL {
            bail!("a Consul session needs a TTL of at least 10 seconds");
        }
        let body = json!({
            "Name": "r3v3rs3",
            "TTL": format!("{}s", ttl.as_secs()),
            "Behavior": "delete",
            "LockDelay": "1s",
        });
        let path = self.path(&["v1", "session", "create"]);
        let response = self.client.put(&path, Some(&body), &[]).await?;
        let created: SessionCreated = read_json(response).await?;
        Ok(Lease {
            id: created.id,
            ttl,
        })
    }

    async fn keep_alive(&self, lease: &Lease) -> anyhow::Result<()> {
        // Consul answers 404 for a session that ended.
        self.session_action("renew", lease).await
    }

    async fn revoke_lease(&self, lease: &Lease) -> anyhow::Result<()> {
        self.session_action("destroy", lease).await
    }

    async fn try_lock(&self, key: &str, value: &[u8], lease: &Lease) -> anyhow::Result<bool> {
        let lock = Write::Put {
            key: key.to_string(),
            value: value.to_vec(),
            lease: Some(lease.id.clone()),
        };
        Ok(self.send_txn(&[write_operation(&lock)]).await? == TxnOutcome::Committed)
    }

    fn watch(&self, prefix: &str, from: &KvList) -> Box<dyn KvWatcher> {
        Box::new(ConsulWatcher {
            store: self.clone(),
            prefix: prefix.to_string(),
            index: from.revision,
            known: versions(&from.items),
        })
    }
}

/// Follows the keys under a prefix with blocking queries. Consul returns every key, so the watcher
/// compares them with the keys of the previous read.
struct ConsulWatcher {
    store: ConsulStore,
    prefix: String,
    index: u64,
    /// The version of each key of the previous read.
    known: BTreeMap<String, u64>,
}

#[async_trait::async_trait]
impl KvWatcher for ConsulWatcher {
    async fn next(&mut self) -> anyhow::Result<WatchBatch> {
        loop {
            let list = self.read().await?;
            // An index that goes back means a restored or a new Consul cluster.
            if list.revision < self.index {
                self.remember(&list);
                return Ok(WatchBatch::Resync(list));
            }
            let events = self.changes(&list);
            self.remember(&list);
            if !events.is_empty() {
                return Ok(WatchBatch::Events {
                    events,
                    revision: list.revision,
                });
            }
        }
    }
}

impl ConsulWatcher {
    async fn read(&self) -> anyhow::Result<KvList> {
        let index = self.index.to_string();
        let query = [
            ("recurse", "true"),
            ("index", index.as_str()),
            ("wait", WAIT),
        ];
        let path = self.store.kv_path(&self.prefix, &query);
        let (kvs, index) = self.store.client.wait_list(&path).await?;
        kv_list(kvs, index)
    }

    /// The puts of the new and the changed keys, and the deletes of the keys that are gone.
    fn changes(&self, list: &KvList) -> Vec<KvEvent> {
        let current = versions(&list.items);
        let puts = list
            .items
            .iter()
            .filter(|item| self.known.get(&item.key) != Some(&item.version))
            .cloned()
            .map(KvEvent::Put);
        let deletes = self
            .known
            .keys()
            .filter(|key| !current.contains_key(*key))
            .cloned()
            .map(KvEvent::Delete);
        puts.chain(deletes).collect()
    }

    fn remember(&mut self, list: &KvList) {
        self.index = list.revision;
        self.known = versions(&list.items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_use_the_transaction_verbs() {
        assert_eq!(
            condition_operation(&Condition::Version("k".into(), 9)),
            json!({"KV": {"Verb": "check-index", "Key": "k", "Index": 9}})
        );
        assert_eq!(
            condition_operation(&Condition::Absent("k".into())),
            json!({"KV": {"Verb": "check-not-exists", "Key": "k"}})
        );
        let locked = Write::Put {
            key: "k".into(),
            value: b"v".to_vec(),
            lease: Some("s1".into()),
        };
        assert_eq!(
            write_operation(&locked),
            json!({"KV": {"Verb": "lock", "Key": "k", "Value": BASE64_STANDARD.encode("v"), "Session": "s1"}})
        );
    }
}
