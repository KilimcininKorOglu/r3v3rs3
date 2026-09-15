use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::auth::Role;
use reqwest::Method;
use serde_json::{json, Value};
use std::time::Duration;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    alloc_tcp_port, login_when_allowed, send, session_cookie, wait_for_listener, with_server,
    TestStorage,
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
        let admin = session_cookie(addr, "admin", "secret").await?;
        let viewer = session_cookie(addr, "viewer", "viewer-secret").await?;
        let accounts_path = "/api/accounts";

        let (status, body) = send(addr, Method::GET, "/api/session", &admin, None).await?;
        assert_eq!(status, 200);
        assert_eq!(
            serde_json::from_str::<Value>(&body)?,
            json!({"username": "admin", "role": "admin", "cert_expiry_warning": "14days"})
        );
        let (status, _) = send(addr, Method::GET, accounts_path, &viewer, None).await?;
        assert_eq!(status, 403);

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
            let (status, body) =
                send(addr, Method::POST, accounts_path, &admin, Some(account)).await?;
            assert_eq!(status, 400, "{body}");
            assert!(body.contains(message), "{body}");
        }

        let editor = json!({
            "username": "editor",
            "password": "editor-secret",
            "role": "editor",
            "proxies": ["web"],
            "totp": true,
        });
        let new_editor = Some(editor.clone());
        let (status, body) = send(addr, Method::POST, accounts_path, &admin, new_editor).await?;
        assert_eq!(status, 200, "{body}");
        let created: Value = serde_json::from_str(&body)?;
        let secret = created["totp_secret"].as_str().unwrap_or_default();
        assert!(!secret.is_empty(), "{body}");
        let (status, _) = send(addr, Method::POST, accounts_path, &admin, Some(editor)).await?;
        assert_eq!(status, 409);

        let (status, body) = send(addr, Method::GET, accounts_path, &admin, None).await?;
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

        // An admin cannot lower its own role or delete itself.
        let own = "/api/accounts/admin";
        let lower = Some(json!({"role": "viewer"}));
        let (status, body) = send(addr, Method::PUT, own, &admin, lower).await?;
        assert_eq!(status, 400);
        assert!(body.contains("cannot_change_own_account"), "{body}");
        let (status, _) = send(addr, Method::DELETE, own, &admin, None).await?;
        assert_eq!(status, 400);
        let missing = "/api/accounts/missing";
        let update = Some(json!({"role": "viewer"}));
        let (status, _) = send(addr, Method::PUT, missing, &admin, update).await?;
        assert_eq!(status, 404);

        // A new password ends the sessions that started before it.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let path = "/api/accounts/viewer";
        let update = Some(json!({"role": "viewer", "password": "viewer-new-secret"}));
        let (status, body) = send(addr, Method::PUT, path, &admin, update).await?;
        assert_eq!(status, 200, "{body}");
        let (status, _) = send(addr, Method::GET, "/api/session", &viewer, None).await?;
        assert_eq!(status, 401);
        let viewer = login_when_allowed(addr, "viewer", "viewer-new-secret").await?;
        let (status, _) = send(addr, Method::GET, "/api/session", &viewer, None).await?;
        assert_eq!(status, 200);

        let (status, _) = send(addr, Method::DELETE, path, &admin, None).await?;
        assert_eq!(status, 200);
        let (status, _) = send(addr, Method::GET, "/api/session", &viewer, None).await?;
        assert_eq!(status, 401);
        Ok(())
    })
    .await?;

    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
