//! [`KvStore`] for etcd. A lock is a key that a transaction creates with the lease of its holder.

use super::etcd::{
    EtcdClient, Event, EventType, KeyRange, KeyValue, RangeResponse, int64, next_watch_line,
    watch_result,
};
use super::http::{Lines, RESPONSE_TIMEOUT, read_json};
use super::{
    Condition, KvEvent, KvItem, KvList, KvStore, KvWatcher, Lease, Txn, TxnOutcome, WatchBatch,
    Write,
};
use anyhow::{Context as _, anyhow, bail};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use hyper::StatusCode;
use serde_derive::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct TxnResponse {
    /// The JSON gateway omits `false`.
    #[serde(default)]
    succeeded: bool,
    #[serde(default)]
    responses: Vec<ResponseOp>,
}

#[derive(Debug, Deserialize)]
struct ResponseOp {
    #[serde(default)]
    response_range: Option<RangeResponse>,
}

#[derive(Debug, Deserialize)]
struct LeaseGrant {
    #[serde(rename = "ID", default, deserialize_with = "int64")]
    id: i64,
    #[serde(rename = "TTL", default, deserialize_with = "int64")]
    ttl: i64,
    #[serde(default)]
    error: String,
}

/// A message of the keepalive stream.
#[derive(Debug, Deserialize)]
struct KeepAliveMessage {
    #[serde(default)]
    result: Option<KeepAliveResult>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct KeepAliveResult {
    /// 0 for a lease that ended.
    #[serde(rename = "TTL", default, deserialize_with = "int64")]
    ttl: i64,
}

fn encode(bytes: impl AsRef<[u8]>) -> String {
    BASE64_STANDARD.encode(bytes)
}

fn decode_key(key: &str) -> anyhow::Result<String> {
    let bytes = BASE64_STANDARD.decode(key).context("invalid etcd key")?;
    String::from_utf8(bytes).context("the etcd key is not UTF-8 text")
}

fn item(kv: KeyValue) -> anyhow::Result<KvItem> {
    let value = BASE64_STANDARD
        .decode(kv.value.unwrap_or_default())
        .context("invalid etcd value")?;
    Ok(KvItem {
        key: decode_key(&kv.key)?,
        value,
        version: u64::try_from(kv.mod_revision)?,
    })
}

fn event(event: Event) -> anyhow::Result<KvEvent> {
    match event.kind {
        EventType::Put => Ok(KvEvent::Put(item(event.kv)?)),
        EventType::Delete => Ok(KvEvent::Delete(decode_key(&event.kv.key)?)),
    }
}

fn compare(condition: &Condition) -> Value {
    match condition {
        Condition::Absent(key) => {
            json!({"key": encode(key), "target": "CREATE", "result": "EQUAL", "create_revision": "0"})
        }
        Condition::Version(key, version) => {
            json!({"key": encode(key), "target": "MOD", "result": "EQUAL", "mod_revision": version.to_string()})
        }
        Condition::LockHeld { key, lease } => {
            json!({"key": encode(key), "target": "LEASE", "result": "EQUAL", "lease": lease})
        }
    }
}

fn request_op(write: &Write) -> Value {
    match write {
        Write::Put { key, value, lease } => {
            let mut put = json!({"key": encode(key), "value": encode(value)});
            if let Some(lease) = lease {
                put["lease"] = json!(lease);
            }
            json!({ "request_put": put })
        }
        Write::Update { key, value } => json!({"request_put": {
            "key": encode(key),
            "value": encode(value),
            "ignore_lease": true,
        }}),
        Write::Delete(key) => json!({"request_delete_range": {"key": encode(key)}}),
    }
}

async fn list(client: &EtcdClient, prefix: &str) -> anyhow::Result<KvList> {
    let range = client.range(&KeyRange::with_prefix(prefix)).await?;
    let revision = u64::try_from(range.revision())?;
    let items = range
        .kvs
        .into_iter()
        .map(item)
        .collect::<anyhow::Result<_>>()?;
    Ok(KvList { items, revision })
}

async fn send_txn(client: &EtcdClient, body: &Value) -> anyhow::Result<TxnResponse> {
    read_json(client.post("/v3/kv/txn", body).await?).await
}

#[async_trait::async_trait]
impl KvStore for EtcdClient {
    async fn list(&self, prefix: &str) -> anyhow::Result<KvList> {
        list(self, prefix).await
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<KvItem>> {
        let body = json!({"key": encode(key)});
        let range: RangeResponse = read_json(self.post("/v3/kv/range", &body).await?).await?;
        range.kvs.into_iter().next().map(item).transpose()
    }

    async fn commit(&self, txn: &Txn) -> anyhow::Result<TxnOutcome> {
        let body = json!({
            "compare": txn.conditions.iter().map(compare).collect::<Vec<_>>(),
            "success": txn.writes.iter().map(request_op).collect::<Vec<_>>(),
        });
        Ok(if send_txn(self, &body).await?.succeeded {
            TxnOutcome::Committed
        } else {
            TxnOutcome::Conflict
        })
    }

    async fn grant_lease(&self, ttl: Duration) -> anyhow::Result<Lease> {
        if ttl.as_secs() == 0 {
            bail!("an etcd lease needs a TTL of at least one second");
        }
        let body = json!({"TTL": ttl.as_secs().to_string()});
        let grant: LeaseGrant = read_json(self.post("/v3/lease/grant", &body).await?).await?;
        if !grant.error.is_empty() {
            bail!("etcd did not grant the lease: {}", grant.error);
        }
        Ok(Lease {
            id: grant.id.to_string(),
            ttl: Duration::from_secs(u64::try_from(grant.ttl)?),
        })
    }

    async fn keep_alive(&self, lease: &Lease) -> anyhow::Result<()> {
        let body = json!({"ID": lease.id});
        let mut lines = Lines::new(self.post("/v3/lease/keepalive", &body).await?);
        let line = tokio::time::timeout(RESPONSE_TIMEOUT, lines.next())
            .await
            .map_err(|_| anyhow!("the lease keepalive timed out"))??
            .context("the lease keepalive stream ended")?;
        let message: KeepAliveMessage =
            serde_json::from_slice(&line).context("invalid lease keepalive message")?;
        if let Some(error) = message.error {
            bail!("the lease keepalive failed: {error}");
        }
        match message.result {
            Some(result) if result.ttl > 0 => Ok(()),
            _ => bail!("the lease {} ended", lease.id),
        }
    }

    async fn revoke_lease(&self, lease: &Lease) -> anyhow::Result<()> {
        // etcd does not find a lease that already ended.
        let body = json!({"ID": lease.id});
        self.post_with("/v3/lease/revoke", &body, &[StatusCode::NOT_FOUND])
            .await?;
        Ok(())
    }

    async fn try_lock(&self, key: &str, value: &[u8], lease: &Lease) -> anyhow::Result<bool> {
        let put = Write::Put {
            key: key.to_string(),
            value: value.to_vec(),
            lease: Some(lease.id.clone()),
        };
        let body = json!({
            "compare": [compare(&Condition::Absent(key.to_string()))],
            "success": [request_op(&put)],
            "failure": [{"request_range": {"key": encode(key)}}],
        });
        let response = send_txn(self, &body).await?;
        if response.succeeded {
            return Ok(true);
        }
        let holder = response
            .responses
            .into_iter()
            .filter_map(|op| op.response_range)
            .flat_map(|range| range.kvs)
            .next();
        Ok(holder.is_some_and(|kv| kv.lease.to_string() == lease.id))
    }

    fn watch(&self, prefix: &str, from: &KvList) -> Box<dyn KvWatcher> {
        Box::new(EtcdWatcher {
            client: self.clone(),
            prefix: prefix.to_string(),
            revision: from.revision,
            lines: None,
        })
    }
}

struct EtcdWatcher {
    client: EtcdClient,
    prefix: String,
    /// The revision of the last change that the watcher returned.
    revision: u64,
    /// The open watch stream.
    lines: Option<Lines>,
}

#[async_trait::async_trait]
impl KvWatcher for EtcdWatcher {
    async fn next(&mut self) -> anyhow::Result<WatchBatch> {
        loop {
            if let Some(batch) = self.read().await? {
                return Ok(batch);
            }
        }
    }
}

impl EtcdWatcher {
    /// Reads one message of the watch stream. `None` for a message without a change. The stream
    /// stays open only after a message without an error.
    async fn read(&mut self) -> anyhow::Result<Option<WatchBatch>> {
        let mut lines = match self.lines.take() {
            Some(lines) => lines,
            None => {
                let range = KeyRange::with_prefix(&self.prefix);
                let start = i64::try_from(self.revision)?;
                self.client.watch(&range, start).await?
            }
        };
        let line = next_watch_line(&mut lines).await?;
        if line.trim_ascii().is_empty() {
            self.lines = Some(lines);
            return Ok(None);
        }
        let result = watch_result(&line)?;
        if result.compact_revision > 0 {
            // etcd ends the stream of a compacted start revision.
            let list = list(&self.client, &self.prefix).await?;
            self.revision = list.revision;
            return Ok(Some(WatchBatch::Resync(list)));
        }
        let last = result
            .events
            .iter()
            .map(|event| event.kv.mod_revision)
            .max();
        let events = result
            .events
            .into_iter()
            .map(event)
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.lines = Some(lines);
        let Some(last) = last else {
            return Ok(None);
        };
        self.revision = self.revision.max(u64::try_from(last)?);
        Ok(Some(WatchBatch::Events {
            events,
            revision: self.revision,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions_and_writes_use_the_gateway_fields() {
        let key = encode("k");
        assert_eq!(
            compare(&Condition::Version("k".into(), 9)),
            json!({"key": key, "target": "MOD", "result": "EQUAL", "mod_revision": "9"})
        );
        assert_eq!(
            compare(&Condition::LockHeld {
                key: "k".into(),
                lease: "77".into()
            }),
            json!({"key": key, "target": "LEASE", "result": "EQUAL", "lease": "77"})
        );
        let put = Write::Put {
            key: "k".into(),
            value: b"v".to_vec(),
            lease: Some("77".into()),
        };
        assert_eq!(
            request_op(&put),
            json!({"request_put": {"key": key, "value": encode("v"), "lease": "77"}})
        );
        assert_eq!(
            request_op(&Write::Delete("k".into())),
            json!({"request_delete_range": {"key": key}})
        );
    }
}
