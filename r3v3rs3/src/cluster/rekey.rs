//! Encrypts every encrypted value of the cluster with the first key, so an older key file can be
//! removed.

use super::crypto::{ClusterKeys, sealed_key_id};
use super::{data_prefix, key_file, store};
use crate::kv::{Condition, KvItem, KvStore, Txn, TxnOutcome, Write};
use r3v3rs3_api::cluster::ClusterConfig;

/// The tries of one value that other nodes change during the run.
const ATTEMPTS: usize = 3;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RekeyReport {
    /// The values that the run encrypted with the first key.
    pub resealed: usize,
    /// The values that already used the first key.
    pub current: usize,
    /// The values that the cluster does not encrypt.
    pub plain: usize,
    /// The values that other nodes deleted during the run.
    pub deleted: usize,
}

enum Outcome {
    Resealed,
    Current,
    Plain,
    Deleted,
}

pub async fn run(config: &ClusterConfig) -> anyhow::Result<RekeyReport> {
    let keys = key_file::load_keys(&config.encryption_key_files).await?;
    let store = store::connect(config).await?;
    rekey(store.as_ref(), &keys, &data_prefix(&config.prefix)).await
}

/// Encrypts the values under `prefix` that an older key encrypted. The lease of a key stays.
pub async fn rekey(
    store: &dyn KvStore,
    keys: &ClusterKeys,
    prefix: &str,
) -> anyhow::Result<RekeyReport> {
    let mut report = RekeyReport::default();
    for item in store.list(prefix).await?.items {
        let count = match reseal(store, keys, item).await? {
            Outcome::Resealed => &mut report.resealed,
            Outcome::Current => &mut report.current,
            Outcome::Plain => &mut report.plain,
            Outcome::Deleted => &mut report.deleted,
        };
        *count += 1;
    }
    Ok(report)
}

async fn reseal(
    store: &dyn KvStore,
    keys: &ClusterKeys,
    mut item: KvItem,
) -> anyhow::Result<Outcome> {
    for _ in 0..ATTEMPTS {
        match sealed_key_id(&item.value) {
            None => return Ok(Outcome::Plain),
            Some(id) if id == keys.primary_id() => return Ok(Outcome::Current),
            Some(_) => {}
        }
        let plaintext = keys.open(&item.key, &item.value)?;
        let txn = Txn {
            conditions: vec![Condition::Version(item.key.clone(), item.version)],
            writes: vec![Write::Update {
                key: item.key.clone(),
                value: keys.seal(&item.key, &plaintext)?,
            }],
        };
        if store.commit(&txn).await? == TxnOutcome::Committed {
            return Ok(Outcome::Resealed);
        }
        // Another node changed the value after the list.
        match store.get(&item.key).await? {
            Some(current) => item = current,
            None => return Ok(Outcome::Deleted),
        }
    }
    anyhow::bail!(
        "the value of {} changed in each of {ATTEMPTS} attempts",
        item.key
    )
}
