use r3v3rs3::{
    admin::start_admin,
    audit::{AuditFilter, AuditStore, SqliteAuditStore},
    clock::unix_ms,
    config::new_appinfo,
    log::DatabaseLayer,
    server::Server,
};
use r3v3rs3_api::{
    app::AppConfig,
    audit::{AuditAction, AuditEntry},
    auth::{LoginMethod, LoginRequest, Role},
    event::ServerEvent,
};
use reqwest::{Client, header::COOKIE};
use serde_json::json;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};
use tokio::sync::broadcast;
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

/// Creates the test directory and its log database.
async fn prepare_dir(name: &str) -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("r3v3rs3-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    DatabaseLayer::new(&dir.join("log.db"), LevelFilter::INFO).await?;
    Ok(dir)
}

/// A running server with its admin API.
struct Panel {
    addr: SocketAddr,
    event: broadcast::Sender<ServerEvent>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Panel {
    /// Starts the server and its admin API, and waits until the admin API listens.
    async fn start(dir: &Path, storage: TestStorage) -> anyhow::Result<Self> {
        let (server, channels) = Server::new(new_appinfo(dir, dir), storage).await;
        let event = channels.event.clone();
        let task = tokio::spawn(server.start());
        let addr = alloc_tcp_port().await?.socket_addr();
        tokio::spawn(start_admin(
            new_appinfo(dir, dir),
            addr,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
        wait_for_listener(addr).await?;
        Ok(Self { addr, event, task })
    }

    /// Shuts the server down and waits for it.
    async fn stop(self) -> anyhow::Result<()> {
        self.event.send(ServerEvent::Shutdown)?;
        self.task.await??;
        Ok(())
    }
}

/// Waits until the audit log holds at least `count` entries since `started`, and returns them.
async fn wait_for_entries(
    dir: &Path,
    started: u64,
    count: usize,
    failure: &str,
) -> anyhow::Result<Vec<AuditEntry>> {
    let store = SqliteAuditStore::new(dir.join("log.db"));
    let filter = AuditFilter {
        since: started,
        until: u64::MAX,
        username: None,
        resource_id: None,
        action: None,
        search: None,
        limit: 100,
    };
    let (store, filter) = (&store, &filter);
    wait_until(
        move || async move { Ok(store.query(filter).await?.len() >= count) },
        failure,
    )
    .await?;
    store.query(filter).await
}

#[tokio::test]
async fn the_audit_log_records_the_changes_and_the_sign_ins_of_an_account() -> anyhow::Result<()> {
    let dir = prepare_dir("audit-log").await?;
    let started = unix_ms();

    let storage = TestStorage::builder()
        .account("admin", "secret", Role::Admin, None)
        .build();
    let panel = Panel::start(&dir, storage).await?;

    let client = Client::new();
    change_and_sign_in(&client, panel.addr).await?;
    let entries =
        wait_for_entries(&dir, started, 4, "the audit log does not hold four entries").await?;
    check_stored_entries(&entries)?;
    check_audit_api(&client, panel.addr).await?;

    panel.stop().await
}

/// Adds an account, fails a sign-in and signs out, so the audit log records each step.
async fn change_and_sign_in(client: &Client, addr: SocketAddr) -> anyhow::Result<()> {
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
        .post(format!("http://{addr}/api/logout"))
        .header(COOKIE, &cookie)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// Checks the four stored entries, their loopback client and that no password is stored.
fn check_stored_entries(entries: &[AuditEntry]) -> anyhow::Result<()> {
    let found = rows(entries);
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
    assert!(!serde_json::to_string(entries)?.contains("viewer-secret"));
    Ok(())
}

/// Reads the audit entries from the admin API with a filter query.
async fn audit_entries(
    client: &Client,
    addr: SocketAddr,
    query: &str,
    cookie: &str,
) -> anyhow::Result<Vec<AuditEntry>> {
    Ok(client
        .get(format!("http://{addr}/api/audit{query}"))
        .header(COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// Checks that the admin API returns the entries to an admin, newest first, and not to a viewer.
async fn check_audit_api(client: &Client, addr: SocketAddr) -> anyhow::Result<()> {
    // The admin API returns the entries to an admin, newest first.
    let cookie = login_when_allowed(addr, "admin", "secret").await?;
    let newest = audit_entries(client, addr, "?username=admin&limit=2", &cookie).await?;
    assert_eq!(newest.len(), 2);
    assert!(newest[0].time >= newest[1].time);
    let added = audit_entries(client, addr, "?resource_id=viewer", &cookie).await?;
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
    let denied = client
        .get(format!("http://{addr}/api/audit"))
        .header(COOKIE, &viewer)
        .send()
        .await?;
    assert_eq!(denied.status(), 403);
    Ok(())
}

#[tokio::test]
async fn the_audit_log_records_the_forwarded_client_of_a_trusted_proxy() -> anyhow::Result<()> {
    let dir = prepare_dir("audit-proxy").await?;
    let started = unix_ms();

    // The panel sits behind r3v3rs3's own proxy on loopback, as on a production host.
    let mut config = AppConfig::default();
    config.admin.trusted_proxies = vec!["127.0.0.1/32".parse()?];
    let storage = TestStorage::builder()
        .config(config)
        .account("admin", "secret", Role::Admin, None)
        .build();
    let panel = Panel::start(&dir, storage).await?;

    send_forwarded_requests(&Client::new(), panel.addr).await?;
    let entries = wait_for_entries(
        &dir,
        started,
        3,
        "the audit log does not hold three entries",
    )
    .await?;
    // A session request and the sign-out record the address that the proxy forwarded.
    assert_eq!(
        client_of(&entries, AuditAction::AddAccount).as_deref(),
        Some("203.0.113.8")
    );
    assert_eq!(
        client_of(&entries, AuditAction::Logout).as_deref(),
        Some("203.0.113.9")
    );

    panel.stop().await
}

/// Adds an account and signs out with the forwarded client addresses of a trusted proxy.
async fn send_forwarded_requests(client: &Client, addr: SocketAddr) -> anyhow::Result<()> {
    let cookie = session_cookie(addr, "admin", "secret").await?;
    let account = json!({"username": "viewer", "password": "viewer-secret", "role": "viewer"});
    client
        .post(format!("http://{addr}/api/accounts"))
        .header(COOKIE, &cookie)
        .header("x-forwarded-for", "203.0.113.8")
        .json(&account)
        .send()
        .await?
        .error_for_status()?;
    client
        .post(format!("http://{addr}/api/logout"))
        .header(COOKIE, &cookie)
        .header("x-forwarded-for", "203.0.113.9")
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

/// The client address of the first entry with the action.
fn client_of(entries: &[AuditEntry], action: AuditAction) -> Option<String> {
    entries
        .iter()
        .find(|entry| entry.action == action)
        .and_then(|entry| entry.client)
        .map(|ip| ip.to_string())
}
