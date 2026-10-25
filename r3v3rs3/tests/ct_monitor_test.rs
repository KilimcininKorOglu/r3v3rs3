use r3v3rs3::admin::start_admin;
use r3v3rs3::config::new_appinfo;
use r3v3rs3::log::DatabaseLayer;
use r3v3rs3::server::Server;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::ct::{
    CtAlertStatus, CtLogEntry, CtMonitorConfig, CtMonitorStatus, DismissCtAlertRequest,
    UpdateCtConfigRequest,
};
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
async fn test_ct_config_and_status() -> anyhow::Result<()> {
    let dir = TempDir::new("ct-config")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = admin_session_cookie(addr).await?;

        // 1. Get initial CT config
        let res = client
            .get(format!("http://{addr}/api/certs/ct/config"))
            .header(COOKIE, cookie.clone())
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let config: CtMonitorConfig = res.json().await?;
        assert!(config.enabled);

        // 2. Update CT config
        let update_req = UpdateCtConfigRequest {
            config: CtMonitorConfig {
                enabled: true,
                check_interval_secs: 7200,
                monitored_domains: vec!["example.com".to_string(), "mycorp.net".to_string()],
                auto_discover_domains: false,
                allowed_issuers: vec!["Let's Encrypt".to_string(), "DigiCert".to_string()],
                alert_on_unknown_issuer: true,
                ct_api_url: None,
            },
        };

        let res = client
            .put(format!("http://{addr}/api/certs/ct/config"))
            .header(COOKIE, cookie.clone())
            .header(CONTENT_TYPE, "application/json")
            .json(&update_req)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let updated: CtMonitorConfig = res.json().await?;
        assert_eq!(updated.check_interval_secs, 7200);
        assert_eq!(updated.monitored_domains.len(), 2);

        // 3. Get status
        let res = client
            .get(format!("http://{addr}/api/certs/ct/status"))
            .header(COOKIE, cookie.clone())
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let status: CtMonitorStatus = res.json().await?;
        assert!(status.enabled);
        assert_eq!(status.monitored_domains.len(), 2);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn test_ct_check_and_dismiss_flow() -> anyhow::Result<()> {
    let dir = TempDir::new("ct-flow")?;

    // Start mock CT API server with mockito
    let mut mock_server = mockito::Server::new_async().await;
    let mock_body = serde_json::json!([
        {
            "id": 1001,
            "issuer_name": "C=US, O=Let's Encrypt, CN=R3",
            "common_name": "example.com",
            "name_value": "example.com\nwww.example.com",
            "serial_number": "03deadbeef01",
            "entry_timestamp": "2024-01-01T00:00:00",
            "not_before": "2024-01-01T00:00:00",
            "not_after": "2024-04-01T00:00:00"
        },
        {
            "id": 1002,
            "issuer_name": "C=CN, O=Untrusted Rogue CA, CN=Evil Root",
            "common_name": "example.com",
            "name_value": "example.com",
            "serial_number": "04deadbeef02",
            "entry_timestamp": "2024-01-02T00:00:00",
            "not_before": "2024-01-02T00:00:00",
            "not_after": "2024-04-02T00:00:00"
        }
    ]);

    let _mock = mock_server
        .mock("GET", "/?q=example.com&output=json")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(mock_body.to_string())
        .create_async()
        .await;

    let mock_url = mock_server.url();

    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = admin_session_cookie(addr).await?;

        // 1. Configure custom CT API URL pointing to our mock server
        let update_req = UpdateCtConfigRequest {
            config: CtMonitorConfig {
                enabled: true,
                check_interval_secs: 3600,
                monitored_domains: vec!["example.com".to_string()],
                auto_discover_domains: false,
                allowed_issuers: vec!["Let's Encrypt".to_string()],
                alert_on_unknown_issuer: true,
                ct_api_url: Some(format!("{mock_url}/?q={{domain}}&output=json")),
            },
        };

        let res = client
            .put(format!("http://{addr}/api/certs/ct/config"))
            .header(COOKIE, cookie.clone())
            .header(CONTENT_TYPE, "application/json")
            .json(&update_req)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);

        // 2. Trigger check
        let res = client
            .post(format!(
                "http://{addr}/api/certs/ct/check?domain=example.com"
            ))
            .header(COOKIE, cookie.clone())
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let count: usize = res.json().await?;
        assert_eq!(count, 1, "Should discover exactly 1 rogue cert");

        // 3. List entries
        let res = client
            .get(format!("http://{addr}/api/certs/ct/entries"))
            .header(COOKIE, cookie.clone())
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let entries: Vec<CtLogEntry> = res.json().await?;
        assert_eq!(entries.len(), 2);

        let rogue = entries.iter().find(|e| e.id == "1002").unwrap();
        assert!(rogue.is_rogue);
        assert_eq!(rogue.status, CtAlertStatus::Investigating);

        // 4. Check status shows 1 active rogue alert
        let res = client
            .get(format!("http://{addr}/api/certs/ct/status"))
            .header(COOKIE, cookie.clone())
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let status: CtMonitorStatus = res.json().await?;
        assert_eq!(status.active_rogue_alerts_count, 1);

        // 5. Dismiss alert
        let dismiss_req = DismissCtAlertRequest {
            id: "1002".to_string(),
            status: Some(CtAlertStatus::Dismissed),
        };
        let res = client
            .post(format!("http://{addr}/api/certs/ct/dismiss"))
            .header(COOKIE, cookie.clone())
            .header(CONTENT_TYPE, "application/json")
            .json(&dismiss_req)
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);

        // 6. Verify status shows 0 active rogue alerts
        let res = client
            .get(format!("http://{addr}/api/certs/ct/status"))
            .header(COOKIE, cookie.clone())
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::OK);
        let status: CtMonitorStatus = res.json().await?;
        assert_eq!(status.active_rogue_alerts_count, 0);

        Ok(())
    })
    .await
}
