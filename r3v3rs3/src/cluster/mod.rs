//! A cluster of nodes that share their state in etcd or Consul.

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
use std::sync::Arc;
use storage::KvStorage;
use tokio::sync::mpsc;

/// The prefix of every key of the cluster data, with its schema version.
pub fn data_prefix(prefix: &str) -> String {
    format!("{}/v1/", prefix.trim_matches('/'))
}

/// Starts the tasks that follow the store and hold the leader lock for the server.
pub fn spawn_tasks(storage: Arc<KvStorage>, command: mpsc::Sender<ServerCommand>) {
    sync::spawn(storage.clone(), command.clone());
    leader::spawn(storage, command);
}
