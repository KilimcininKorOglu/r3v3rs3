//! The cluster against the etcd and Consul services of `tests/discovery/docker-compose.yml`.
//!
//! `make test-cluster-e2e` starts the services, runs the tests and removes the services. Each test
//! imports the files of a node below a new key prefix. The nodes connect with an etcd user or a
//! Consul token that has only the permissions of the cluster guide, so the tests also check these
//! permissions.

use base64::prelude::{Engine as _, BASE64_STANDARD};
use r3v3rs3::clock::unix_ms;
use r3v3rs3::cluster::import;
use r3v3rs3::cluster::key_file::write_new_key_file;
use r3v3rs3::cluster::storage::KvStorage;
use r3v3rs3::config::file::FileStorage;
use r3v3rs3::config::storage::Storage;
use r3v3rs3::server::rpc::cluster::GetClusterStatus;
use r3v3rs3::server::rpc::config::{GetConfig, SetConfig};
use r3v3rs3::server::rpc::ports::GetPortList;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::cluster::{ClusterBackend, ClusterConfig, ClusterState, ClusterStatus};
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;

mod common;
use common::cluster::Node;
use common::e2e::{consul_put, etcd_post, etcd_token, CONSUL, ETCD};
use common::{alloc_tcp_port, http_port_entry};

/// The parent of the key prefixes of the tests. The restricted credentials write only below it.
const E2E_PREFIX: &str = "r3v3rs3-e2e/";
/// The first key after every key below [`E2E_PREFIX`].
const E2E_PREFIX_END: &str = "r3v3rs3-e2e0";

/// A new name for the credentials, the key prefix and the directory of a test.
fn run_name(backend: &str) -> String {
    format!("{backend}-{}", unix_ms())
}

fn test_dir(name: &str) -> anyhow::Result<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../target/cluster-e2e")
        .join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Adds an etcd user whose role reads and writes the keys below [`E2E_PREFIX`], and returns its
/// password.
async fn etcd_cluster_user(name: &str) -> anyhow::Result<String> {
    let root = etcd_token().await?;
    let token = Some(root.as_str());
    let password = format!("{name}-password");
    etcd_post("/v3/auth/role/add", json!({"name": name}), token).await?;
    let permission = json!({
        "permType": "READWRITE",
        "key": BASE64_STANDARD.encode(E2E_PREFIX),
        "range_end": BASE64_STANDARD.encode(E2E_PREFIX_END),
    });
    let grant = json!({"name": name, "perm": permission});
    etcd_post("/v3/auth/role/grant", grant, token).await?;
    let user = json!({"name": name, "password": password});
    etcd_post("/v3/auth/user/add", user, token).await?;
    let role = json!({"user": name, "role": name});
    etcd_post("/v3/auth/user/grant", role, token).await?;
    Ok(password)
}

/// Adds a Consul token whose policy writes the keys below [`E2E_PREFIX`] and creates sessions,
/// and returns its secret.
async fn consul_cluster_token(name: &str) -> anyhow::Result<String> {
    let rules = format!(
        "key_prefix \"{E2E_PREFIX}\" {{ policy = \"write\" }}\nsession_prefix \"\" {{ policy = \"write\" }}"
    );
    let policy = json!({"Name": name, "Rules": rules});
    consul_put("/v1/acl/policy", policy.to_string()).await?;
    let token = json!({"Description": name, "Policies": [{"Name": name}]});
    let created = consul_put("/v1/acl/token", token.to_string()).await?;
    created["SecretID"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("Consul returned no token: {created}"))
}

fn node_config(cluster: &ClusterConfig, node: &str) -> AppConfig {
    AppConfig {
        cluster: ClusterConfig {
            node_name: node.into(),
            ..cluster.clone()
        },
        ..Default::default()
    }
}

/// Waits until one of the nodes leads, and checks that the other node does not. Returns the
/// leader and the other node.
async fn leader_and_follower(mut a: Node, mut b: Node) -> anyhow::Result<(Node, Node)> {
    for _ in 0..300 {
        let (a_leads, b_leads) = (
            a.call(GetClusterStatus).await?.leader,
            b.call(GetClusterStatus).await?.leader,
        );
        assert!(!(a_leads && b_leads), "both nodes lead");
        match (a_leads, b_leads) {
            (true, _) => return Ok((a, b)),
            (_, true) => return Ok((b, a)),
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
    anyhow::bail!("no node leads")
}

/// Imports the files of a node, starts two nodes from the store, and checks the sync of a change
/// and the failover of the leader.
async fn imported_nodes_share_changes_and_fail_over(
    mut cluster: ClusterConfig,
    name: &str,
) -> anyhow::Result<()> {
    let dir = test_dir(name)?;
    let key_file = dir.join("cluster.key");
    write_new_key_file(&key_file).await?;
    cluster.enabled = true;
    cluster.prefix = format!("{E2E_PREFIX}{name}");
    cluster.encryption_key_files = vec![key_file];

    let files = FileStorage::new(&dir);
    files
        .save_app_config(&node_config(&cluster, "node-a"))
        .await?;
    let port = http_port_entry("web", &alloc_tcp_port().await?);
    files.save_ports(&[port]).await?;
    let report = import::run(&dir).await?;
    assert_eq!(report.ports, 1);

    let mut a = Node::start(KvStorage::open(node_config(&cluster, "node-a")).await?).await?;
    let mut b = Node::start(KvStorage::open(node_config(&cluster, "node-b")).await?).await?;
    let synced = |status: &ClusterStatus| status.state == ClusterState::Synced;
    a.wait_for(|| GetClusterStatus, synced).await?;
    b.wait_for(|| GetClusterStatus, synced).await?;
    assert_eq!(b.call(GetPortList).await?.len(), 1);

    let mut config = a.call(GetConfig).await?;
    config.background_task_interval = Duration::from_secs(777);
    a.call(SetConfig { config }).await?;
    let changed = |config: &AppConfig| config.background_task_interval == Duration::from_secs(777);
    b.wait_for(|| GetConfig, changed).await?;

    let (leader, mut follower) = leader_and_follower(a, b).await?;
    leader.stop().await?;
    let leads = |status: &ClusterStatus| status.leader;
    follower.wait_for(|| GetClusterStatus, leads).await?;
    follower.stop().await
}

#[tokio::test]
#[ignore = "needs the discovery containers, run it with make test-cluster-e2e"]
async fn etcd_nodes_share_the_imported_state_and_fail_over() -> anyhow::Result<()> {
    let name = run_name("etcd");
    let password = etcd_cluster_user(&name).await?;
    let cluster = ClusterConfig {
        backend: ClusterBackend::Etcd,
        endpoints: vec![ETCD.into()],
        username: name.clone(),
        password: Some(password),
        lock_ttl: Duration::from_secs(3),
        ..Default::default()
    };
    imported_nodes_share_changes_and_fail_over(cluster, &name).await
}

#[tokio::test]
#[ignore = "needs the discovery containers, run it with make test-cluster-e2e"]
async fn consul_nodes_share_the_imported_state_and_fail_over() -> anyhow::Result<()> {
    let name = run_name("consul");
    let token = consul_cluster_token(&name).await?;
    let cluster = ClusterConfig {
        backend: ClusterBackend::Consul,
        endpoints: vec![CONSUL.into()],
        token: Some(token),
        lock_ttl: Duration::from_secs(10),
        ..Default::default()
    };
    imported_nodes_share_changes_and_fail_over(cluster, &name).await
}
