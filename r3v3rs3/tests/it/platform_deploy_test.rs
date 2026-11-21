//! A blue-green deployment against a real Docker Engine: deploy, redeploy, roll back and delete
//! an app through the admin API, and reach it through an r3v3rs3 proxy port.
//!
//! The test needs a Docker Engine. It reads `DOCKER_HOST` and falls back to
//! `unix:///var/run/docker.sock`. `make test-runtime-docker` runs it.

use anyhow::{Context as _, bail, ensure};
use r3v3rs3::build::{Revision, git};
use r3v3rs3::kv::http::{ApiClient, RESPONSE_TIMEOUT};
use r3v3rs3::runtime::ContainerRuntime;
use r3v3rs3::runtime::docker::DockerRuntime;
use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::auth::Role;
use r3v3rs3_api::container::{AppName, NetworkName};
use r3v3rs3_api::discovery::Endpoint;
use reqwest::Method;
use reqwest::header::HOST;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use tracing_subscriber::filter::LevelFilter;

use crate::common;
use common::{TestPort, TestStorage, alloc_tcp_port, port_entry, send, session_cookie};

/// A small image that serves HTTP on port 80 and prints `WHOAMI_NAME` in its answer.
const IMAGE: &str = "traefik/whoami:v1.10.3";

/// The repository of the same app, with a multi-stage Dockerfile, at the tag of [`IMAGE`].
const GIT_REPOSITORY: &str = "https://github.com/traefik/whoami.git";
const GIT_TAG: &str = "v1.10.3";
/// The commit of [`GIT_TAG`].
const GIT_COMMIT: &str = "dec1ed84e37648285d4ddfae911344483c77906b";

const DOMAIN: &str = "whoami.test";

/// A small image with a shell that serves HTTP on port 80 and writes its access log to stdout.
const SHELL_IMAGE: &str = "mirror.gcr.io/library/nginx:1.29-alpine";

/// The time a deployment may take, including the image pull or the build.
const DEPLOY_TIMEOUT: Duration = Duration::from_secs(600);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> anyhow::Result<Self> {
        let unique = hex::encode(rand::random::<[u8; 8]>());
        let path = std::env::temp_dir().join(format!("r3v3rs3-deploy-{unique}"));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn docker_host() -> String {
    std::env::var("DOCKER_HOST").unwrap_or("unix:///var/run/docker.sock".into())
}

fn api_client() -> anyhow::Result<ApiClient> {
    let endpoint = docker_host().parse::<Endpoint>()?;
    let tls = ClientConfig::builder()
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth();
    Ok(ApiClient::new(vec![endpoint], Arc::new(tls)))
}

fn runtime() -> anyhow::Result<DockerRuntime> {
    Ok(DockerRuntime::new(api_client()?))
}

struct Context {
    admin: SocketAddr,
    cookie: String,
    proxy: TestPort,
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn an_app_deploys_blue_green_and_rolls_back() -> anyhow::Result<()> {
    let source = json!({"type": "image", "image": IMAGE});
    with_app(source, |ctx, id| async move {
        deploy_and_roll_back(&ctx, &id).await
    })
    .await
}

#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_git_app_builds_its_image_and_deploys() -> anyhow::Result<()> {
    let source = json!({
        "type": "git",
        "repository": GIT_REPOSITORY,
        "branch": GIT_TAG,
    });
    with_app(source, |ctx, id| async move {
        build_and_delete(&ctx, &id).await
    })
    .await
}

/// Source: the CLI help of `r3 app open-terminal` ("an interactive shell in the container") and
/// `r3 app stream-logs` ("the container log of an app as it is written"). Commands piped into
/// the terminal must run in the container and their output must reach stdout before the client
/// ends; a size message must change the size of the shell; a new log line must reach stdout.
#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn the_client_runs_a_terminal_and_follows_the_logs() -> anyhow::Result<()> {
    let source = json!({"type": "image", "image": SHELL_IMAGE});
    with_app_checked(source, None, |ctx, id| async move {
        deploy(&ctx, &id, "shell").await?;
        wait_for_answer(&ctx, "Welcome to nginx").await?;
        let token = api_token(&ctx).await?;
        let binary = crate::mcp_oauth_test::client_binary()?;
        check_cli_terminal(&ctx, &binary, &token, &id).await?;
        check_terminal_resize(&ctx, &id).await?;
        check_cli_logs(&ctx, &binary, &token, &id).await?;
        delete_app(&ctx, &id).await
    })
    .await
}

/// An API token of the admin account with the edit scope, for the client binary.
async fn api_token(ctx: &Context) -> anyhow::Result<String> {
    let body = json!({"name": "e2e", "scope": "edit"});
    let created = call(ctx, Method::POST, "/api/auth/tokens", Some(body)).await?;
    created["token"]
        .as_str()
        .map(str::to_string)
        .context("token")
}

/// Pipes a command into `r3 app open-terminal`. The shell computes the number, so the echo of
/// the typed line cannot pass for the output.
async fn check_cli_terminal(
    ctx: &Context,
    binary: &Path,
    token: &str,
    id: &str,
) -> anyhow::Result<()> {
    let args = ["app", "open-terminal", id];
    let input = "echo r3-$((40+2))-ok\n";
    let run = crate::mcp_oauth_test::run_client_stdin(binary, &ctx.admin, token, &args, input);
    let out = tokio::time::timeout(Duration::from_secs(60), run)
        .await
        .context("the terminal client did not end after its input ended")??;
    let stdout = String::from_utf8_lossy(&out.stdout);
    ensure!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    ensure!(stdout.contains("r3-42-ok"), "the output missed: {stdout}");
    Ok(())
}

/// Sends a size message on the terminal WebSocket, then asks the shell for its size.
async fn check_terminal_resize(ctx: &Context, id: &str) -> anyhow::Result<()> {
    use futures::SinkExt;
    use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};
    let url = format!("ws://{}/api/apps/{id}/terminal?cols=80&rows=24", ctx.admin);
    let mut request = url.into_client_request()?;
    request.headers_mut().insert("cookie", ctx.cookie.parse()?);
    // A browser sends its origin; the panel refuses a cookie WebSocket without one (CSRF).
    let origin = format!("http://{}", ctx.admin);
    request.headers_mut().insert("origin", origin.parse()?);
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await?;
    let resize = json!({"cols": 100, "rows": 40}).to_string();
    socket.send(Message::Text(resize.into())).await?;
    socket
        .send(Message::Binary(b"stty size; exit\n".to_vec().into()))
        .await?;
    let output = tokio::time::timeout(Duration::from_secs(60), read_terminal(&mut socket))
        .await
        .context("the terminal did not end")??;
    ensure!(output.contains("40 100"), "stty answered: {output}");
    Ok(())
}

/// Collects the output of a terminal WebSocket until the server closes the session.
async fn read_terminal<S>(socket: &mut S) -> anyhow::Result<String>
where
    S: futures::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + Unpin,
{
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    let mut output = String::new();
    while let Some(message) = socket.next().await {
        match message? {
            Message::Binary(bytes) => output.push_str(&String::from_utf8_lossy(&bytes)),
            Message::Text(text) => output.push_str(&text),
            Message::Close(_) => break,
            _ => {}
        }
    }
    Ok(output)
}

/// Requests a marked path, then waits until `r3 app stream-logs` prints its access log line.
async fn check_cli_logs(ctx: &Context, binary: &Path, token: &str, id: &str) -> anyhow::Result<()> {
    use tokio::io::AsyncBufReadExt;
    let mut child = tokio::process::Command::new(binary)
        .arg(format!("http://{}", ctx.admin))
        .args(["--token", token, "app", "stream-logs", id, "--tail", "1"])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child.stdout.take().context("no stdout")?;
    let mut lines = tokio::io::BufReader::new(stdout).lines();
    let url = format!("http://{}/r3-log-marker", ctx.proxy.socket_addr());
    reqwest::Client::new()
        .get(&url)
        .header(HOST, DOMAIN)
        .send()
        .await?;
    let find = async {
        while let Some(line) = lines.next_line().await? {
            if line.contains("/r3-log-marker") {
                return anyhow::Ok(());
            }
        }
        bail!("the log stream ended without the marked request")
    };
    let found = tokio::time::timeout(Duration::from_secs(60), find).await;
    child.kill().await?;
    found.context("no log line of the marked request")?
}

/// A rollback of a Compose app fetches the commit of the old deployment, which no branch names.
#[tokio::test]
#[ignore = "needs a Docker Engine; run with make test-runtime-docker"]
async fn a_commit_checks_out_without_its_branch() -> anyhow::Result<()> {
    let dir = TempDir::new()?;
    let dest = dir.0.join("src");
    let repository = GIT_REPOSITORY.parse()?;
    let commit = GIT_COMMIT.parse()?;
    let sha = git::clone(&repository, Revision::Commit(&commit), None, &dest).await?;
    ensure!(sha == GIT_COMMIT, "{sha}");
    ensure!(dest.join("Dockerfile").is_file());
    Ok(())
}

/// Starts a server with the platform, adds an app with `source` and runs `check` on it. The
/// containers and the network of the app are removed after a failed check too.
async fn with_app<F, O>(source: Value, check: F) -> anyhow::Result<()>
where
    F: FnOnce(Context, String) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    with_app_checked(source, Some("/health"), check).await
}

/// [`with_app`] with the health check path of the app, `None` for no health check.
async fn with_app_checked<F, O>(
    source: Value,
    health: Option<&'static str>,
    check: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Context, String) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    let dir = TempDir::new()?;
    let proxy = alloc_tcp_port().await?;
    let mut config = AppConfig::default();
    config.platform.enabled = true;
    config.platform.docker = docker_host();
    // proxy_ports stays empty: an app without its own ports takes the ports of the config, or
    // every HTTP and HTTPS port when that list is also empty.
    config.platform.proxy_ports = Vec::new();
    let storage = TestStorage::builder()
        .config(config)
        .ports(vec![port_entry("apps", proxy.multiaddr_http())])
        .account("admin", "admin-secret", Role::Admin, None)
        .build();
    let app_id = Arc::new(std::sync::Mutex::new(None));
    let shared_id = app_id.clone();
    let result = with_server(&dir.0, storage, |admin| async move {
        let cookie = session_cookie(admin, "admin", "admin-secret").await?;
        let ctx = Context {
            admin,
            cookie,
            proxy,
        };
        let id = add_app(&ctx, source, health).await?;
        if let Ok(mut shared) = shared_id.lock() {
            *shared = Some(id.clone());
        }
        check(ctx, id).await
    })
    .await;
    let id = app_id.lock().ok().and_then(|id| id.clone());
    let cleanup = match id {
        Some(id) => remove_leftovers(&id).await,
        None => Ok(()),
    };
    result?;
    cleanup
}

/// Builds and deploys the Git app, then deletes it with its built image.
async fn build_and_delete(ctx: &Context, id: &str) -> anyhow::Result<()> {
    let deployment = deploy(ctx, id, "git").await?;
    wait_for_answer(ctx, "Name: git").await?;
    check_build(ctx, &deployment).await?;
    let tag = format!("r3v3rs3/{id}:{deployment}").parse()?;
    let runtime = runtime()?;
    ensure!(runtime.inspect_image(&tag).await?.is_some());
    delete_app(ctx, id).await?;
    ensure!(runtime.inspect_image(&tag).await?.is_none());
    Ok(())
}

/// Checks that the deployment records its commit and the id of its built image.
async fn check_build(ctx: &Context, deployment: &str) -> anyhow::Result<()> {
    let path = format!("/api/deployments/{deployment}");
    let entry = call(ctx, Method::GET, &path, None).await?;
    // The app deploys the branch `GIT_TAG`, so the build must record the commit of that tag.
    let sha = entry["commit_sha"].as_str().context("commit sha")?;
    ensure!(sha == GIT_COMMIT, "{entry}");
    let digest = entry["image_digest"].as_str().context("image digest")?;
    ensure!(digest.starts_with("sha256:"), "{entry}");
    Ok(())
}

async fn with_server<F, O>(dir: &Path, storage: TestStorage, func: F) -> anyhow::Result<()>
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
    common::wait_for_listener(addr).await?;
    let result = func(addr).await;
    event.send(r3v3rs3_api::event::ServerEvent::Shutdown)?;
    task.await??;
    result
}

async fn call(
    ctx: &Context,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> anyhow::Result<Value> {
    let (status, text) = send(ctx.admin, method, path, &ctx.cookie, body).await?;
    ensure!(status == 200, "{path}: {status} {text}");
    Ok(serde_json::from_str(&text)?)
}

async fn add_app(ctx: &Context, source: Value, health: Option<&str>) -> anyhow::Result<String> {
    let name = format!("e2e-{:08x}", rand::random::<u32>());
    let body = json!({
        "name": name,
        "target": "local",
        "spec": {
            "source": source,
            "port": 80,
            "domains": [DOMAIN],
            "health_check_path": health,
            "restart": "no",
        },
    });
    let app = call(ctx, Method::POST, "/api/apps", Some(body)).await?;
    app["id"].as_str().map(str::to_string).context("app id")
}

async fn deploy_and_roll_back(ctx: &Context, id: &str) -> anyhow::Result<()> {
    let blue = deploy(ctx, id, "blue").await?;
    wait_for_answer(ctx, "Name: blue").await?;
    // A blue-green deployment keeps the old container serving until the new one answers, so
    // no request fails during the switch.
    let probe = Probe::start(ctx.proxy.socket_addr());
    let green = deploy(ctx, id, "green").await?;
    ensure!(blue != green);
    wait_for_answer(ctx, "Name: green").await?;
    let failures = probe.stop().await?;
    ensure!(
        failures.is_empty(),
        "requests failed during the switch: {failures:?}"
    );
    wait_for_containers(id, 1).await?;
    roll_back(ctx, &blue).await?;
    wait_for_containers(id, 1).await?;
    delete_app(ctx, id).await
}

/// Rolls back to the deployment `blue`, which set `WHOAMI_NAME` to `blue`.
async fn roll_back(ctx: &Context, blue: &str) -> anyhow::Result<()> {
    let path = format!("/api/deployments/{blue}/rollback");
    let rollback = call_when_free(ctx, Method::POST, &path).await?;
    let rollback = rollback["id"].as_str().context("rollback id")?.to_string();
    let entry = wait_for_deployment(ctx, &rollback).await?;
    ensure!(entry["trigger"] == "rollback", "{entry}");
    wait_for_answer(ctx, "Name: blue").await?;

    let old = call(ctx, Method::GET, &format!("/api/deployments/{blue}"), None).await?;
    ensure!(old["status"] == "superseded", "{old}");
    ensure!(
        old["image_digest"] == entry["image_digest"],
        "{old} {entry}"
    );
    Ok(())
}

/// Deletes the app, and checks that its containers, its network and its route are gone.
async fn delete_app(ctx: &Context, id: &str) -> anyhow::Result<()> {
    call_when_free(ctx, Method::DELETE, &format!("/api/apps/{id}")).await?;
    wait_for_containers(id, 0).await?;
    let network: NetworkName = format!("r3v3rs3-{id}").parse()?;
    ensure!(
        !network_exists(&network).await?,
        "the network {network} stays"
    );
    wait_until_unrouted(ctx).await
}

/// Sets `WHOAMI_NAME`, deploys, waits until the deployment runs, and returns its id.
async fn deploy(ctx: &Context, id: &str, name: &str) -> anyhow::Result<String> {
    let env = json!([{"key": "WHOAMI_NAME", "value": name, "secret": true}]);
    let path = format!("/api/apps/{id}/env");
    let (status, text) = send(ctx.admin, Method::PUT, &path, &ctx.cookie, Some(env)).await?;
    ensure!(status == 200, "{text}");
    let path = format!("/api/apps/{id}/deploy");
    let entry = call_when_free(ctx, Method::POST, &path).await?;
    let deployment = entry["id"].as_str().context("deployment id")?.to_string();
    wait_for_deployment(ctx, &deployment).await?;
    Ok(deployment)
}

/// Sends a request that the running pipeline of the app refuses with 409 until it has removed
/// the old container.
async fn call_when_free(ctx: &Context, method: Method, path: &str) -> anyhow::Result<Value> {
    let deadline = tokio::time::Instant::now() + DEPLOY_TIMEOUT;
    loop {
        let (status, text) = send(ctx.admin, method.clone(), path, &ctx.cookie, None).await?;
        match status {
            200 => return Ok(serde_json::from_str(&text)?),
            409 if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            _ => bail!("{path}: {status} {text}"),
        }
    }
}

async fn wait_for_deployment(ctx: &Context, id: &str) -> anyhow::Result<Value> {
    let path = format!("/api/deployments/{id}");
    let deadline = tokio::time::Instant::now() + DEPLOY_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let entry = call(ctx, Method::GET, &path, None).await?;
        match entry["status"].as_str() {
            Some("running") => return Ok(entry),
            Some("failed" | "cancelled" | "superseded") => bail!("{entry}"),
            _ => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
    bail!("the deployment {id} did not run in time")
}

/// Waits until the proxy port answers with `expected` in the body.
async fn wait_for_answer(ctx: &Context, expected: &str) -> anyhow::Result<()> {
    let url = format!("http://{}/", ctx.proxy.socket_addr());
    let client = reqwest::Client::new();
    let mut last = String::new();
    for _ in 0..60 {
        let reply = client.get(&url).header(HOST, DOMAIN).send().await;
        if let Ok(reply) = reply {
            let status = reply.status();
            last = format!("{status} {}", reply.text().await.unwrap_or_default());
            if status == 200 && last.contains(expected) {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("the proxy did not answer {expected}: {last}")
}

/// Sends a request to the app every 50 ms and records every answer that is not 200.
struct Probe {
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<Vec<String>>,
}

impl Probe {
    fn start(proxy: SocketAddr) -> Self {
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(async move {
            let (url, client) = (format!("http://{proxy}/"), reqwest::Client::new());
            let mut failures = Vec::new();
            while !*stopped.borrow() {
                if let Some(failure) = probe_once(&client, &url).await {
                    failures.push(failure);
                }
                let _ = tokio::time::timeout(Duration::from_millis(50), stopped.changed()).await;
            }
            failures
        });
        Self { stop, task }
    }

    async fn stop(self) -> anyhow::Result<Vec<String>> {
        self.stop.send(true)?;
        Ok(self.task.await?)
    }
}

/// Sends one request and describes it when it does not answer 200.
async fn probe_once(client: &reqwest::Client, url: &str) -> Option<String> {
    match client.get(url).header(HOST, DOMAIN).send().await {
        Ok(reply) if reply.status() == 200 => None,
        Ok(reply) => Some(reply.status().to_string()),
        Err(err) => Some(err.to_string()),
    }
}

async fn wait_until_unrouted(ctx: &Context) -> anyhow::Result<()> {
    let url = format!("http://{}/", ctx.proxy.socket_addr());
    let client = reqwest::Client::new();
    for _ in 0..60 {
        let reply = client.get(&url).header(HOST, DOMAIN).send().await;
        if reply.is_ok_and(|reply| reply.status() != 200) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("the proxy still routes {DOMAIN}")
}

/// Waits until the app has `count` containers. An old container stops after the drain period.
async fn wait_for_containers(id: &str, count: usize) -> anyhow::Result<()> {
    let runtime = runtime()?;
    let label: AppName = id.parse()?;
    let mut names = Vec::new();
    for _ in 0..120 {
        names = runtime
            .list_containers(&label)
            .await?
            .into_iter()
            .map(|container| container.name)
            .collect::<Vec<_>>();
        if names.len() == count {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    bail!("the app has the containers {names:?}, not {count}")
}

async fn network_exists(network: &NetworkName) -> anyhow::Result<bool> {
    let client = api_client()?;
    let request = client.get(&format!("/networks/{network}"))?;
    let allowed = [hyper::StatusCode::NOT_FOUND];
    let response = client
        .send_with(request, RESPONSE_TIMEOUT, &allowed)
        .await?;
    Ok(response.status() == hyper::StatusCode::OK)
}

/// Removes what a failed run left: the containers and the network of the app.
async fn remove_leftovers(id: &str) -> anyhow::Result<()> {
    let runtime = runtime()?;
    for container in runtime.list_containers(&id.parse()?).await? {
        runtime.remove_container(&container.name.parse()?).await?;
    }
    runtime
        .remove_network(&format!("r3v3rs3-{id}").parse()?)
        .await
}
