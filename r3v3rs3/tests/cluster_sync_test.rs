use r3v3rs3::certs::challenges::ServedChallenges;
use r3v3rs3::cluster;
use r3v3rs3::cluster::crypto::{ClusterKey, ClusterKeys};
use r3v3rs3::cluster::storage::KvStorage;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::config::storage::Storage;
use r3v3rs3::server::rpc::cluster::GetClusterStatus;
use r3v3rs3::server::rpc::config::{GetConfig, SetConfig};
use r3v3rs3::server::rpc::ports::{AddPort, GetPortList};
use r3v3rs3::server::rpc::proxies::{AddProxy, GetProxyList};
use r3v3rs3::server::rpc::RpcMethod;
use r3v3rs3::server::{Server, ServerChannels};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::cluster::{ClusterConfig, ClusterState, ClusterStatus};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::proxy::{HttpProxy, Proxy, ProxyKind};
use reqwest::Client;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinHandle;

mod common;
use common::kv::{check_locks_and_leases, MemoryStore};
use common::{alloc_tcp_port, call, http_port_entry, http_route, wait_for_rpc};

/// The storage of a node with the memory store.
fn node_storage(store: &Arc<MemoryStore>, name: &str) -> anyhow::Result<KvStorage> {
    let local = AppConfig {
        cluster: ClusterConfig {
            enabled: true,
            endpoints: vec!["http://127.0.0.1:2379".into()],
            node_name: name.into(),
            encryption_key_files: vec!["/etc/r3v3rs3/cluster.key".into()],
            lock_ttl: Duration::from_secs(1),
            ..Default::default()
        },
        ..Default::default()
    };
    let keys = ClusterKeys::new(vec![ClusterKey::new(&[1; 32])?])?;
    Ok(KvStorage::new(store.clone(), keys, local))
}

struct Node {
    channels: ServerChannels,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Node {
    async fn start(store: &Arc<MemoryStore>, name: &str) -> anyhow::Result<Self> {
        let storage = Arc::new(node_storage(store, name)?);
        let app_info = new_appinfo(Path::new("."), Path::new("."));
        let (server, channels) = Server::new_shared(app_info, storage.clone()).await;
        cluster::spawn_tasks(storage, channels.command.clone());
        let task = tokio::spawn(server.start());
        Ok(Self { channels, task })
    }

    async fn call<M: RpcMethod + 'static>(&mut self, method: M) -> anyhow::Result<M::Output> {
        Ok(call(&mut self.channels, method).await??)
    }

    async fn wait_for<M>(
        &mut self,
        method: impl Fn() -> M,
        done: impl Fn(&M::Output) -> bool,
    ) -> anyhow::Result<M::Output>
    where
        M: RpcMethod + 'static,
        M::Output: std::fmt::Debug,
    {
        wait_for_rpc(&mut self.channels, method, done).await
    }

    async fn stop(self) -> anyhow::Result<()> {
        self.channels.event.send(ServerEvent::Shutdown)?;
        self.task.await?
    }
}

#[tokio::test]
async fn a_change_on_one_node_reaches_the_other_node() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut a = Node::start(&store, "node-a").await?;
    let mut b = Node::start(&store, "node-b").await?;
    let synced = |status: &ClusterStatus| status.state == ClusterState::Synced;
    let status = b.wait_for(|| GetClusterStatus, synced).await?;
    assert_eq!(status.node_name, "node-b");

    let mut config = a.call(GetConfig).await?;
    config.background_task_interval = Duration::from_secs(777);
    a.call(SetConfig { config }).await?;
    let config = b
        .wait_for(
            || GetConfig,
            |config| config.background_task_interval == Duration::from_secs(777),
        )
        .await?;
    assert_eq!(config.cluster.node_name, "node-b");

    let port = http_port_entry("web", &alloc_tcp_port().await?);
    a.call(AddPort { entry: port.port }).await?;
    let ports = b.wait_for(|| GetPortList, |ports| ports.len() == 1).await?;

    let proxy = Proxy {
        ports: vec![ports[0].id],
        kind: ProxyKind::Http(Box::new(HttpProxy {
            routes: vec![http_route("/", "http://127.0.0.1:1/", None)],
            ..Default::default()
        })),
        ..Default::default()
    };
    a.call(AddProxy { entry: proxy }).await?;
    let proxies = b
        .wait_for(|| GetProxyList, |proxies| proxies.len() == 1)
        .await?;
    assert_eq!(proxies, a.call(GetProxyList).await?);

    a.stop().await?;
    b.stop().await
}

#[tokio::test]
async fn every_node_serves_the_challenges_of_the_store() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let leader = node_storage(&store, "leader")?;
    let addr = alloc_tcp_port().await?.socket_addr();
    let config = AppConfig {
        http_challenge_addr: addr,
        ..Default::default()
    };
    leader.save_app_config(&config).await?;
    let mut node = Node::start(&store, "node-b").await?;
    let synced = |status: &ClusterStatus| status.state == ClusterState::Synced;
    node.wait_for(|| GetClusterStatus, synced).await?;

    let challenges = ServedChallenges {
        http: HashMap::from([("token".into(), "token.key".into())]),
        ..Default::default()
    };
    leader.save_challenges(&challenges).await?;
    let client = Client::new();
    let url = format!("http://{addr}/.well-known/acme-challenge/token");
    let mut body = None;
    for _ in 0..100 {
        if let Ok(response) = client.get(&url).send().await {
            body = Some(response.text().await?);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(body.as_deref(), Some("token.key"));
    assert!(
        leader
            .wait_for_nodes(&challenges, Duration::from_secs(5))
            .await?
    );
    // No node serves these challenges, so the wait ends without them.
    let other = ServedChallenges {
        http: HashMap::from([("other".into(), "other.key".into())]),
        ..Default::default()
    };
    assert!(
        !leader
            .wait_for_nodes(&other, Duration::from_millis(300))
            .await?
    );

    leader.save_challenges(&ServedChallenges::default()).await?;
    // A new connection, because the client keeps its earlier connection open.
    let mut closed = false;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(addr).await.is_err() {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(closed, "the node still serves the challenge listener");
    node.stop().await
}

#[tokio::test]
async fn the_memory_store_keeps_locks_and_leases() -> anyhow::Result<()> {
    check_locks_and_leases(&MemoryStore::default()).await
}

#[tokio::test]
async fn one_node_leads_and_another_node_takes_over_when_it_stops() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut nodes = [
        Node::start(&store, "node-a").await?,
        Node::start(&store, "node-b").await?,
    ];
    let mut leaders = Vec::new();
    for _ in 0..100 {
        leaders.clear();
        for (index, node) in nodes.iter_mut().enumerate() {
            if node.call(GetClusterStatus).await?.leader {
                leaders.push(index);
            }
        }
        if !leaders.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(leaders.len(), 1, "{leaders:?}");
    // The lock of the leader stays, so the other node does not lead after several lock checks.
    tokio::time::sleep(Duration::from_secs(1)).await;
    for (index, node) in nodes.iter_mut().enumerate() {
        let leader = node.call(GetClusterStatus).await?.leader;
        assert_eq!(leader, index == leaders[0], "node {index}");
    }

    let [first, second] = nodes;
    let (leader, mut follower) = match leaders[0] {
        0 => (first, second),
        _ => (second, first),
    };
    leader.stop().await?;
    let leads = |status: &ClusterStatus| status.leader;
    follower.wait_for(|| GetClusterStatus, leads).await?;
    follower.stop().await
}

#[tokio::test]
async fn a_node_that_loses_the_store_rejects_changes_until_it_returns() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut a = Node::start(&store, "node-a").await?;
    let mut b = Node::start(&store, "node-b").await?;
    let state = |expected: ClusterState| move |status: &ClusterStatus| status.state == expected;
    b.wait_for(|| GetClusterStatus, state(ClusterState::Synced))
        .await?;

    store.set_unavailable(true);
    let status = b
        .wait_for(|| GetClusterStatus, state(ClusterState::Degraded))
        .await?;
    assert!(status.error.is_some());
    let config = b.call(GetConfig).await?;
    let rejected = call(&mut b.channels, SetConfig { config }).await?;
    assert!(
        matches!(rejected, Err(Error::ClusterUnavailable)),
        "{rejected:?}"
    );

    store.set_unavailable(false);
    let status = b
        .wait_for(|| GetClusterStatus, state(ClusterState::Synced))
        .await?;
    assert_eq!(status.error, None);
    let mut config = b.call(GetConfig).await?;
    config.background_task_interval = Duration::from_secs(555);
    b.call(SetConfig { config }).await?;
    a.wait_for(
        || GetConfig,
        |config| config.background_task_interval == Duration::from_secs(555),
    )
    .await?;

    a.stop().await?;
    b.stop().await
}
