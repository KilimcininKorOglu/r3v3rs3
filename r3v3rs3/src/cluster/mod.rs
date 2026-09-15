//! A cluster of nodes that share their state in etcd or Consul.

pub mod cache;
pub mod crypto;
pub mod import;
pub mod key_file;
pub mod layout;
pub mod leader;
pub mod rate_limit;
pub mod rekey;
pub mod sessions;
pub mod storage;
pub mod store;
pub mod sync;

use crate::command::ServerCommand;
use anyhow::anyhow;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use storage::KvStorage;
use tokio::sync::mpsc;

/// The prefix of every key of the cluster data, with its schema version.
pub fn data_prefix(prefix: &str) -> String {
    format!("{}/v1/", prefix.trim_matches('/'))
}

/// The interval of the store checks of a node: three checks in each `lock_ttl`.
fn check_interval(storage: &KvStorage) -> Duration {
    storage.cluster_config().lock_ttl / 3
}

/// Waits for a call of the store up to `limit`. Without the limit, a store that stops answering
/// without an error holds the call for the whole response timeout of the client.
async fn answer_within<T>(
    limit: Duration,
    call: impl Future<Output = anyhow::Result<T>>,
) -> anyhow::Result<T> {
    tokio::time::timeout(limit, call)
        .await
        .map_err(|_| anyhow!("the cluster store did not answer in {limit:?}"))?
}

/// Starts the tasks that follow the store and hold the leader lock for the server.
pub fn spawn_tasks(storage: Arc<KvStorage>, command: mpsc::Sender<ServerCommand>) {
    sync::spawn(storage.clone(), command.clone());
    leader::spawn(storage, command);
}
