use base64::prelude::{Engine, BASE64_URL_SAFE_NO_PAD};
use r3v3rs3::accounts::AccountDirectory;
use r3v3rs3::admin::start_admin;
use r3v3rs3::certs::acme::AcmeEntry;
use r3v3rs3::certs::challenges::ServedChallenges;
use r3v3rs3::clock;
use r3v3rs3::cluster;
use r3v3rs3::cluster::crypto::{ClusterKey, ClusterKeys};
use r3v3rs3::cluster::storage::KvStorage;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::config::storage::Storage;
use r3v3rs3::kv::KvStore;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::proxy::http::rate_share::{
    ClientCount, LimiterCounts, NodeCounts, RateCountExchange, WindowCount,
};
use r3v3rs3::server::rpc::acme::GetAcmeList;
use r3v3rs3::server::rpc::cluster::GetClusterStatus;
use r3v3rs3::server::rpc::config::{GetConfig, SetConfig};
use r3v3rs3::server::rpc::ports::{AddPort, GetPortList};
use r3v3rs3::server::rpc::proxies::{AddProxy, GetProxyList};
use r3v3rs3::server::Server;
use r3v3rs3::sessions::{SessionBackend, SessionRecord, SessionScope};
use r3v3rs3_api::acme::{Acme, AcmeConfig, HTTP_01};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::cache::CacheConfig;
use r3v3rs3_api::cluster::{ClusterConfig, ClusterState, ClusterStatus};
use r3v3rs3_api::error::Error;
use r3v3rs3_api::event::ServerEvent;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::policy::{RateLimit, RatePeriod};
use r3v3rs3_api::proxy::{HttpProxy, Proxy, ProxyKind};
use reqwest::{Client, Url};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::cluster::{stop_server, Node};
use common::kv::{check_locks_and_leases, MemoryStore};
use common::{
    admin_session_cookie, alloc_tcp_port, call, http_port_entry, http_route, wait_for_listener,
    wait_until,
};

const SYNC_INTERVAL: Duration = Duration::from_millis(100);

/// The storage of a node with the memory store.
fn node_storage(store: &Arc<MemoryStore>, name: &str) -> anyhow::Result<KvStorage> {
    let local = AppConfig {
        cluster: ClusterConfig {
            enabled: true,
            endpoints: vec!["http://127.0.0.1:2379".into()],
            node_name: name.into(),
            encryption_key_files: vec!["/etc/r3v3rs3/cluster.key".into()],
            lock_ttl: Duration::from_secs(1),
            rate_limit_sync_interval: SYNC_INTERVAL,
            share_cache: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let keys = ClusterKeys::new(vec![ClusterKey::new(&[1; 32])?])?;
    Ok(KvStorage::new(store.clone(), keys, local))
}

async fn start_node(store: &Arc<MemoryStore>, name: &str) -> anyhow::Result<Node> {
    Node::start(node_storage(store, name)?).await
}

#[tokio::test]
async fn a_change_on_one_node_reaches_the_other_node() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut a = start_node(&store, "node-a").await?;
    let mut b = start_node(&store, "node-b").await?;
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
    a.call(AddProxy {
        entry: proxy,
        owner: None,
    })
    .await?;
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
    let mut node = start_node(&store, "node-b").await?;
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

/// A node that serves the admin API instead of answering the calls of the test.
struct AdminNode {
    addr: SocketAddr,
    event: broadcast::Sender<ServerEvent>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl AdminNode {
    async fn start(store: &Arc<MemoryStore>, name: &str, dir: &Path) -> anyhow::Result<Self> {
        let storage = Arc::new(node_storage(store, name)?);
        let (server, channels) = Server::new_shared(new_appinfo(dir, dir), storage.clone()).await;
        cluster::spawn_tasks(storage, channels.command.clone());
        let task = tokio::spawn(server.start());
        let addr = alloc_tcp_port().await?.socket_addr();
        tokio::spawn(start_admin(
            new_appinfo(dir, dir),
            addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
        wait_for_listener(addr).await?;
        Ok(Self {
            addr,
            event: channels.event,
            task,
        })
    }

    async fn status(&self, path: &str, cookie: &str) -> anyhow::Result<u16> {
        let res = Client::new()
            .get(format!("http://{}{path}", self.addr))
            .header(reqwest::header::COOKIE, cookie)
            .send()
            .await?;
        Ok(res.status().as_u16())
    }

    async fn stop(self) -> anyhow::Result<()> {
        stop_server(&self.event, self.task).await
    }
}

#[tokio::test]
async fn a_session_of_one_node_is_valid_on_the_other_node_until_logout() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-cluster-session-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    let store = Arc::new(MemoryStore::default());
    node_storage(&store, "import")?
        .add_account("admin", "secret", false, Role::Admin)
        .await?;
    let a = AdminNode::start(&store, "node-a", &dir).await?;
    let b = AdminNode::start(&store, "node-b", &dir).await?;

    let cookie = admin_session_cookie(a.addr).await?;
    assert_eq!(b.status("/api/ports", &cookie).await?, 200);
    assert_eq!(a.status("/api/logout", &cookie).await?, 200);
    assert_eq!(b.status("/api/ports", &cookie).await?, 401);
    assert_eq!(a.status("/api/ports", &cookie).await?, 401);

    a.stop().await?;
    b.stop().await?;
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[tokio::test]
async fn a_client_that_used_its_limit_on_another_node_is_limited() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut a = start_node(&store, "node-a").await?;
    let port = alloc_tcp_port().await?;
    let addr = port.socket_addr();
    a.call(AddPort {
        entry: http_port_entry("web", &port).port,
    })
    .await?;
    let ports = a.call(GetPortList).await?;
    let proxy = Proxy {
        ports: vec![ports[0].id],
        kind: ProxyKind::Http(Box::new(HttpProxy {
            routes: vec![http_route("/", "http://127.0.0.1:1/", None)],
            rate_limit: RateLimit {
                requests: 3,
                per: RatePeriod::Hour,
                burst: 0,
            },
            ..Default::default()
        })),
        ..Default::default()
    };
    a.call(AddProxy {
        entry: proxy,
        owner: None,
    })
    .await?;
    let id = a.call(GetProxyList).await?[0].id;

    // Node B already served the limit of the client.
    let b = node_storage(&store, "node-b")?;
    let client = ClientCount {
        ip: "127.0.0.1".parse()?,
        count: WindowCount {
            window: clock::unix_ms() / 3_600_000,
            current: 3,
            previous: 0,
        },
    };
    let limiter = LimiterCounts {
        key: format!("{id}/-"),
        period_ms: 3_600_000,
        counts: vec![client],
    };
    let publish = async {
        loop {
            let counts = NodeCounts {
                published_at: clock::unix_ms(),
                limiters: vec![limiter.clone()],
            };
            if let Err(err) = b.exchange(&counts).await {
                return err;
            }
            tokio::time::sleep(SYNC_INTERVAL).await;
        }
    };
    let request = async {
        tokio::time::sleep(SYNC_INTERVAL * 10).await;
        for _ in 0..100 {
            if let Ok(response) = Client::new().get(format!("http://{addr}/")).send().await {
                return Ok(response.status().as_u16());
            }
            tokio::time::sleep(SYNC_INTERVAL).await;
        }
        anyhow::bail!("the proxy port did not accept connections")
    };
    let status = tokio::select! {
        err = publish => return Err(err),
        status = request => status?,
    };
    assert_eq!(status, 429);
    a.stop().await
}

/// Sends a request until the node accepts the connection. Returns the `x-cache` header and the body.
async fn cached_get(url: &Url) -> anyhow::Result<(String, String)> {
    for _ in 0..100 {
        if let Ok(response) = Client::new().get(url.clone()).send().await {
            let cache = response
                .headers()
                .get("x-cache")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            return Ok((cache, response.text().await?));
        }
        tokio::time::sleep(SYNC_INTERVAL).await;
    }
    anyhow::bail!("the proxy port did not accept connections")
}

async fn wait_for_keys(store: &MemoryStore, prefix: &str) -> anyhow::Result<()> {
    let written = || async { Ok(!store.list(prefix).await?.items.is_empty()) };
    wait_until(written, &format!("the store has no key below {prefix}")).await
}

#[tokio::test]
async fn a_cached_response_of_one_node_serves_another_node() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut upstream = mockito::Server::new_async().await;
    let mock = upstream
        .mock("GET", "/page")
        .with_header("cache-control", "max-age=600")
        .with_body("page")
        .expect(2)
        .create_async()
        .await;
    let mut a = start_node(&store, "node-a").await?;
    let port = alloc_tcp_port().await?;
    a.call(AddPort {
        entry: http_port_entry("web", &port).port,
    })
    .await?;
    let ports = a.call(GetPortList).await?;
    let proxy = Proxy {
        ports: vec![ports[0].id],
        kind: ProxyKind::Http(Box::new(HttpProxy {
            vhosts: vec!["localhost".parse()?],
            routes: vec![http_route("/", &upstream.url(), None)],
            upgrade_insecure: false,
            cache: CacheConfig {
                enabled: true,
                ..Default::default()
            },
            ..Default::default()
        })),
        ..Default::default()
    };
    a.call(AddProxy {
        entry: proxy,
        owner: None,
    })
    .await?;
    let id = a.call(GetProxyList).await?[0].id;
    let url = port.http_url("/page");
    let responses = "r3v3rs3/v1/cache/";
    let miss = ("MISS".to_string(), "page".to_string());
    assert_eq!(cached_get(&url).await?, miss);
    wait_for_keys(&store, responses).await?;

    // A purge on another node purges the cache of this node.
    let other = node_storage(&store, "node-b")?;
    other.purge_shared_cache(id, clock::unix_ms()).await?;
    let mut purged = false;
    for _ in 0..50 {
        if cached_get(&url).await? == miss {
            purged = true;
            break;
        }
        tokio::time::sleep(SYNC_INTERVAL).await;
    }
    assert!(purged, "the node did not purge its cache");
    wait_for_keys(&store, responses).await?;
    a.stop().await?;

    // Node B has no response of its own, so it serves the response of node A.
    let b = start_node(&store, "node-b").await?;
    let hit = ("HIT".to_string(), "page".to_string());
    assert_eq!(cached_get(&url).await?, hit);
    b.stop().await?;
    mock.assert_async().await;
    Ok(())
}

#[tokio::test]
async fn the_store_keeps_sessions_until_they_expire() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let a = node_storage(&store, "node-a")?;
    let b = node_storage(&store, "node-b")?;
    let hour = Duration::from_secs(3600);
    let record = SessionRecord::proxy("example.com", "admin");
    let token = a.create(SessionScope::Proxy, record.clone(), hour).await?;

    assert_eq!(b.get(SessionScope::Proxy, &token).await?, Some(record));
    assert_eq!(b.get(SessionScope::Admin, &token).await?, None);
    let keys = store.list("").await?.items;
    assert!(keys.iter().all(|item| !item.key.contains(&token)));

    b.remove_expired(hour).await?;
    assert!(a.get(SessionScope::Proxy, &token).await?.is_some());
    b.remove_expired(Duration::ZERO).await?;
    assert_eq!(a.get(SessionScope::Proxy, &token).await?, None);
    Ok(())
}

#[tokio::test]
async fn the_memory_store_keeps_locks_and_leases() -> anyhow::Result<()> {
    check_locks_and_leases(&MemoryStore::default()).await
}

#[tokio::test]
async fn one_node_leads_and_another_node_takes_over_when_it_stops() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut nodes = [
        start_node(&store, "node-a").await?,
        start_node(&store, "node-b").await?,
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

/// An HTTP-01 entry whose ACME directory is `directory`. The order of the entry connects to it
/// first.
fn acme_entry(directory: &str) -> anyhow::Result<AcmeEntry> {
    let key = rcgen::KeyPair::generate()?;
    let credentials: instant_acme::AccountCredentials =
        serde_json::from_value(serde_json::json!({
            "id": format!("{directory}/account"),
            "key_pkcs8": BASE64_URL_SAFE_NO_PAD.encode(key.serialize_der()),
            "directory": directory,
        }))?;
    Ok(AcmeEntry {
        id: ShortId::from([1; 7]),
        acme: Acme {
            config: AcmeConfig::default(),
            identifiers: vec!["example.com".parse()?],
            challenge_type: HTTP_01.to_string(),
            dns_provider: None,
        },
        account: Arc::new(credentials),
    })
}

#[tokio::test]
async fn only_the_leader_orders_certificates() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let keys = ClusterKeys::new(vec![ClusterKey::new(&[1; 32])?])?;
    let lock = format!("{}lock/leader", cluster::data_prefix("r3v3rs3"));
    let lease = store.grant_lease(Duration::from_secs(60)).await?;
    assert!(
        store
            .try_lock(&lock, &keys.seal(&lock, b"other")?, &lease)
            .await?
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    // ACME requests need https. The client connects over TCP before the TLS handshake, so the
    // listener counts the order without a TLS server.
    let directory = format!("https://{}/directory", listener.local_addr()?);
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    let acceptor = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });

    let mut follower = start_node(&store, "node-b").await?;
    node_storage(&store, "writer")?
        .save_acme(&acme_entry(&directory)?)
        .await?;
    follower
        .wait_for(|| GetAcmeList, |list| list.len() == 1)
        .await?;
    // The follower applied the entry. It must not order during several lock checks.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(connections.load(Ordering::SeqCst), 0);

    store.revoke_lease(&lease).await?;
    let ordered = || {
        let connections = connections.clone();
        async move { Ok(connections.load(Ordering::SeqCst) > 0) }
    };
    wait_until(ordered, "the new leader did not order the certificate").await?;
    acceptor.abort();
    follower.stop().await
}

#[tokio::test]
async fn an_account_change_reaches_the_account_directory_of_a_node() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let writer = node_storage(&store, "writer")?;
    writer
        .add_account("admin", "passw0rd", false, Role::Admin)
        .await?;
    let node = start_node(&store, "node-b").await?;
    let mut directory = node.channels.accounts.clone();
    let role = |directory: &AccountDirectory| directory.get("admin").map(|entry| entry.role);
    assert_eq!(role(&directory.borrow()), Some(Role::Admin));

    let mut accounts = writer.load_accounts().await?;
    let admin = accounts
        .get_mut("admin")
        .ok_or_else(|| anyhow::anyhow!("the account is missing"))?;
    admin.role = Role::Viewer;
    writer.save_accounts(&accounts).await?;
    let changed = directory.wait_for(|directory| role(directory) == Some(Role::Viewer));
    tokio::time::timeout(Duration::from_secs(5), changed).await??;
    node.stop().await
}

#[tokio::test]
async fn a_node_that_loses_the_store_rejects_changes_until_it_returns() -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut a = start_node(&store, "node-a").await?;
    let mut b = start_node(&store, "node-b").await?;
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

#[tokio::test]
async fn a_node_whose_store_stops_answering_becomes_degraded_and_stops_leading(
) -> anyhow::Result<()> {
    let store = Arc::new(MemoryStore::default());
    let mut node = start_node(&store, "node-a").await?;
    let leading = |status: &ClusterStatus| status.state == ClusterState::Synced && status.leader;
    node.wait_for(|| GetClusterStatus, leading).await?;

    // The calls wait without an error, so only the checks of the node find the lost store.
    store.set_unresponsive(true);
    let degraded =
        |status: &ClusterStatus| status.state == ClusterState::Degraded && !status.leader;
    let status = node.wait_for(|| GetClusterStatus, degraded).await?;
    assert!(status.error.is_some());

    // The memory store keeps the lock of the lease that the node could not revoke, so the node
    // only syncs again.
    store.set_unresponsive(false);
    let synced = |status: &ClusterStatus| status.state == ClusterState::Synced;
    node.wait_for(|| GetClusterStatus, synced).await?;
    node.stop().await
}
