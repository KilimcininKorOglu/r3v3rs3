use mockito::Matcher;
use r3v3rs3::certs::Cert;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::{AppConfig, NotificationConfig, WebhookConfig};
use r3v3rs3_api::auth::Role;
use reqwest::Method;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{alloc_tcp_port, send, session_cookie, wait_for_listener, with_server, TestStorage};

const TOKEN: &str = "hook-token";

/// A storage with one CA certificate and a webhook that marks every certificate as expiring. The
/// background tasks run every second.
fn storage(url: &str, ca: Cert) -> TestStorage {
    let config = AppConfig {
        background_task_interval: Duration::from_secs(1),
        notifications: NotificationConfig {
            cert_expiry_warning: Duration::MAX,
            webhook: Some(WebhookConfig {
                url: url.into(),
                token: Some(TOKEN.into()),
                ..Default::default()
            }),
        },
        ..Default::default()
    };
    TestStorage::builder()
        .config(config)
        .certs(HashMap::from([(ca.id(), Arc::new(ca))]))
        .build()
}

#[tokio::test]
async fn the_webhook_gets_each_certificate_event_once() -> anyhow::Result<()> {
    let mut server = mockito::Server::new_async().await;
    let ca = Cert::new_ca()?;
    let body = json!({
        "event": "certificate_expiring",
        "certificate": {"id": ca.id().to_string()},
    });
    let mock = server
        .mock("POST", "/")
        .match_header("authorization", format!("Bearer {TOKEN}").as_str())
        .match_body(Matcher::PartialJson(body))
        .expect(1)
        .create_async()
        .await;

    with_server(storage(&server.url(), ca), |_| async move {
        // The leader checks the certificates in three background runs.
        tokio::time::sleep(Duration::from_millis(3500)).await;
        Ok(())
    })
    .await?;
    mock.assert_async().await;
    Ok(())
}

#[tokio::test]
async fn a_failed_notification_is_sent_three_times() -> anyhow::Result<()> {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/")
        .with_status(500)
        .expect(3)
        .create_async()
        .await;

    with_server(storage(&server.url(), Cert::new_ca()?), |_| async move {
        // The second attempt waits one second and the third waits two seconds.
        tokio::time::sleep(Duration::from_millis(5500)).await;
        Ok(())
    })
    .await?;
    mock.assert_async().await;
    Ok(())
}

#[tokio::test]
async fn an_admin_sends_a_test_notification_to_the_saved_webhook() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-notifications-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let mut server = mockito::Server::new_async().await;
    let url = server.url();
    let mock = server
        .mock("POST", "/")
        .match_header("authorization", format!("Bearer {TOKEN}").as_str())
        .match_body(Matcher::PartialJson(json!({"event": "test"})))
        .expect(1)
        .create_async()
        .await;

    let addr = alloc_tcp_port().await?.socket_addr();
    let storage = TestStorage::builder()
        .account("admin", "admin-secret", Role::Admin, None)
        .account("editor", "editor-secret", Role::Editor, None)
        .build();
    let app_info = new_appinfo(&dir, &dir);
    with_server(storage, |channels| async move {
        tokio::spawn(start_admin(
            app_info,
            addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
        wait_for_listener(addr).await?;
        let admin = session_cookie(addr, "admin", "admin-secret").await?;
        let editor = session_cookie(addr, "editor", "editor-secret").await?;
        let test = "/api/config/notifications/test";

        let (status, body) = send(addr, Method::POST, test, &admin, None).await?;
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("notification_webhook_missing"), "{body}");

        let mut config = serde_json::to_value(AppConfig::default())?;
        config["notifications"]["webhook"] = json!({"url": "http://hooks.example.com/"});
        let (status, body) = send(
            addr,
            Method::PUT,
            "/api/config",
            &admin,
            Some(config.clone()),
        )
        .await?;
        assert_eq!(status, 400, "{body}");
        assert!(body.contains("notification_webhook_url_invalid"), "{body}");

        // The admin API keeps the token and returns only whether it is set.
        config["notifications"]["webhook"] = json!({"url": url, "token": TOKEN});
        let (status, body) = send(
            addr,
            Method::PUT,
            "/api/config",
            &admin,
            Some(config.clone()),
        )
        .await?;
        assert_eq!(status, 200, "{body}");
        let (_, body) = send(addr, Method::GET, "/api/config", &admin, None).await?;
        let saved: Value = serde_json::from_str(&body)?;
        assert_eq!(
            saved["notifications"]["webhook"],
            json!({"url": url, "token_set": true, "timeout": "10s"})
        );

        let (status, _) = send(addr, Method::POST, test, &editor, None).await?;
        assert_eq!(status, 403);
        let (status, body) = send(addr, Method::POST, test, &admin, None).await?;
        assert_eq!(status, 200, "{body}");

        // A webhook that does not answer is a failed notification.
        config["notifications"]["webhook"] = json!({"url": "http://127.0.0.1:1/"});
        let (status, body) = send(addr, Method::PUT, "/api/config", &admin, Some(config)).await?;
        assert_eq!(status, 200, "{body}");
        let (status, body) = send(addr, Method::POST, test, &admin, None).await?;
        assert_eq!(status, 502, "{body}");
        assert!(body.contains("notification_failed"), "{body}");
        Ok(())
    })
    .await?;

    mock.assert_async().await;
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
