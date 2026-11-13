use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::auth::Role;
use reqwest::Method;
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Duration};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    TestStorage, alloc_tcp_port, login_when_allowed, send, session_cookie, wait_for_listener,
    with_server,
};

#[tokio::test]
async fn an_admin_manages_the_accounts_through_the_admin_api() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-accounts-api-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;

    let addr = alloc_tcp_port().await?.socket_addr();
    let storage = TestStorage::builder()
        .account("admin", "secret", Role::Admin, None)
        .account("viewer", "viewer-secret", Role::Viewer, None)
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
        manage_the_accounts(addr).await
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

const ACCOUNTS_PATH: &str = "/api/accounts";

/// Signs the admin and the viewer in and runs every account check in order.
async fn manage_the_accounts(addr: SocketAddr) -> anyhow::Result<()> {
    let admin = session_cookie(addr, "admin", "secret").await?;
    let viewer = session_cookie(addr, "viewer", "viewer-secret").await?;

    check_session_and_viewer_access(addr, &admin, &viewer).await?;
    check_invalid_accounts(addr, &admin).await?;
    check_malformed_json(addr, &admin).await?;
    check_invalid_field_type(addr, &admin).await?;
    check_create_editor(addr, &admin).await?;
    check_account_list(addr, &admin).await?;
    check_own_account_rules(addr, &admin).await?;
    check_new_password_ends_sessions(addr, &admin, &viewer).await
}

/// Checks the session of the admin and that a viewer cannot read the accounts.
async fn check_session_and_viewer_access(
    addr: SocketAddr,
    admin: &str,
    viewer: &str,
) -> anyhow::Result<()> {
    let (status, body) = send(addr, Method::GET, "/api/session", admin, None).await?;
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_str::<Value>(&body)?,
        json!({"username": "admin", "role": "admin", "cert_expiry_warning": "14days", "display_timezone": "UTC"})
    );
    let (status, _) = send(addr, Method::GET, ACCOUNTS_PATH, viewer, None).await?;
    assert_eq!(status, 403);
    Ok(())
}

/// Checks that invalid new accounts fail with their error messages.
async fn check_invalid_accounts(addr: SocketAddr, admin: &str) -> anyhow::Result<()> {
    let invalid = [
        (
            json!({"username": "editor", "password": "short"}),
            "password_too_short",
        ),
        (
            json!({"username": "editor", "password": "long-enough", "proxies": ["web"]}),
            "invalid_account_scope",
        ),
        (
            json!({"username": "bad:name", "password": "long-enough"}),
            "invalid_username",
        ),
    ];
    for (account, message) in invalid {
        let (status, body) = send(addr, Method::POST, ACCOUNTS_PATH, admin, Some(account)).await?;
        assert_eq!(status, 400, "{body}");
        assert!(body.contains(message), "{body}");
    }
    Ok(())
}

/// Posts a raw JSON body to the accounts endpoint.
async fn post_raw_account(
    addr: SocketAddr,
    admin: &str,
    body: &'static str,
) -> anyhow::Result<reqwest::Response> {
    Ok(reqwest::Client::new()
        .post(format!("http://{addr}/api/accounts"))
        .header("cookie", admin)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await?)
}

/// Checks that malformed JSON syntax fails with 400 and a structured JSON error message.
async fn check_malformed_json(addr: SocketAddr, admin: &str) -> anyhow::Result<()> {
    // Malformed JSON syntax rejected with 400 and structured JSON ErrorMessage
    let res = post_raw_account(addr, admin, "{ invalid json syntax").await?;
    assert_eq!(res.status(), 400);
    let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("application/json"));
    let err: r3v3rs3_api::error::ErrorMessage = res.json().await?;
    assert_eq!(err.message, "Malformed or invalid JSON payload");
    assert!(err.request_id.is_some());
    Ok(())
}

/// Checks that an invalid field type fails with 422 and a structured JSON error message.
async fn check_invalid_field_type(addr: SocketAddr, admin: &str) -> anyhow::Result<()> {
    // Invalid field type rejected with 422 and structured JSON ErrorMessage without internal type leakage
    let res = post_raw_account(
        addr,
        admin,
        r#"{"username": 12345, "password": "valid-secret"}"#,
    )
    .await?;
    assert_eq!(res.status(), 422);
    let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
    assert!(ct.starts_with("application/json"));
    let err: r3v3rs3_api::error::ErrorMessage = res.json().await?;
    assert_eq!(
        err.message,
        "Invalid field format or value in request payload"
    );
    Ok(())
}

/// Creates an editor with TOTP and checks that a second account with the same name fails.
async fn check_create_editor(addr: SocketAddr, admin: &str) -> anyhow::Result<()> {
    let editor = json!({
        "username": "editor",
        "password": "editor-secret",
        "role": "editor",
        "proxies": ["web"],
        "totp": true,
    });
    let new_editor = Some(editor.clone());
    let (status, body) = send(addr, Method::POST, ACCOUNTS_PATH, admin, new_editor).await?;
    assert_eq!(status, 200, "{body}");
    let created: Value = serde_json::from_str(&body)?;
    let secret = created["totp_secret"].as_str().unwrap_or_default();
    assert!(!secret.is_empty(), "{body}");
    let (status, _) = send(addr, Method::POST, ACCOUNTS_PATH, admin, Some(editor)).await?;
    assert_eq!(status, 409);
    Ok(())
}

/// Checks the account list without the secrets.
async fn check_account_list(addr: SocketAddr, admin: &str) -> anyhow::Result<()> {
    let (status, body) = send(addr, Method::GET, ACCOUNTS_PATH, admin, None).await?;
    assert_eq!(status, 200);
    let mut accounts: Vec<Value> = serde_json::from_str(&body)?;
    accounts.sort_by_key(|account| account["username"].as_str().map(str::to_string));
    assert_eq!(
        accounts,
        vec![
            json!({"username": "admin", "role": "admin", "totp": false}),
            json!({"username": "editor", "role": "editor", "proxies": ["web"], "totp": true}),
            json!({"username": "viewer", "role": "viewer", "totp": false}),
        ]
    );
    Ok(())
}

/// Checks that an admin cannot lower its own role or delete itself, and that a missing account fails.
async fn check_own_account_rules(addr: SocketAddr, admin: &str) -> anyhow::Result<()> {
    // An admin cannot lower its own role or delete itself.
    let own = "/api/accounts/admin";
    let lower = Some(json!({"role": "viewer"}));
    let (status, body) = send(addr, Method::PUT, own, admin, lower).await?;
    assert_eq!(status, 400);
    assert!(body.contains("cannot_change_own_account"), "{body}");
    let (status, _) = send(addr, Method::DELETE, own, admin, None).await?;
    assert_eq!(status, 400);
    let missing = "/api/accounts/missing";
    let update = Some(json!({"role": "viewer"}));
    let (status, _) = send(addr, Method::PUT, missing, admin, update).await?;
    assert_eq!(status, 404);
    Ok(())
}

/// Checks that a new password and a deletion end the sessions that started before them.
async fn check_new_password_ends_sessions(
    addr: SocketAddr,
    admin: &str,
    viewer: &str,
) -> anyhow::Result<()> {
    // A new password ends the sessions that started before it.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let path = "/api/accounts/viewer";
    let update = Some(json!({"role": "viewer", "password": "viewer-new-secret"}));
    let (status, body) = send(addr, Method::PUT, path, admin, update).await?;
    assert_eq!(status, 200, "{body}");
    let (status, _) = send(addr, Method::GET, "/api/session", viewer, None).await?;
    assert_eq!(status, 401);
    let viewer = login_when_allowed(addr, "viewer", "viewer-new-secret").await?;
    let (status, _) = send(addr, Method::GET, "/api/session", &viewer, None).await?;
    assert_eq!(status, 200);

    let (status, _) = send(addr, Method::DELETE, path, admin, None).await?;
    assert_eq!(status, 200);
    let (status, _) = send(addr, Method::GET, "/api/session", &viewer, None).await?;
    assert_eq!(status, 401);
    Ok(())
}
