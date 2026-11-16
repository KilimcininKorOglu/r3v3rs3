//! Integration tests for the real-time access log and traffic streaming API.

use r3v3rs3::admin::start_admin;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::proxy::access_log::record;
use r3v3rs3::server::Server;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::id::ShortId;
use r3v3rs3_api::traffic::AccessLogEntry;
use reqwest::header::COOKIE;
use reqwest::{Client, StatusCode};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use tracing_subscriber::filter::LevelFilter;

use crate::common;
use common::{TestStorage, alloc_tcp_port, login_when_allowed, wait_for_listener};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> anyhow::Result<Self> {
        let unique = hex::encode(rand::random::<[u8; 8]>());
        let path = std::env::temp_dir().join(format!("r3v3rs3-{name}-{unique}"));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A random id. `ShortId::new()` is the zero id, the same in every test.
fn random_id() -> ShortId {
    ShortId::from(rand::random::<[u8; 7]>())
}

/// The accounts of the tests. `scoped` sees only a proxy that no entry of the tests belongs to;
/// `editor` sees every proxy but is no admin; `viewer` only reads.
fn accounts() -> common::TestStorageBuilder {
    TestStorage::builder()
        .account("admin", "password", Role::Admin, None)
        .account("editor", "password", Role::Editor, None)
        .account("viewer", "password", Role::Viewer, None)
        .account(
            "scoped",
            "password",
            Role::Editor,
            Some(std::collections::BTreeSet::from([random_id()])),
        )
}

async fn with_admin<F, O>(dir: &Path, func: F) -> anyhow::Result<()>
where
    F: FnOnce(SocketAddr) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    with_admin_storage(dir, accounts().build(), func).await
}

async fn with_admin_storage<F, O>(dir: &Path, storage: TestStorage, func: F) -> anyhow::Result<()>
where
    F: FnOnce(SocketAddr) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    let addr = alloc_tcp_port().await?.socket_addr();
    let (server, channels) = Server::new(new_appinfo(dir, dir), storage).await;
    let event = channels.event.clone();
    let task = tokio::spawn(server.start());
    tokio::spawn(start_admin(
        new_appinfo(dir, dir),
        addr,
        channels.command,
        channels.callback,
        channels.event.clone(),
        channels.accounts.clone(),
    ));
    wait_for_listener(addr).await?;
    let result = func(addr).await;
    event.send(r3v3rs3_api::event::ServerEvent::Shutdown)?;
    task.await??;
    result
}

/// The access log is one store for the whole process, and the proxy tests of the other files
/// write into it while this test runs. Every entry of this test carries its own resource id,
/// and every query filters on it.
fn sample_log(
    resource: ShortId,
    status: u16,
    uri: &str,
    method: &str,
    country: Option<&str>,
) -> AccessLogEntry {
    AccessLogEntry {
        id: 0,
        timestamp: OffsetDateTime::now_utc(),
        resource_id: Some(resource),
        client_ip: "10.0.0.42".to_string(),
        method: method.to_string(),
        host: "app.example.com".to_string(),
        uri: uri.to_string(),
        status,
        latency_ms: 15.2,
        proto: "HTTP/1.1".to_string(),
        error: None,
        country: country.map(ToString::to_string),
        request_headers: vec![r3v3rs3_api::traffic::HeaderEntry::new("X-Test", "value")],
        response_headers: vec![r3v3rs3_api::traffic::HeaderEntry::new(
            "Content-Type",
            "application/json",
        )],
        request_body: Some("{\"test\":true}".to_string()),
        response_body: None,
        content_type: Some("application/json".to_string()),
        is_websocket: Some(false),
    }
}

async fn verify_list(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    resource: ShortId,
) -> anyhow::Result<()> {
    let unauth = client
        .get(format!("http://{addr}/api/traffic"))
        .send()
        .await?;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let resp = client
        .get(format!("http://{addr}/api/traffic?resource_id={resource}"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let items: Vec<AccessLogEntry> = resp.json().await?;
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].status, 500);
    Ok(())
}

async fn verify_filters_and_sse(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    resource: ShortId,
) -> anyhow::Result<()> {
    let filtered_status: Vec<AccessLogEntry> = client
        .get(format!(
            "http://{addr}/api/traffic?resource_id={resource}&status=404"
        ))
        .header(COOKIE, cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(filtered_status.len(), 1);
    assert_eq!(filtered_status[0].uri, "/unknown/endpoint");

    let filtered_search: Vec<AccessLogEntry> = client
        .get(format!(
            "http://{addr}/api/traffic?resource_id={resource}&search=health"
        ))
        .header(COOKIE, cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(filtered_search.len(), 1);

    let sse = client
        .get(format!("http://{addr}/api/traffic/live"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(sse.status(), StatusCode::OK);
    Ok(())
}

async fn verify_geo(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    resource: ShortId,
) -> anyhow::Result<()> {
    let unauth = client
        .get(format!("http://{addr}/api/traffic/geo"))
        .send()
        .await?;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let geo: r3v3rs3_api::traffic::TrafficGeoDistribution = client
        .get(format!(
            "http://{addr}/api/traffic/geo?resource_id={resource}"
        ))
        .header(COOKIE, cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(geo.total_resolved, 2);
    assert_eq!(geo.top_countries.len(), 1);
    assert_eq!(geo.top_countries[0].country, "TR");
    assert_eq!(geo.top_countries[0].count, 2);
    Ok(())
}

async fn verify_get_entry_and_replay(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    resource: ShortId,
) -> anyhow::Result<()> {
    let items: Vec<AccessLogEntry> = client
        .get(format!("http://{addr}/api/traffic?resource_id={resource}"))
        .header(COOKIE, cookie)
        .send()
        .await?
        .json()
        .await?;
    let first = &items[0];
    let single: AccessLogEntry = client
        .get(format!("http://{addr}/api/traffic/{}", first.id))
        .header(COOKIE, cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(single.id, first.id);
    assert_eq!(single.request_headers.len(), 1);
    assert_eq!(single.request_body.as_deref(), Some("{\"test\":true}"));

    let not_found = client
        .get(format!("http://{addr}/api/traffic/999999"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(not_found.status(), StatusCode::NOT_FOUND);

    let replay_req = r3v3rs3_api::traffic::RequestReplayRequest {
        method: "GET".to_string(),
        url: format!("http://{addr}/api/traffic?resource_id={resource}"),
        headers: vec![r3v3rs3_api::traffic::HeaderEntry::new("Cookie", cookie)],
        body: None,
        timeout_ms: Some(5000),
        allow_internal: true,
        insecure_skip_verify: false,
    };
    let replay_resp: r3v3rs3_api::traffic::RequestReplayResponse = client
        .post(format!("http://{addr}/api/traffic/replay"))
        .header(COOKIE, cookie)
        .json(&replay_req)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(replay_resp.status, 200);
    assert!(replay_resp.latency_ms > 0.0);
    assert!(replay_resp.body.contains("app.example.com"));
    Ok(())
}

#[tokio::test]
async fn traffic_api_lists_and_filters_logs() -> anyhow::Result<()> {
    let dir = TempDir::new("traffic-test")?;
    with_admin(&dir.0, |addr| async move {
        let resource = random_id();
        record(sample_log(
            resource,
            200,
            "/api/v1/health",
            "GET",
            Some("TR"),
        ));
        record(sample_log(
            resource,
            404,
            "/unknown/endpoint",
            "GET",
            Some("TR"),
        ));
        record(sample_log(resource, 500, "/api/v1/crash", "POST", None));

        let client = Client::new();
        verify_as_admin(&client, addr, resource).await?;
        verify_other_accounts(&client, addr, resource).await
    })
    .await
}

/// Runs the checks of the admin, who sees every entry.
async fn verify_as_admin(
    client: &Client,
    addr: SocketAddr,
    resource: ShortId,
) -> anyhow::Result<()> {
    let cookie = login_when_allowed(addr, "admin", "password").await?;
    verify_list(client, addr, &cookie, resource).await?;
    verify_filters_and_sse(client, addr, &cookie, resource).await?;
    verify_geo(client, addr, &cookie, resource).await?;
    verify_get_entry_and_replay(client, addr, &cookie, resource).await?;
    verify_active_connections(client, addr, &cookie).await
}

/// Runs the checks of the accounts that are no admin.
async fn verify_other_accounts(
    client: &Client,
    addr: SocketAddr,
    resource: ShortId,
) -> anyhow::Result<()> {
    let scoped = login_when_allowed(addr, "scoped", "password").await?;
    verify_scoped_account(client, addr, &scoped, resource).await?;
    let editor = login_when_allowed(addr, "editor", "password").await?;
    verify_editor_cannot_replay(client, addr, &editor).await
}

/// An account with a proxy list must not read the requests of another proxy, and must not learn
/// that the proxy exists, so every way to its entries answers as for a missing resource.
async fn verify_scoped_account(
    client: &Client,
    addr: SocketAddr,
    scoped: &str,
    resource: ShortId,
) -> anyhow::Result<()> {
    let get = |path: String| {
        client
            .get(format!("http://{addr}{path}"))
            .header(COOKIE, scoped)
    };
    let items: Vec<AccessLogEntry> = get(format!("/api/traffic?resource_id={resource}"))
        .send()
        .await?
        .json()
        .await?;
    assert!(items.is_empty(), "{items:?}");

    let query = r3v3rs3_api::traffic::AccessLogQuery {
        resource_id: Some(resource),
        ..Default::default()
    };
    let entry = r3v3rs3::proxy::access_log::recent(&query)
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no entry of {resource}"))?;
    let status = get(format!("/api/traffic/{}", entry.id))
        .send()
        .await?
        .status();
    assert_eq!(status, StatusCode::NOT_FOUND);
    let status = get(format!("/api/traffic/geo?resource_id={resource}"))
        .send()
        .await?
        .status();
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

/// Only an admin replays a request: the replay reaches internal addresses with the headers that
/// the caller chooses. An editor without a proxy list holds every other edit right.
async fn verify_editor_cannot_replay(
    client: &Client,
    addr: SocketAddr,
    editor: &str,
) -> anyhow::Result<()> {
    let replay = r3v3rs3_api::traffic::RequestReplayRequest {
        method: "GET".to_string(),
        url: format!("http://{addr}/healthz"),
        headers: Vec::new(),
        body: None,
        timeout_ms: Some(5000),
        allow_internal: true,
        insecure_skip_verify: false,
    };
    let status = client
        .post(format!("http://{addr}/api/traffic/replay"))
        .header(COOKIE, editor)
        .json(&replay)
        .send()
        .await?
        .status();
    assert_eq!(status, StatusCode::FORBIDDEN);
    Ok(())
}

/// Lists the active connections, of one proxy when `proxy_id` is set.
async fn connections(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    proxy_id: Option<ShortId>,
) -> anyhow::Result<Vec<r3v3rs3_api::traffic::ActiveConnectionInfo>> {
    let query = proxy_id
        .map(|id| format!("?proxy_id={id}"))
        .unwrap_or_default();
    let resp = client
        .get(format!("http://{addr}/api/traffic/connections{query}"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    Ok(resp.json().await?)
}

async fn verify_active_connections(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
) -> anyhow::Result<()> {
    use r3v3rs3::proxy::connection_tracker;
    use r3v3rs3_api::traffic::ConnectionProtocol;

    let unauth = client
        .get(format!("http://{addr}/api/traffic/connections"))
        .send()
        .await?;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    // The connection tracker is also one store for the whole process, so the checks filter
    // on the proxy id of this test.
    let proxy_id = random_id();
    assert!(
        connections(client, addr, cookie, Some(proxy_id))
            .await?
            .is_empty()
    );

    let (conn_id, guard, cancel_rx, sent, recv) = connection_tracker::register(
        Some(proxy_id),
        "test-ws",
        ConnectionProtocol::WebSocket,
        "192.168.1.50:54321",
        "10.0.0.5:8080",
    );
    sent.fetch_add(2048, std::sync::atomic::Ordering::Relaxed);
    recv.fetch_add(1024, std::sync::atomic::Ordering::Relaxed);

    // The list without a filter holds the connections of every test, so find this one by id.
    let items = connections(client, addr, cookie, None).await?;
    let item = items
        .iter()
        .find(|item| item.id == conn_id)
        .ok_or_else(|| anyhow::anyhow!("connection {conn_id} is not listed"))?;
    assert_eq!(item.protocol, ConnectionProtocol::WebSocket);
    assert_eq!(item.proxy_name, "test-ws");
    assert_eq!(item.bytes_sent, 2048);
    assert_eq!(item.bytes_received, 1024);
    assert_eq!(
        connections(client, addr, cookie, Some(proxy_id))
            .await?
            .len(),
        1
    );

    assert!(!*cancel_rx.borrow());
    let del_resp = client
        .delete(format!("http://{addr}/api/traffic/connections/{conn_id}"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(del_resp.status(), StatusCode::OK);
    assert!(*cancel_rx.borrow());

    drop(guard);
    let not_found_resp = client
        .delete(format!("http://{addr}/api/traffic/connections/{conn_id}"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(not_found_resp.status(), StatusCode::NOT_FOUND);

    Ok(())
}

/// Opens the live stream, records one entry of `resource`, and returns the stream text read
/// within `wait`.
async fn live_text(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    resource: ShortId,
    wait: std::time::Duration,
) -> anyhow::Result<String> {
    let mut resp = client
        .get(format!("http://{addr}/api/traffic/live"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    // The handler subscribes before it answers, so the entry is sent after the subscription.
    record(sample_log(resource, 200, "/live", "GET", None));
    let mut text = String::new();
    let deadline = tokio::time::Instant::now() + wait;
    while !text.contains(&resource.to_string()) {
        match tokio::time::timeout_at(deadline, resp.chunk()).await {
            Ok(Ok(Some(chunk))) => text.push_str(&String::from_utf8_lossy(&chunk)),
            _ => break,
        }
    }
    Ok(text)
}

/// The live stream follows the same scope as the list: an account with a proxy list gets no
/// entry of another proxy, while the admin gets it.
#[tokio::test]
async fn the_live_stream_keeps_the_scope_of_the_account() -> anyhow::Result<()> {
    let dir = TempDir::new("traffic-live")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let resource = random_id();
        let admin = login_when_allowed(addr, "admin", "password").await?;
        let wait = std::time::Duration::from_secs(5);
        let seen = live_text(&client, addr, &admin, resource, wait).await?;
        assert!(seen.contains(&resource.to_string()), "{seen}");

        let scoped = login_when_allowed(addr, "scoped", "password").await?;
        let wait = std::time::Duration::from_secs(1);
        let seen = live_text(&client, addr, &scoped, resource, wait).await?;
        assert!(!seen.contains(&resource.to_string()), "{seen}");
        Ok(())
    })
    .await
}

/// Every value below is a secret of a client; each carries `s3cr3t`, so one search finds a leak.
const SECRET_QUERY: &str = "/search?q=public&token=s3cr3t-query&pass%77ord=s3cr3t-encoded";
const SECRET_JSON: &str = r#"{"user":"alice","password":"s3cr3t-json"}"#;
const SECRET_FORM: &str = "user=alice&pass%77ord=s3cr3t-form";

/// Sends requests whose query string, headers and bodies carry secrets through the proxy.
async fn send_secrets(client: &Client, base: &url::Url) -> anyhow::Result<()> {
    client
        .get(base.join(SECRET_QUERY)?)
        .header("authorization", "Bearer s3cr3t-auth")
        .header(COOKIE, "session=s3cr3t-cookie")
        .header("x-upstream-token", "s3cr3t-header")
        .send()
        .await?;
    client
        .post(base.join("/login")?)
        .header("content-type", "application/json")
        .body(SECRET_JSON)
        .send()
        .await?;
    client
        .post(base.join("/form")?)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(SECRET_FORM)
        .send()
        .await?;
    Ok(())
}

/// The access log entries of the proxy `secrets`, read by an account with only Read permission.
async fn viewer_entries(client: &Client, addr: SocketAddr) -> anyhow::Result<String> {
    let viewer = login_when_allowed(addr, "viewer", "password").await?;
    let mut entries = String::new();
    for _ in 0..50 {
        let items: Vec<AccessLogEntry> = client
            .get(format!("http://{addr}/api/traffic?resource_id=secrets"))
            .header(COOKIE, &viewer)
            .send()
            .await?
            .json()
            .await?;
        entries = serde_json::to_string(&items)?;
        if items.len() == 3 {
            return Ok(entries);
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("the access log did not get the 3 requests: {entries}")
}

/// Every account with Read permission reads the access log, so a secret of a client must not
/// reach it: not in the query string, not in a header, not in a JSON or form body, also when the
/// name of the field is percent-encoded. The other values stay readable.
#[tokio::test]
async fn the_access_log_hides_the_secrets_of_a_request() -> anyhow::Result<()> {
    let dir = TempDir::new("traffic-secrets")?;
    let mut upstream = mockito::Server::new_async().await;
    for method in ["GET", "POST"] {
        upstream
            .mock(method, mockito::Matcher::Any)
            .expect_at_least(0)
            .create_async()
            .await;
    }
    let port = alloc_tcp_port().await?;
    let proxy = r3v3rs3_api::proxy::HttpProxy {
        routes: vec![common::http_route("/", &upstream.url(), None)],
        upgrade_insecure: false,
        ..Default::default()
    };
    let storage = accounts()
        .ports(vec![common::http_port_entry("p", &port)])
        .proxies(vec![common::http_proxy_entry("secrets", "p", proxy)])
        .build();

    with_admin_storage(&dir.0, storage, |addr| async move {
        let client = Client::new();
        send_secrets(&client, &port.http_url("/")).await?;
        let entries = viewer_entries(&client, addr).await?;
        assert!(!entries.contains("s3cr3t"), "{entries}");
        for kept in ["q=public", "alice", "/search"] {
            assert!(entries.contains(kept), "{kept} is missing: {entries}");
        }
        Ok(())
    })
    .await
}
