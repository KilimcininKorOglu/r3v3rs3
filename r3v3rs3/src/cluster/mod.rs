//! A cluster of nodes that share their state in etcd or Consul.

pub mod crypto;
pub mod import;
pub mod key_file;
pub mod layout;
pub mod rekey;
pub mod storage;
pub mod store;
pub mod sync;

/// The prefix of every key of the cluster data, with its schema version.
pub fn data_prefix(prefix: &str) -> String {
    format!("{}/v1/", prefix.trim_matches('/'))
}
