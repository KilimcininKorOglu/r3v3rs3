//! The Prometheus metrics endpoint: its authorization and its exported counters.

use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::auth::Role;
use reqwest::{Client, StatusCode};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{TestStorage, alloc_tcp_port, login_when_allowed, session_cookie, wait_for_listener};

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

/// Runs a server and its admin API.
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

/// Signs in as `admin` and returns the session cookie, waiting out a login rate limit.
async fn sign_in(addr: &SocketAddr) -> anyhow::Result<String> {
    match session_cookie(*addr, "admin", "password").await {
        Ok(cookie) => Ok(cookie),
        Err(err) if err.to_string().contains("429") => {
            login_when_allowed(*addr, "admin", "password").await
        }
        Err(err) => Err(err),
    }
}

#[tokio::test]
async fn the_metrics_need_a_session_or_a_bearer_token() -> anyhow::Result<()> {
    let dir = TempDir::new("metrics-auth")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let url = format!("http://{addr}/api/metrics");

        // A request without credentials is unauthorized.
        let res = client.get(&url).send().await?;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // A Bearer token that no grant issued is unauthorized too.
        let res = client
            .get(&url)
            .header("authorization", "Bearer not-a-token")
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // The admin session reads the metrics.
        let cookie = sign_in(&addr).await?;
        let res = client.get(&url).header("cookie", &cookie).send().await?;
        assert_eq!(res.status(), StatusCode::OK);
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap()
            .to_string();
        assert!(content_type.starts_with("text/plain"));
        let body = res.text().await?;
        for name in [
            "r3v3rs3_http_requests_total",
            "r3v3rs3_h3_requests_total",
            "r3v3rs3_upstream_errors_total",
            "r3v3rs3_cache_hits_total",
            "r3v3rs3_cache_misses_total",
            "r3v3rs3_open_connections",
        ] {
            assert!(
                body.contains(&format!("# TYPE {name} ")),
                "the metrics miss {name}"
            );
        }
        Ok(())
    })
    .await
}
