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
        verify_geo(&client, addr, &cookie).await
    })
    .await
}
