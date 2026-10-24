//! Integration tests for the real-time access log and traffic streaming API.

use r3v3rs3::admin::start_admin;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::proxy::access_log::{self, record};
use r3v3rs3::server::Server;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::traffic::AccessLogEntry;
use reqwest::header::COOKIE;
use reqwest::{Client, StatusCode};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use tracing_subscriber::filter::LevelFilter;

mod common;
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

async fn with_admin<F, O>(dir: &Path, func: F) -> anyhow::Result<()>
where
    F: FnOnce(SocketAddr) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    let storage = TestStorage::builder()
        .account("admin", "password", Role::Admin, None)
        .build();
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

fn sample_log(status: u16, uri: &str, method: &str, country: Option<&str>) -> AccessLogEntry {
    AccessLogEntry {
        id: 0,
        timestamp: OffsetDateTime::now_utc(),
        resource_id: Some("proxy-1".parse().unwrap()),
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

async fn verify_list(client: &Client, addr: SocketAddr, cookie: &str) -> anyhow::Result<()> {
    let unauth = client
        .get(format!("http://{addr}/api/traffic"))
        .send()
        .await?;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let resp = client
        .get(format!("http://{addr}/api/traffic"))
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
) -> anyhow::Result<()> {
    let filtered_status: Vec<AccessLogEntry> = client
        .get(format!("http://{addr}/api/traffic?status=404"))
        .header(COOKIE, cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(filtered_status.len(), 1);
    assert_eq!(filtered_status[0].uri, "/unknown/endpoint");

    let filtered_search: Vec<AccessLogEntry> = client
        .get(format!("http://{addr}/api/traffic?search=health"))
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

async fn verify_geo(client: &Client, addr: SocketAddr, cookie: &str) -> anyhow::Result<()> {
    let unauth = client
        .get(format!("http://{addr}/api/traffic/geo"))
        .send()
        .await?;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let geo: r3v3rs3_api::traffic::TrafficGeoDistribution = client
        .get(format!("http://{addr}/api/traffic/geo"))
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
) -> anyhow::Result<()> {
    let items: Vec<AccessLogEntry> = client
        .get(format!("http://{addr}/api/traffic"))
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
        url: format!("http://{addr}/api/traffic"),
        headers: vec![r3v3rs3_api::traffic::HeaderEntry::new("Cookie", cookie)],
        body: None,
        timeout_ms: Some(5000),
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
        access_log::clear();
        record(sample_log(200, "/api/v1/health", "GET", Some("TR")));
        record(sample_log(404, "/unknown/endpoint", "GET", Some("TR")));
        record(sample_log(500, "/api/v1/crash", "POST", None));

        let client = Client::new();
        let cookie = login_when_allowed(addr, "admin", "password").await?;
        verify_list(&client, addr, &cookie).await?;
        verify_filters_and_sse(&client, addr, &cookie).await?;
        verify_geo(&client, addr, &cookie).await?;
        verify_get_entry_and_replay(&client, addr, &cookie).await?;
        verify_active_connections(&client, addr, &cookie).await
    })
    .await
}

async fn verify_active_connections(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
) -> anyhow::Result<()> {
    use r3v3rs3::proxy::connection_tracker;
    use r3v3rs3_api::id::ShortId;
    use r3v3rs3_api::traffic::{ActiveConnectionInfo, ConnectionProtocol};

    let unauth = client
        .get(format!("http://{addr}/api/traffic/connections"))
        .send()
        .await?;
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let resp = client
        .get(format!("http://{addr}/api/traffic/connections"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let items: Vec<ActiveConnectionInfo> = resp.json().await?;
    assert!(items.is_empty());

    let proxy_id = ShortId::new();
    let (conn_id, guard, cancel_rx, sent, recv) = connection_tracker::register(
        Some(proxy_id),
        "test-ws",
        ConnectionProtocol::WebSocket,
        "192.168.1.50:54321",
        "10.0.0.5:8080",
    );
    sent.fetch_add(2048, std::sync::atomic::Ordering::Relaxed);
    recv.fetch_add(1024, std::sync::atomic::Ordering::Relaxed);

    let resp = client
        .get(format!("http://{addr}/api/traffic/connections"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let items: Vec<ActiveConnectionInfo> = resp.json().await?;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, conn_id);
    assert_eq!(items[0].protocol, ConnectionProtocol::WebSocket);
    assert_eq!(items[0].proxy_name, "test-ws");
    assert_eq!(items[0].bytes_sent, 2048);
    assert_eq!(items[0].bytes_received, 1024);

    let resp = client
        .get(format!(
            "http://{addr}/api/traffic/connections?proxy_id={proxy_id}"
        ))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(resp.status(), StatusCode::OK);
    let items: Vec<ActiveConnectionInfo> = resp.json().await?;
    assert_eq!(items.len(), 1);

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
