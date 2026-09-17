use r3v3rs3::{
    admin::start_admin,
    audit::{AuditFilter, AuditStore, SqliteAuditStore},
    clock::unix_ms,
    config::new_appinfo,
    log::DatabaseLayer,
    server::Server,
};
use r3v3rs3_api::{
    audit::{AuditAction, AuditEntry},
    auth::{LoginMethod, LoginRequest, Role},
    event::ServerEvent,
};
use reqwest::{Client, header::COOKIE};
use serde_json::json;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    TestStorage, alloc_tcp_port, login_when_allowed, session_cookie, wait_for_listener, wait_until,
};

/// The action, the account, the resource id and the summary of each entry.
fn rows(entries: &[AuditEntry]) -> Vec<(AuditAction, &str, Option<&str>, &str)> {
    entries
        .iter()
        .map(|entry| {
            let id = entry.resource_id.as_deref();
            (
                entry.action,
                entry.username.as_str(),
                id,
                entry.summary.as_str(),
            )
        })
        .collect()
}

#[tokio::test]
async fn the_audit_log_records_the_changes_and_the_sign_ins_of_an_account() -> anyhow::Result<()> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-audit-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    let started = unix_ms();

    let storage = TestStorage::builder()
        .account("admin", "secret", Role::Admin, None)
        .build();
    let (server, channels) = Server::new(new_appinfo(&dir, &dir), storage).await;
    let event = channels.event.clone();
    let task = tokio::spawn(server.start());
    let addr = alloc_tcp_port().await?.socket_addr();
    tokio::spawn(start_admin(
        new_appinfo(&dir, &dir),
        addr,
        channels.command,
        channels.callback,
        channels.event.clone(),
        channels.accounts.clone(),
    ));
    wait_for_listener(addr).await?;

    let client = Client::new();
    let cookie = session_cookie(addr, "admin", "secret").await?;
    let account = json!({"username": "viewer", "password": "viewer-secret", "role": "viewer"});
    let added = client
        .post(format!("http://{addr}/api/accounts"))
        .header(COOKIE, &cookie)
        .json(&account)
        .send()
        .await?;
    assert_eq!(added.status(), 200);
    let wrong = LoginRequest {
        username: "admin".into(),
        method: LoginMethod::Password {
            password: "wrong".into(),
        },
        insecure: true,
    };
    let failed = client
        .post(format!("http://{addr}/api/login"))
        .json(&wrong)
        .send()
        .await?;
    assert_eq!(failed.status(), 400);
    client
        .get(format!("http://{addr}/api/logout"))
        .header(COOKIE, &cookie)
        .send()
        .await?
        .error_for_status()?;

    let store = SqliteAuditStore::new(dir.join("log.db"));
    let filter = AuditFilter {
        since: started,
        until: u64::MAX,
        username: None,
        resource_id: None,
        limit: 100,
    };
    let (store, filter) = (&store, &filter);
    wait_until(
        move || async move { Ok(store.query(filter).await?.len() >= 4) },
        "the audit log does not hold four entries",
    )
    .await?;

    let entries = store.query(filter).await?;
    let found = rows(&entries);
    assert_eq!(found.len(), 4, "{found:?}");
    let expected = [
        (AuditAction::Login, "admin", None, ""),
        (
            AuditAction::AddAccount,
            "admin",
            Some("viewer"),
            "role=viewer",
        ),
        (AuditAction::LoginFailed, "admin", None, ""),
        (AuditAction::Logout, "admin", None, ""),
    ];
    for row in expected {
        assert!(found.contains(&row), "{row:?} is missing in {found:?}");
    }
    assert!(
        entries
            .iter()
            .all(|entry| entry.client.is_some_and(|ip| ip.is_loopback()))
    );
    assert!(!serde_json::to_string(&entries)?.contains("viewer-secret"));

    // The admin API returns the entries to an admin, newest first.
    let cookie = login_when_allowed(addr, "admin", "secret").await?;
    let audit = |query: &str, cookie: &str| {
        client
            .get(format!("http://{addr}/api/audit{query}"))
            .header(COOKIE, cookie)
            .send()
    };
    let newest: Vec<AuditEntry> = audit("?username=admin&limit=2", &cookie)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(newest.len(), 2);
    assert!(newest[0].time >= newest[1].time);
    let added: Vec<AuditEntry> = audit("?resource_id=viewer", &cookie)
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(
        rows(&added),
        [(
            AuditAction::AddAccount,
            "admin",
            Some("viewer"),
            "role=viewer"
        )]
    );
    let viewer = login_when_allowed(addr, "viewer", "viewer-secret").await?;
    assert_eq!(audit("", &viewer).await?.status(), 403);

    event.send(ServerEvent::Shutdown)?;
    task.await??;
    Ok(())
}
