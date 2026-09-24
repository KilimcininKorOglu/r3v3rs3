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
const TARGETS: &str = "/api/targets";
const SECRET: &str = "s3cr3t-token-value";
const GIT_TOKEN: &str = "ghp_g1tT0kenValue";

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

    with_admin(&dir.0, storage, manage_apps).await?;

    // The key of the environment values is readable only by its owner.
    let mode = std::fs::metadata(dir.0.join("platform.key"))?
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    Ok(())
}

/// Signs in the three accounts and checks what each may do with an app.
async fn manage_apps(addr: SocketAddr) -> anyhow::Result<()> {
    let admin = session_cookie(addr, "admin", "admin-secret").await?;
    let scoped = session_cookie(addr, "scoped", "scoped-secret").await?;
    // The third sign-in waits for the login rate limit.
    let viewer = login_when_allowed(addr, "viewer", "viewer-secret").await?;

    check_read_access(addr, &viewer, &scoped).await?;
    let id = check_app_writes(addr, &admin).await?;
    let item = format!("{APPS}/{id}");
    check_env(addr, &admin, &viewer, &item).await?;
    check_secrets(addr, &admin, &viewer, &id).await?;
    check_deploy_access(addr, &viewer, &scoped, &item).await?;
    check_delete(addr, &admin, &viewer, &item).await?;
    check_audit(addr, &admin, &id).await
}

/// A viewer reads the targets but adds no app, and an account with a proxy list gets no access.
async fn check_read_access(addr: SocketAddr, viewer: &str, scoped: &str) -> anyhow::Result<()> {
    let targets = get(addr, "/api/targets", viewer).await?;
    assert_eq!(
        targets,
        json!([{"id": "local", "name": "local", "kind": "local", "enrolled": true, "online": true}])
    );
    let (status, _) = send(addr, Method::GET, APPS, scoped, None).await?;
    assert_eq!(status, 403);
    let body = Some(app("shop", "local"));
    let (status, _) = send(addr, Method::POST, APPS, viewer, body).await?;
    assert_eq!(status, 403);
    Ok(())
}

/// The platform runs without the agent port, so it adds no agent target.
async fn check_no_agent_port(addr: SocketAddr, admin: &str) -> anyhow::Result<()> {
    let body = Some(json!({"name": "edge"}));
    let (status, text) = send(addr, Method::POST, TARGETS, admin, body).await?;
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("agent_port_missing"), "{text}");
    Ok(())
}

/// Only an editor deletes the app, and a second deletion finds no app.
async fn check_delete(
    addr: SocketAddr,
    admin: &str,
    viewer: &str,
    item: &str,
) -> anyhow::Result<()> {
    let (status, _) = send(addr, Method::DELETE, item, viewer, None).await?;
    assert_eq!(status, 403);
    let (status, _) = send(addr, Method::DELETE, item, admin, None).await?;
    assert_eq!(status, 200);
    let (status, _) = send(addr, Method::DELETE, item, admin, None).await?;
    assert_eq!(status, 404);
    assert_eq!(get(addr, APPS, admin).await?, json!([]));
    Ok(())
}

/// Adds the app `shop` after the rejected requests, and returns its id.
async fn check_app_writes(addr: SocketAddr, admin: &str) -> anyhow::Result<String> {
    check_no_agent_port(addr, admin).await?;
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

/// The Git token and the webhook secret of the app.
async fn check_secrets(
    addr: SocketAddr,
    admin: &str,
    viewer: &str,
    id: &str,
) -> anyhow::Result<()> {
    check_git_token(addr, admin, viewer, &format!("{APPS}/{id}")).await?;
    let secret = create_webhook_secret(addr, admin, viewer, id).await?;
    check_hook_requests(addr, admin, id, &secret).await
}

/// The Git token needs the Edit permission, and no response returns it.
async fn check_git_token(
    addr: SocketAddr,
    admin: &str,
    viewer: &str,
    item: &str,
) -> anyhow::Result<()> {
    let path = format!("{item}/git_token");
    let body = json!({"token": GIT_TOKEN});
    let (status, _) = send(addr, Method::PUT, &path, viewer, Some(body.clone())).await?;
    assert_eq!(status, 403);
    let (status, text) = send(addr, Method::PUT, &path, admin, Some(body)).await?;
    assert_eq!(status, 200, "{text}");
    assert!(!text.contains(GIT_TOKEN), "{text}");
    assert_eq!(get(addr, item, viewer).await?["git_token_set"], true);

    let spaced = Some(json!({"token": "two words"}));
    let (status, _) = send(addr, Method::PUT, &path, admin, spaced).await?;
    assert_eq!(status, 400);
    let (status, text) = send(addr, Method::DELETE, &path, admin, None).await?;
    assert_eq!(status, 200, "{text}");
    assert_eq!(get(addr, item, viewer).await?["git_token_set"], false);
    Ok(())
}

/// Sends a webhook request without a session. `secret` signs the body like GitHub does.
async fn hook(
    addr: SocketAddr,
    id: &str,
    event: &str,
    secret: Option<&str>,
) -> anyhow::Result<(u16, String)> {
    use hmac::{Hmac, KeyInit, Mac};
    let body = r#"{"zen":"Keep it logically awesome."}"#;
    let mut request = reqwest::Client::new()
        .post(format!("http://{addr}/hooks/apps/{id}"))
        .header("x-github-event", event)
        .header("content-type", "application/json")
        .body(body);
    if let Some(secret) = secret {
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())?;
        mac.update(body.as_bytes());
        let signature = hex::encode(mac.finalize().into_bytes());
        request = request.header("x-hub-signature-256", format!("sha256={signature}"));
    }
    let response = request.send().await?;
    Ok((response.status().as_u16(), response.text().await?))
}

/// The webhook secret needs the Edit permission, and only its own response returns it.
async fn create_webhook_secret(
    addr: SocketAddr,
    admin: &str,
    viewer: &str,
    id: &str,
) -> anyhow::Result<String> {
    let item = format!("{APPS}/{id}");
    let path = format!("{item}/webhook_secret");
    let (status, _) = hook(addr, id, "ping", None).await?;
    assert_eq!(status, 404, "an app without a secret has no webhook");
    let (status, _) = send(addr, Method::POST, &path, viewer, None).await?;
    assert_eq!(status, 403);
    let (status, text) = send(addr, Method::POST, &path, admin, None).await?;
    assert_eq!(status, 200, "{text}");
    let created: Value = serde_json::from_str(&text)?;
    let secret = created["secret"].as_str().unwrap_or_default().to_string();
    assert_eq!(secret.len(), 64, "{text}");
    let entry = get(addr, &item, viewer).await?;
    assert_eq!(entry["webhook_secret_set"], true);
    assert!(!entry.to_string().contains(&secret), "{entry}");
    Ok(secret)
}

/// A hook request needs its signature instead of a session, and a deleted secret turns the
/// webhook off.
async fn check_hook_requests(
    addr: SocketAddr,
    admin: &str,
    id: &str,
    secret: &str,
) -> anyhow::Result<()> {
    let path = format!("{APPS}/{id}/webhook_secret");
    let (status, text) = hook(addr, id, "ping", None).await?;
    assert_eq!(status, 401, "{text}");
    let (status, text) = hook(addr, id, "ping", Some("wrong")).await?;
    assert_eq!(status, 401, "{text}");
    let (status, text) = hook(addr, id, "ping", Some(secret)).await?;
    assert_eq!(status, 200, "{text}");
    assert_eq!(
        serde_json::from_str::<Value>(&text)?,
        json!({"outcome": "ignored"})
    );

    let (status, text) = send(addr, Method::DELETE, &path, admin, None).await?;
    assert_eq!(status, 200, "{text}");
    assert_eq!(
        serde_json::from_str::<Value>(&text)?["webhook_secret_set"],
        false
    );
    let (status, _) = hook(addr, id, "ping", Some(secret)).await?;
    assert_eq!(status, 404);
    Ok(())
}

/// A viewer reads the app and its deployments, a deployment needs the Edit permission, and an
/// unknown deployment is not found.
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
    assert_eq!(get(addr, item, viewer).await?["name"], "shop");
    let deployments = get(addr, &format!("{item}/deployments"), viewer).await?;
    assert_eq!(deployments, json!([]));
    check_logs(addr, viewer, scoped, item).await
}

/// A viewer reads the log of an app without a running deployment, and an account with a proxy
/// list does not.
async fn check_logs(
    addr: SocketAddr,
    viewer: &str,
    scoped: &str,
    item: &str,
) -> anyhow::Result<()> {
    let logs = format!("{item}/logs?tail=5");
    let (status, _) = send(addr, Method::GET, &logs, scoped, None).await?;
    assert_eq!(status, 403);
    let log = get(addr, &logs, viewer).await?;
    assert_eq!(log, json!({"log": "", "running": false}));
    let (status, _) = send(
        addr,
        Method::GET,
        &format!("{APPS}/bcd-fgh/logs"),
        viewer,
        None,
    )
    .await?;
    assert_eq!(status, 404);
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
        [
            "delete_app",
            "delete_app_webhook_secret",
            "new_app_webhook_secret",
            "delete_app_git_token",
            "set_app_git_token",
            "update_app_env",
            "update_app",
            "add_app"
        ]
    );
    assert_eq!(entries[1]["summary"], "shop");
    assert_eq!(entries[2]["summary"], "shop");
    assert_eq!(entries[5]["summary"], "MODE, TOKEN");
    assert!(entries.iter().all(|entry| entry["username"] == "admin"));
    let text = Value::Array(entries).to_string();
    assert!(
        !text.contains(SECRET) && !text.contains(GIT_TOKEN),
        "{text}"
    );
    Ok(())
}

#[tokio::test]
async fn an_admin_manages_the_agent_targets() -> anyhow::Result<()> {
    let dir = TempDir::new("platform-targets")?;
    let mut config = AppConfig::default();
    config.platform.enabled = true;
    config.platform.agent_port = Some(alloc_tcp_port().await?.socket_addr().port());
    let storage = TestStorage::builder()
        .config(config)
        .account("admin", "admin-secret", Role::Admin, None)
        .account("editor", "editor-secret", Role::Editor, None)
        .build();
    with_admin(&dir.0, storage, manage_targets).await
}

async fn manage_targets(addr: SocketAddr) -> anyhow::Result<()> {
    let admin = session_cookie(addr, "admin", "admin-secret").await?;
    let editor = session_cookie(addr, "editor", "editor-secret").await?;
    let (id, first) = check_target_add(addr, &admin, &editor).await?;
    let second = check_target_token(addr, &admin, &editor, &id).await?;
    assert_ne!(first, second);
    check_target_delete(addr, &admin, &editor, &id).await?;
    check_target_audit(addr, &admin, &id, &[&first, &second]).await
}

/// Only an admin adds a target, and the answer carries its token once.
async fn check_target_add(
    addr: SocketAddr,
    admin: &str,
    editor: &str,
) -> anyhow::Result<(String, String)> {
    let body = json!({"name": "edge"});
    let (status, _) = send(addr, Method::POST, TARGETS, editor, Some(body.clone())).await?;
    assert_eq!(status, 403);
    let (status, text) = send(addr, Method::POST, TARGETS, admin, Some(body.clone())).await?;
    assert_eq!(status, 200, "{text}");
    let added: Value = serde_json::from_str(&text)?;
    assert_eq!(added["target"]["kind"], "agent");
    assert_eq!(added["target"]["enrolled"], false);
    assert!(added["agent_port"].as_u64().is_some_and(|port| port > 0));
    let (status, text) = send(addr, Method::POST, TARGETS, admin, Some(body)).await?;
    assert_eq!(status, 409, "{text}");
    assert!(text.contains("target_name_exists"), "{text}");

    let listed = get(addr, TARGETS, editor).await?;
    assert_eq!(listed.as_array().map(Vec::len), Some(2));
    assert!(!listed.to_string().contains("token"), "{listed}");
    let id = added["target"]["id"].as_str().unwrap_or_default();
    let token = added["token"].as_str().unwrap_or_default();
    Ok((id.to_string(), token.to_string()))
}

/// Only an admin replaces the token of an agent target, and the local target has no token.
async fn check_target_token(
    addr: SocketAddr,
    admin: &str,
    editor: &str,
    id: &str,
) -> anyhow::Result<String> {
    let path = format!("{TARGETS}/{id}/token");
    let (status, _) = send(addr, Method::POST, &path, editor, None).await?;
    assert_eq!(status, 403);
    let local = format!("{TARGETS}/local/token");
    let (status, text) = send(addr, Method::POST, &local, admin, None).await?;
    assert_eq!(status, 403, "{text}");
    assert!(text.contains("target_read_only"), "{text}");
    let (status, text) = send(addr, Method::POST, &path, admin, None).await?;
    assert_eq!(status, 200, "{text}");
    let renewed: Value = serde_json::from_str(&text)?;
    Ok(renewed["token"].as_str().unwrap_or_default().to_string())
}

/// Only an admin deletes an agent target, and a second deletion finds no target.
async fn check_target_delete(
    addr: SocketAddr,
    admin: &str,
    editor: &str,
    id: &str,
) -> anyhow::Result<()> {
    let path = format!("{TARGETS}/{id}");
    let (status, _) = send(addr, Method::DELETE, &path, editor, None).await?;
    assert_eq!(status, 403);
    let (status, text) = send(addr, Method::DELETE, &path, admin, None).await?;
    assert_eq!(status, 200, "{text}");
    let (status, _) = send(addr, Method::DELETE, &path, admin, None).await?;
    assert_eq!(status, 404);
    let (status, _) = send(addr, Method::DELETE, "/api/targets/local", admin, None).await?;
    assert_eq!(status, 403);
    Ok(())
}

/// The audit log names the target changes and never a token.
async fn check_target_audit(
    addr: SocketAddr,
    admin: &str,
    id: &str,
    tokens: &[&str],
) -> anyhow::Result<()> {
    let entries = get(addr, &format!("/api/audit?resource_id={id}"), admin).await?;
    let entries = entries.as_array().cloned().unwrap_or_default();
    let actions = entries
        .iter()
        .map(|entry| entry["action"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(actions, ["delete_target", "new_target_token", "add_target"]);
    assert!(entries.iter().all(|entry| entry["summary"] == "edge"));
    let text = Value::Array(entries).to_string();
    assert!(tokens.iter().all(|token| !text.contains(token)), "{text}");
    Ok(())
}

const CONNECTIONS: &str = "/api/git/connections";
const CLIENT_SECRET: &str = "c1ient-s3cr3t-value";

#[tokio::test]
async fn an_admin_manages_the_git_connections() -> anyhow::Result<()> {
    let dir = TempDir::new("platform-connections")?;
    let mut config = AppConfig::default();
    config.platform.enabled = true;
    let storage = TestStorage::builder()
        .config(config)
        .account("admin", "admin-secret", Role::Admin, None)
        .account("editor", "editor-secret", Role::Editor, None)
        .build();
    with_admin(&dir.0, storage, manage_connections).await
}

async fn manage_connections(addr: SocketAddr) -> anyhow::Result<()> {
    let admin = session_cookie(addr, "admin", "admin-secret").await?;
    let editor = session_cookie(addr, "editor", "editor-secret").await?;
    let id = check_connection_add(addr, &admin, &editor).await?;
    check_authorization(addr, &admin, &editor, &id).await?;
    check_not_connected(addr, &editor, &id).await?;
    check_connected_app(addr, &admin, &editor, &id).await?;
    check_connection_update(addr, &admin, &editor, &id).await?;
    check_platform_settings(addr, &admin, &editor).await?;
    check_connection_audit(addr, &admin, &id).await
}

fn connection(name: &str, secret: Option<&str>) -> Value {
    let mut body = json!({
        "name": name,
        "provider": "gitea",
        "url": "https://git.example.com",
        "client_id": "client-id",
    });
    if let Some(secret) = secret {
        body["client_secret"] = json!(secret);
    }
    body
}

/// Only an admin adds a connection, an editor lists it, and no response holds its secret.
async fn check_connection_add(
    addr: SocketAddr,
    admin: &str,
    editor: &str,
) -> anyhow::Result<String> {
    let body = Some(connection("team", Some(CLIENT_SECRET)));
    let (status, _) = send(addr, Method::POST, CONNECTIONS, editor, body.clone()).await?;
    assert_eq!(status, 403);
    let no_secret = Some(connection("team", None));
    let (status, text) = send(addr, Method::POST, CONNECTIONS, admin, no_secret).await?;
    assert_eq!(status, 400, "{text}");
    assert!(text.contains("invalid_git_connection"), "{text}");
    let (status, text) = send(addr, Method::POST, CONNECTIONS, admin, body.clone()).await?;
    assert_eq!(status, 200, "{text}");
    let added: Value = serde_json::from_str(&text)?;
    assert_eq!(added["status"], "not_connected");
    let (status, text) = send(addr, Method::POST, CONNECTIONS, admin, body).await?;
    assert_eq!(status, 409, "{text}");
    assert!(text.contains("git_connection_name_exists"), "{text}");

    let listed = get(addr, CONNECTIONS, editor).await?;
    assert_eq!(listed.as_array().map(Vec::len), Some(1));
    assert!(!listed.to_string().contains(CLIENT_SECRET), "{listed}");
    Ok(added["id"].as_str().unwrap_or_default().to_string())
}

/// Only an admin starts an authorization, and the callback without a session needs a state that
/// an authorization issued.
async fn check_authorization(
    addr: SocketAddr,
    admin: &str,
    editor: &str,
    id: &str,
) -> anyhow::Result<()> {
    let path = format!("{CONNECTIONS}/{id}/authorize");
    let callback = format!("http://{addr}/oauth/git/callback");
    let body = json!({"redirect_uri": callback});
    let (status, _) = send(addr, Method::POST, &path, editor, Some(body.clone())).await?;
    assert_eq!(status, 403);
    let wrong = Some(json!({"redirect_uri": format!("http://{addr}/elsewhere")}));
    let (status, text) = send(addr, Method::POST, &path, admin, wrong).await?;
    assert_eq!(status, 422, "{text}");
    let (status, text) = send(addr, Method::POST, &path, admin, Some(body)).await?;
    assert_eq!(status, 200, "{text}");
    let started: Value = serde_json::from_str(&text)?;
    let url = started["url"].as_str().unwrap_or_default();
    assert!(
        url.starts_with("https://git.example.com/login/oauth/authorize?client_id=client-id&"),
        "{url}"
    );

    for (query, result) in [
        ("code=c&state=forged", "error=oauth_state_invalid"),
        (
            "error=access_denied&state=forged",
            "error=git_authorization_refused",
        ),
    ] {
        let response = reqwest::get(format!("{callback}?{query}")).await?;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["referrer-policy"], "no-referrer");
        let page = response.text().await?;
        assert!(
            page.contains(&format!("url=/git_connections?{result}")),
            "{page}"
        );
    }
    Ok(())
}

/// An editor lists repositories and branches, and a connection without tokens refuses both.
async fn check_not_connected(addr: SocketAddr, editor: &str, id: &str) -> anyhow::Result<()> {
    for path in [
        format!("{CONNECTIONS}/{id}/repositories?search=app"),
        format!("{CONNECTIONS}/{id}/branches?repository=team/app"),
    ] {
        let (status, text) = send(addr, Method::GET, &path, editor, None).await?;
        assert_eq!(status, 409, "{text}");
        assert!(text.contains("git_connection_not_connected"), "{text}");
    }
    let unsafe_name = format!("{CONNECTIONS}/{id}/branches?repository=../app");
    let (status, _) = send(addr, Method::GET, &unsafe_name, editor, None).await?;
    assert_eq!(status, 400);
    Ok(())
}

/// An editor adds an app with the connection. The app keeps the reason of its missing webhook,
/// and the connection cannot be deleted while the app uses it.
async fn check_connected_app(
    addr: SocketAddr,
    admin: &str,
    editor: &str,
    id: &str,
) -> anyhow::Result<()> {
    let mut body = app("site", "local");
    body["spec"]["source"] = json!({
        "type": "git",
        "repository": "https://git.example.com/team/site.git",
        "connection": id,
    });
    let (status, text) = send(addr, Method::POST, APPS, editor, Some(body)).await?;
    assert_eq!(status, 200, "{text}");
    let added: Value = serde_json::from_str(&text)?;
    assert_eq!(added["hook"]["repository"], "team/site");
    assert_eq!(added["hook"]["installed"], false);
    let app_id = added["id"].as_str().unwrap_or_default();

    let hook = format!("{APPS}/{app_id}/hook");
    let (status, text) = send(addr, Method::POST, &hook, editor, None).await?;
    assert_eq!(status, 409, "{text}");
    assert!(text.contains("public_url_missing"), "{text}");
    let item = format!("{CONNECTIONS}/{id}");
    let (status, text) = send(addr, Method::DELETE, &item, admin, None).await?;
    assert_eq!(status, 409, "{text}");
    assert!(text.contains("git_connection_in_use"), "{text}");
    let (status, text) = send(
        addr,
        Method::DELETE,
        &format!("{APPS}/{app_id}"),
        editor,
        None,
    )
    .await?;
    assert_eq!(status, 200, "{text}");
    Ok(())
}

/// Only an admin changes or deletes a connection, and an update keeps the secret.
async fn check_connection_update(
    addr: SocketAddr,
    admin: &str,
    editor: &str,
    id: &str,
) -> anyhow::Result<()> {
    let item = format!("{CONNECTIONS}/{id}");
    let body = Some(connection("renamed", None));
    let (status, _) = send(addr, Method::PUT, &item, editor, body.clone()).await?;
    assert_eq!(status, 403);
    let (status, text) = send(addr, Method::PUT, &item, admin, body).await?;
    assert_eq!(status, 200, "{text}");
    assert_eq!(get(addr, &item, editor).await?["name"], "renamed");
    let (status, _) = send(addr, Method::DELETE, &item, editor, None).await?;
    assert_eq!(status, 403);
    let (status, text) = send(addr, Method::DELETE, &item, admin, None).await?;
    assert_eq!(status, 200, "{text}");
    let (status, _) = send(addr, Method::GET, &item, admin, None).await?;
    assert_eq!(status, 404);
    Ok(())
}

/// An editor reads the public address, and only an admin changes it.
async fn check_platform_settings(
    addr: SocketAddr,
    admin: &str,
    editor: &str,
) -> anyhow::Result<()> {
    let path = "/api/platform/settings";
    assert_eq!(get(addr, path, editor).await?, json!({}));
    let body = json!({"public_url": "https://deploy.example.com"});
    let (status, _) = send(addr, Method::PUT, path, editor, Some(body.clone())).await?;
    assert_eq!(status, 403);
    let invalid = Some(json!({"public_url": "https://deploy.example.com/"}));
    let (status, text) = send(addr, Method::PUT, path, admin, invalid).await?;
    assert_eq!(status, 422, "{text}");
    let (status, text) = send(addr, Method::PUT, path, admin, Some(body.clone())).await?;
    assert_eq!(status, 200, "{text}");
    assert_eq!(get(addr, path, editor).await?, body);
    Ok(())
}

/// The audit log names the connection and its provider, never the secret.
async fn check_connection_audit(addr: SocketAddr, admin: &str, id: &str) -> anyhow::Result<()> {
    let entries = get(addr, &format!("/api/audit?resource_id={id}"), admin).await?;
    let entries = entries.as_array().cloned().unwrap_or_default();
    let actions = entries
        .iter()
        .map(|entry| entry["action"].as_str().unwrap_or_default())
        .collect::<Vec<_>>();
    assert_eq!(
        actions,
        [
            "delete_git_connection",
            "update_git_connection",
            "add_git_connection"
        ]
    );
    assert_eq!(entries[0]["summary"], "renamed (gitea)");
    let text = Value::Array(entries).to_string();
    assert!(!text.contains(CLIENT_SECRET), "{text}");
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
