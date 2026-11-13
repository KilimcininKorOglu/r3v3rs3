use r3v3rs3::admin::start_admin;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::server::Server;
use r3v3rs3_api::anomaly::{AnomalyAlertRules, SystemAnomaliesSnapshot};
use r3v3rs3_api::auth::Role;
use reqwest::header::{CONTENT_TYPE, COOKIE};
use reqwest::{Client, StatusCode};
use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{TestStorage, admin_session_cookie, alloc_tcp_port, wait_for_listener};

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
        .account("admin", "secret", Role::Admin, None)
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

#[tokio::test]
async fn test_anomaly_detection_endpoints() -> anyhow::Result<()> {
    let dir = TempDir::new("anomaly-test")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();

        // 1. Unauthorized access fails
        let unauth_res = client
            .get(format!("http://{addr}/api/system/anomalies"))
            .send()
            .await?;
        assert_eq!(unauth_res.status(), StatusCode::UNAUTHORIZED);

        // 2. Login as admin
        let cookie = admin_session_cookie(addr).await?;

        check_snapshot(&client, addr, &cookie).await?;
        let rules = check_read_rules(&client, addr, &cookie).await?;
        let new_rules = check_update_rules(&client, addr, &cookie, rules).await?;
        check_invalid_rules_and_dismiss(&client, addr, &cookie, new_rules).await
    })
    .await
}

/// Sends a GET request with the session cookie.
async fn get_with_cookie(
    client: &Client,
    addr: SocketAddr,
    path: &str,
    cookie: &str,
) -> anyhow::Result<reqwest::Response> {
    Ok(client
        .get(format!("http://{addr}{path}"))
        .header(COOKIE, cookie)
        .send()
        .await?)
}

/// Sends the rules in a PUT request with the session cookie.
async fn put_rules(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    rules: &AnomalyAlertRules,
) -> anyhow::Result<reqwest::Response> {
    Ok(client
        .put(format!("http://{addr}/api/system/anomalies/rules"))
        .header(COOKIE, cookie)
        .header(CONTENT_TYPE, "application/json")
        .json(rules)
        .send()
        .await?)
}

/// Checks that the snapshot carries the default rules.
async fn check_snapshot(client: &Client, addr: SocketAddr, cookie: &str) -> anyhow::Result<()> {
    // 3. GET /api/system/anomalies returns snapshot
    let res = get_with_cookie(client, addr, "/api/system/anomalies", cookie).await?;
    assert_eq!(res.status(), StatusCode::OK);
    let snapshot: SystemAnomaliesSnapshot = res.json().await?;
    assert!(snapshot.rules.enabled);
    assert_eq!(snapshot.rules.max_p99_latency_ms, Some(1000));
    assert_eq!(snapshot.rules.max_5xx_percent, Some(5));
    Ok(())
}

/// Checks that the rules endpoint returns the enabled rules, and returns them.
async fn check_read_rules(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
) -> anyhow::Result<AnomalyAlertRules> {
    // 4. GET /api/system/anomalies/rules
    let res = get_with_cookie(client, addr, "/api/system/anomalies/rules", cookie).await?;
    assert_eq!(res.status(), StatusCode::OK);
    let rules: AnomalyAlertRules = res.json().await?;
    assert!(rules.enabled);
    Ok(rules)
}

/// Checks that valid updated rules are saved and read back, and returns them.
async fn check_update_rules(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    rules: AnomalyAlertRules,
) -> anyhow::Result<AnomalyAlertRules> {
    // 5. PUT /api/system/anomalies/rules with valid updated rules
    let mut new_rules = rules;
    new_rules.max_p99_latency_ms = Some(1500);
    new_rules.max_5xx_percent = Some(15);

    let res = put_rules(client, addr, cookie, &new_rules).await?;
    assert_eq!(res.status(), StatusCode::OK);
    let updated: AnomalyAlertRules = res.json().await?;
    assert_eq!(updated.max_p99_latency_ms, Some(1500));
    assert_eq!(updated.max_5xx_percent, Some(15));

    // Verify persistence
    let res = get_with_cookie(client, addr, "/api/system/anomalies/rules", cookie).await?;
    let reloaded: AnomalyAlertRules = res.json().await?;
    assert_eq!(reloaded.max_p99_latency_ms, Some(1500));
    assert_eq!(reloaded.max_5xx_percent, Some(15));
    Ok(new_rules)
}

/// Checks that invalid rules fail with 400 and that the anomalies can be dismissed.
async fn check_invalid_rules_and_dismiss(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    new_rules: AnomalyAlertRules,
) -> anyhow::Result<()> {
    // 6. PUT invalid rules (5xx percentage > 100) returns 400
    let mut invalid_rules = new_rules;
    invalid_rules.max_5xx_percent = Some(120);
    let res = put_rules(client, addr, cookie, &invalid_rules).await?;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // 7. POST /api/system/anomalies/dismiss
    let res = client
        .post(format!("http://{addr}/api/system/anomalies/dismiss"))
        .header(COOKIE, cookie)
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    Ok(())
}
