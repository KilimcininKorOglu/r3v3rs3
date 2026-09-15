use r3v3rs3::certs::Cert;
use r3v3rs3::cluster::crypto::{sealed_key_id, ClusterKey, ClusterKeys};
use r3v3rs3::cluster::import::import;
use r3v3rs3::cluster::storage::KvStorage;
use r3v3rs3::config::file::{FileState, FileStorage};
use r3v3rs3::config::storage::Storage;
use r3v3rs3::kv::{KvStore, Txn, Write};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::{LoginMethod, LoginRequest, LoginResponse, Role};
use r3v3rs3_api::cluster::ClusterConfig;
use r3v3rs3_api::error::Error;
use r3v3rs3_api::proxy::HttpProxy;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::kv::MemoryStore;
use common::{alloc_tcp_port, http_port_entry, http_proxy_entry, http_route};

fn keys() -> anyhow::Result<ClusterKeys> {
    ClusterKeys::new(vec![ClusterKey::new(&[1; 32])?])
}

fn local_config(node_name: &str) -> AppConfig {
    AppConfig {
        cluster: ClusterConfig {
            enabled: true,
            endpoints: vec!["http://127.0.0.1:2379".into()],
            node_name: node_name.into(),
            encryption_key_files: vec!["/etc/r3v3rs3/cluster.key".into()],
            ..Default::default()
        },
        ..Default::default()
    }
}

/// A node with the memory store.
fn node(store: &Arc<MemoryStore>, name: &str) -> anyhow::Result<KvStorage> {
    Ok(KvStorage::new(store.clone(), keys()?, local_config(name)))
}

async fn read_files(files: &FileStorage) -> anyhow::Result<(FileState, Cert)> {
    let mut config = local_config("node-a");
    config.background_task_interval = Duration::from_secs(600);
    files.save_app_config(&config).await?;
    let port = http_port_entry("test", &alloc_tcp_port().await?);
    files.save_ports(&[port]).await?;
    let proxy = HttpProxy {
        routes: vec![http_route("/", "http://127.0.0.1:1/", None)],
        ..Default::default()
    };
    files
        .save_proxies(&[http_proxy_entry("manual", "test", proxy)])
        .await?;
    let ca = Cert::new_ca()?;
    files.save_cert(&ca).await?;
    files
        .add_account("admin", "passw0rd", false, Role::Admin)
        .await?;
    Ok((files.read_state().await?, ca))
}

#[tokio::test]
async fn an_import_copies_the_files_and_another_node_reads_them() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!(
        "r3v3rs3-import-{}",
        hex::encode(rand::random::<[u8; 8]>())
    ));
    let files = FileStorage::new(&dir);
    let read = read_files(&files).await;
    std::fs::remove_dir_all(&dir)?;
    let (state, ca) = read?;
    let (ports, proxies) = (state.ports.clone(), state.proxies.clone());

    let store = Arc::new(MemoryStore::default());
    let first = node(&store, "node-a")?;
    assert!(!first.is_imported().await?);
    let report = import(&first, state).await?;
    assert_eq!(
        (report.ports, report.proxies, report.certs, report.accounts),
        (1, 1, 1, 1)
    );
    assert!(first.is_imported().await?);

    // The config is encrypted and the ports are not.
    let config = store.value("r3v3rs3/v1/state/config")?;
    assert!(sealed_key_id(&config).is_some());
    let port = store.value(&format!("r3v3rs3/v1/state/ports/{}", ports[0].id))?;
    assert!(sealed_key_id(&port).is_none());

    // The other node keeps its own cluster settings.
    let second = node(&store, "node-b")?;
    let loaded = second.load_app_config().await;
    assert_eq!(loaded.background_task_interval, Duration::from_secs(600));
    assert_eq!(loaded.cluster, local_config("node-b").cluster);
    assert_eq!(second.load_ports().await, ports);
    assert_eq!(second.load_proxies().await, proxies);
    let certs = second.load_certs().await;
    assert_eq!(certs.len(), 1);
    assert_eq!(certs[0].fingerprint, ca.fingerprint);
    assert!(certs[0].key.is_some());
    let login = LoginRequest {
        username: "admin".into(),
        method: LoginMethod::Password {
            password: "passw0rd".into(),
        },
        insecure: false,
    };
    let response = second.verify_account(login).await;
    assert!(
        matches!(response, Ok(LoginResponse::Success)),
        "{response:?}"
    );

    let again = import(&second, FileState::default()).await;
    assert!(again.is_err());
    Ok(())
}

#[tokio::test]
async fn a_save_writes_only_changed_keys_and_reports_conflicts() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let first = node(&store, "node-a")?;
    let one = http_port_entry("one", &alloc_tcp_port().await?);
    let two = http_port_entry("two", &alloc_tcp_port().await?);

    first.save_ports(&[one.clone(), two.clone()]).await?;
    let commits = store.commits();
    first.save_ports(&[one.clone(), two.clone()]).await?;
    assert_eq!(store.commits(), commits);
    first.save_ports(std::slice::from_ref(&one)).await?;
    assert!(store.value("r3v3rs3/v1/state/ports/two").is_err());

    // The second node changes the port after the first node wrote it.
    let second = node(&store, "node-b")?;
    assert_eq!(second.load_ports().await, std::slice::from_ref(&one));
    let mut theirs = one.clone();
    theirs.port.name = "theirs".into();
    second.save_ports(&[theirs.clone()]).await?;

    let mut mine = one.clone();
    mine.port.name = "mine".into();
    let conflict = first.save_ports(&[mine.clone()]).await;
    assert!(
        matches!(conflict, Err(Error::ClusterWriteConflict)),
        "{conflict:?}"
    );
    assert_eq!(second.load_ports().await, [theirs]);
    // The conflict read the new version, so the next save succeeds.
    first.save_ports(&[mine.clone()]).await?;
    assert_eq!(second.load_ports().await, [mine]);

    store.set_unavailable(true);
    let unavailable = first.save_app_config(&AppConfig::default()).await;
    assert!(
        matches!(unavailable, Err(Error::ClusterUnavailable)),
        "{unavailable:?}"
    );
    Ok(())
}

#[tokio::test]
async fn an_account_save_conflicts_with_a_change_of_another_account() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let first = node(&store, "node-a")?;
    first
        .add_account("one", "passw0rd", false, Role::Admin)
        .await?;
    first
        .add_account("two", "passw0rd", false, Role::Admin)
        .await?;
    let second = node(&store, "node-b")?;
    let mut mine = first.load_accounts().await?;
    let mut theirs = second.load_accounts().await?;
    assert_eq!(mine.len(), 2);

    // Each node demotes a different admin. One save must fail, so an admin stays.
    let missing = || anyhow::anyhow!("the account is missing");
    theirs.get_mut("two").ok_or_else(missing)?.role = Role::Viewer;
    second.save_accounts(&theirs).await?;
    mine.get_mut("one").ok_or_else(missing)?.role = Role::Viewer;
    let conflict = first.save_accounts(&mine).await;
    assert!(
        matches!(conflict, Err(Error::ClusterWriteConflict)),
        "{conflict:?}"
    );

    let mut accounts = first.load_accounts().await?;
    assert_eq!(accounts["one"].role, Role::Admin);
    assert_eq!(accounts["two"].role, Role::Viewer);

    // The save after the load deletes the account that the map does not have.
    accounts.remove("one");
    first.save_accounts(&accounts).await?;
    let left = second.load_accounts().await?;
    assert_eq!(left.keys().collect::<Vec<_>>(), ["two"]);
    Ok(())
}

#[tokio::test]
async fn an_unencrypted_value_under_an_encrypted_key_is_not_used() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let injected = AppConfig {
        background_task_interval: Duration::from_secs(5),
        ..Default::default()
    };
    let txn = Txn {
        conditions: vec![],
        writes: vec![Write::Put {
            key: "r3v3rs3/v1/state/config".into(),
            value: serde_json::to_vec(&injected)?,
            lease: None,
        }],
    };
    store.commit(&txn).await?;
    let storage = node(&store, "node-a")?;
    assert_eq!(storage.load_app_config().await, local_config("node-a"));
    Ok(())
}
