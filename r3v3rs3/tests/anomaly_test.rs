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

        // 3. GET /api/system/anomalies returns snapshot
        let res = client
            .get(format!("http://{addr}/api/system/anomalies"))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let snapshot: SystemAnomaliesSnapshot = res.json().await?;
        assert!(snapshot.rules.enabled);
        assert_eq!(snapshot.rules.max_p99_latency_ms, Some(1000));
        assert_eq!(snapshot.rules.max_5xx_percent, Some(5));

        // 4. GET /api/system/anomalies/rules
        let res = client
            .get(format!("http://{addr}/api/system/anomalies/rules"))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let rules: AnomalyAlertRules = res.json().await?;
        assert!(rules.enabled);

        // 5. PUT /api/system/anomalies/rules with valid updated rules
        let mut new_rules = rules;
        new_rules.max_p99_latency_ms = Some(1500);
        new_rules.max_5xx_percent = Some(15);

        let res = client
            .put(format!("http://{addr}/api/system/anomalies/rules"))
            .header(COOKIE, &cookie)
            .header(CONTENT_TYPE, "application/json")
            .json(&new_rules)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let updated: AnomalyAlertRules = res.json().await?;
        assert_eq!(updated.max_p99_latency_ms, Some(1500));
        assert_eq!(updated.max_5xx_percent, Some(15));

        // Verify persistence
        let res = client
            .get(format!("http://{addr}/api/system/anomalies/rules"))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        let reloaded: AnomalyAlertRules = res.json().await?;
        assert_eq!(reloaded.max_p99_latency_ms, Some(1500));
        assert_eq!(reloaded.max_5xx_percent, Some(15));

        // 6. PUT invalid rules (5xx percentage > 100) returns 400
        let mut invalid_rules = new_rules;
        invalid_rules.max_5xx_percent = Some(120);
        let res = client
            .put(format!("http://{addr}/api/system/anomalies/rules"))
            .header(COOKIE, &cookie)
            .header(CONTENT_TYPE, "application/json")
            .json(&invalid_rules)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // 7. POST /api/system/anomalies/dismiss
        let res = client
            .post(format!("http://{addr}/api/system/anomalies/dismiss"))
            .header(COOKIE, &cookie)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);

        Ok(())
    })
    .await
}
