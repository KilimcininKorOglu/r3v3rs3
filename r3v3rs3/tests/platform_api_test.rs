//! The apps and the targets of the deployment platform through the admin API.

use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::Role;
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    TestStorage, alloc_tcp_port, login_when_allowed, send, session_cookie, wait_for_listener,
};

const APPS: &str = "/api/apps";
const SECRET: &str = "s3cr3t-token-value";

/// A config directory that is removed after the test.
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

/// Runs a server and its admin API with the config directory `dir`.
async fn with_admin<F, O>(dir: &Path, storage: TestStorage, func: F) -> anyhow::Result<()>
where
    F: FnOnce(SocketAddr) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
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

fn app(name: &str, target: &str) -> Value {
    json!({
        "name": name,
        "target": target,
        "spec": {
            "source": {"type": "image", "image": "nginx:1.27"},
            "port": 80,
            "domains": ["shop.example.com"],
        },
    })
}

async fn get(addr: SocketAddr, path: &str, cookie: &str) -> anyhow::Result<Value> {
    let (status, body) = send(addr, Method::GET, path, cookie, None).await?;
    anyhow::ensure!(status == 200, "{path}: {status} {body}");
    Ok(serde_json::from_str(&body)?)
}

#[tokio::test]
async fn an_editor_manages_apps_and_their_secrets_stay_hidden() -> anyhow::Result<()> {
    let dir = TempDir::new("platform-api")?;
    let mut config = AppConfig::default();
    config.platform.enabled = true;
    let storage = TestStorage::builder()
        .config(config)
        .account("admin", "admin-secret", Role::Admin, None)
        .account(
            "scoped",
            "scoped-secret",
            Role::Editor,
            Some(BTreeSet::new()),
        )
        .account("viewer", "viewer-secret", Role::Viewer, None)
        .build();

    with_admin(&dir.0, storage, |addr| async move {
        let admin = session_cookie(addr, "admin", "admin-secret").await?;
        let scoped = session_cookie(addr, "scoped", "scoped-secret").await?;
        // The third sign-in waits for the login rate limit.
        let viewer = login_when_allowed(addr, "viewer", "viewer-secret").await?;

        let targets = get(addr, "/api/targets", &viewer).await?;
        assert_eq!(
            targets,
            json!([{"id": "local", "name": "local", "kind": "local"}])
        );

        // An account with a proxy list gets no access, and a viewer only reads.
        let (status, _) = send(addr, Method::GET, APPS, &scoped, None).await?;
        assert_eq!(status, 403);
        let body = Some(app("shop", "local"));
        let (status, _) = send(addr, Method::POST, APPS, &viewer, body).await?;
        assert_eq!(status, 403);

        let id = check_app_writes(addr, &admin).await?;
        let item = format!("{APPS}/{id}");
        assert_eq!(get(addr, &item, &viewer).await?["name"], "shop");
        check_env(addr, &admin, &viewer, &item).await?;
        check_deploy_access(addr, &viewer, &scoped, &item).await?;
        assert_eq!(
            get(addr, &format!("{item}/deployments"), &viewer).await?,
            json!([])
        );

        let (status, _) = send(addr, Method::DELETE, &item, &viewer, None).await?;
        assert_eq!(status, 403);
        let (status, _) = send(addr, Method::DELETE, &item, &admin, None).await?;
        assert_eq!(status, 200);
        let (status, _) = send(addr, Method::DELETE, &item, &admin, None).await?;
        assert_eq!(status, 404);
        assert_eq!(get(addr, APPS, &admin).await?, json!([]));

        check_audit(addr, &admin, &id).await
    })
    .await?;

    // The key of the environment values is readable only by its owner.
    let mode = std::fs::metadata(dir.0.join("platform.key"))?
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    Ok(())
}

/// Adds the app `shop` after the rejected requests, and returns its id.
async fn check_app_writes(addr: SocketAddr, admin: &str) -> anyhow::Result<String> {
    let mut invalid = app("shop", "local");
    invalid["spec"]["port"] = json!(0);
    let (status, body) = send(addr, Method::POST, APPS, admin, Some(invalid)).await?;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("invalid_container_spec"), "{body}");
    let body = Some(app("shop", "nowhere"));
    let (status, _) = send(addr, Method::POST, APPS, admin, body).await?;
    assert_eq!(status, 404);

    let (status, body) = send(addr, Method::POST, APPS, admin, Some(app("shop", "local"))).await?;
    assert_eq!(status, 200, "{body}");
    let created: Value = serde_json::from_str(&body)?;
    let id = created["id"].as_str().unwrap_or_default().to_string();

    let (status, body) = send(addr, Method::POST, APPS, admin, Some(app("shop", "local"))).await?;
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("app_name_exists"), "{body}");

    let item = format!("{APPS}/{id}");
    let mut changed = app("shop", "local");
    changed["spec"]["port"] = json!(8080);
    let (status, body) = send(addr, Method::PUT, &item, admin, Some(changed)).await?;
    assert_eq!(status, 200, "{body}");
    let updated: Value = serde_json::from_str(&body)?;
    assert_eq!(updated["spec"]["port"], 8080);
    assert_eq!(updated["created_at"], created["created_at"]);
    Ok(id)
}

async fn check_env(addr: SocketAddr, admin: &str, viewer: &str, item: &str) -> anyhow::Result<()> {
    let env = format!("{item}/env");
    let body = json!([
        {"key": "MODE", "value": "production"},
        {"key": "TOKEN", "value": SECRET, "secret": true},
    ]);
    let (status, _) = send(addr, Method::PUT, &env, viewer, Some(body.clone())).await?;
    assert_eq!(status, 403);
    let (status, body) = send(addr, Method::PUT, &env, admin, Some(body)).await?;
    assert_eq!(status, 200, "{body}");

    let (status, _) = send(addr, Method::GET, &env, viewer, None).await?;
    assert_eq!(status, 403);
    let listed = get(addr, &env, admin).await?;
    assert_eq!(
        listed,
        json!([
            {"key": "MODE", "value": "production", "secret": false},
            {"key": "TOKEN", "secret": true},
        ])
    );
    Ok(())
}

/// A deployment needs the Edit permission, and an unknown deployment is not found.
async fn check_deploy_access(
    addr: SocketAddr,
    viewer: &str,
    scoped: &str,
    item: &str,
) -> anyhow::Result<()> {
    let deploy = format!("{item}/deploy");
    for cookie in [viewer, scoped] {
        let (status, _) = send(addr, Method::POST, &deploy, cookie, None).await?;
        assert_eq!(status, 403);
    }
    let (status, body) = send(addr, Method::GET, "/api/deployments/bcd-fgh", viewer, None).await?;
    assert_eq!(status, 404, "{body}");
    let rollback = "/api/deployments/bcd-fgh/rollback";
    let (status, _) = send(addr, Method::POST, rollback, viewer, None).await?;
    assert_eq!(status, 403);
    Ok(())
}

/// The audit log names the app changes and the changed keys, never a value.
async fn check_audit(addr: SocketAddr, admin: &str, id: &str) -> anyhow::Result<()> {
    let entries = get(addr, &format!("/api/audit?resource_id={id}"), admin).await?;
    let entries = entries.as_array().cloned().unwrap_or_default();
    let actions = entries
        .iter()
        .map(|entry| entry["action"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        actions,
        ["delete_app", "update_app_env", "update_app", "add_app"]
    );
    assert_eq!(entries[1]["summary"], "MODE, TOKEN");
    assert!(entries.iter().all(|entry| entry["username"] == "admin"));
    assert!(!Value::Array(entries).to_string().contains(SECRET));
    Ok(())
}

#[tokio::test]
async fn the_platform_answers_503_until_it_is_enabled() -> anyhow::Result<()> {
    let dir = TempDir::new("platform-off")?;
    let storage = TestStorage::builder()
        .account("admin", "admin-secret", Role::Admin, None)
        .build();
    with_admin(&dir.0, storage, |addr| async move {
        let admin = session_cookie(addr, "admin", "admin-secret").await?;
        let (status, body) = send(addr, Method::GET, APPS, &admin, None).await?;
        assert_eq!(status, 503);
        assert!(body.contains("platform_disabled"), "{body}");
        Ok(())
    })
    .await?;
    assert!(!dir.0.join("platform.db").exists());
    Ok(())
}
