//! The admin panel as production serves it: a client on the internet reaches an HTTPS port of
//! r3v3rs3, whose proxy forwards to the admin API on a loopback address. The other admin tests
//! call the admin API directly, so they never see what the proxy hop changes: the client
//! address, the HTTPS state, the `Host` header and the `Origin` checks.
//!
//! The edge in front of the HTTPS port is a trusted proxy of the port, so the `X-Forwarded-For`
//! header of a test request plays the address of a client on the internet.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ipnet::IpNet;
use r3v3rs3::certs::Cert;
use r3v3rs3::server::{Server, ServerChannels};
use r3v3rs3::{
    admin::{start_admin, start_admin_in_process},
    config::new_appinfo,
    log::DatabaseLayer,
};
use r3v3rs3_api::{
    app::{AppConfig, McpConfig},
    auth::Role,
    client_ip::ClientIpConfig,
    port::{Port, PortEntry, PortOptions},
    proxy::HttpProxy,
    tls::TlsTermination,
};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode, redirect::Policy};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::filter::LevelFilter;

mod common;
use common::{
    TestPort, TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    wait_until,
};

const PASSWORD: &str = "correct-horse-battery";
const REDIRECT_URI: &str = "http://127.0.0.1:49170/callback";

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The panel behind the HTTPS port of the proxy.
struct Panel {
    /// The address that the admin API listens on, as on a production host.
    admin: SocketAddr,
    /// The public URL of the panel: the address of the HTTPS port.
    public: String,
    /// The address of the plain HTTP port of the same proxy.
    plain: String,
    root: Arc<Cert>,
}

impl Panel {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.public)
    }

    /// A browser on the internet with the address `client`: it trusts the certificate of the
    /// port, sends the `Origin` of the panel and does not follow redirects.
    fn browser(&self, client: &str) -> anyhow::Result<Client> {
        self.browser_from(client, &self.public)
    }

    /// A browser on the internet that sends the `Origin` `origin`. It reaches the public host
    /// of the panel on the loopback address.
    fn browser_from(&self, client: &str, origin: &str) -> anyhow::Result<Client> {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_str(client)?);
        headers.insert("origin", HeaderValue::from_str(origin)?);
        Ok(Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&self.root.pem_chain)?)
            .resolve(PANEL_HOST, SocketAddr::from(([127, 0, 0, 1], 0)))
            .default_headers(headers)
            .redirect(Policy::none())
            .build()?)
    }
}

/// The public host of the panel. It is no loopback name, as in production, so every check of
/// the `Host` header sees what a remote client sends.
const PANEL_HOST: &str = "panel.test";

/// The HTTPS port with a certificate for the panel host.
fn https_port(https: &TestPort) -> PortEntry {
    PortEntry {
        id: "https".parse().unwrap(),
        port: Port {
            active: true,
            name: String::new(),
            listen: https.multiaddr_https(),
            opts: PortOptions {
                tls_termination: Some(TlsTermination {
                    server_names: vec![PANEL_HOST.into()],
                    ..Default::default()
                }),
                proxy_protocol: None,
                http2_limits: None,
                http3_limits: None,
                stream_priority: None,
            },
        },
        status: None,
    }
}

/// The proxy of the panel host. Its trusted proxy list stands for the edge in front of the port.
fn panel_proxy(admin: SocketAddr) -> HttpProxy {
    HttpProxy {
        vhosts: vec![PANEL_HOST.parse().unwrap()],
        routes: vec![http_route("/", &format!("http://{admin}/"), None)],
        upgrade_insecure: false,
        client_ip: ClientIpConfig {
            trust_cdn: false,
            trusted_proxies: vec!["127.0.0.0/8".parse().unwrap()],
        },
        ..Default::default()
    }
}

/// Runs the server, its admin API on a loopback address and the HTTPS port in front of it. The
/// admin API trusts `admin_trusted_proxies` and has one admin account, `username`.
async fn with_panel<F, O>(
    admin_trusted_proxies: Vec<IpNet>,
    username: &str,
    func: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Panel) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    with_panel_listening(true, admin_trusted_proxies, username, func).await
}

/// [`with_panel`], with the TCP listener of the admin API on or off.
async fn with_panel_listening<F, O>(
    listen: bool,
    admin_trusted_proxies: Vec<IpNet>,
    username: &str,
    func: F,
) -> anyhow::Result<()>
where
    F: FnOnce(Panel) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    let dir = panel_dir().await?;
    let (panel, storage) = new_panel(admin_trusted_proxies, username).await?;

    let (server, channels) = Server::new(new_appinfo(&dir.0, &dir.0), storage).await;
    let event = channels.event.clone();
    let task = tokio::spawn(server.start());
    spawn_admin(listen, &dir.0, panel.admin, channels);
    wait_for_panel(&panel).await?;
    let result = func(panel).await;
    event.send(r3v3rs3_api::event::ServerEvent::Shutdown)?;
    task.await??;
    result
}

/// Allocates the HTTPS, HTTP and admin ports, creates the root certificate, and returns the
/// panel with its storage.
async fn new_panel(
    admin_trusted_proxies: Vec<IpNet>,
    username: &str,
) -> anyhow::Result<(Panel, TestStorage)> {
    let https = alloc_tcp_port().await?;
    let http = alloc_tcp_port().await?;
    let admin = alloc_tcp_port().await?.socket_addr();
    let panel = Panel {
        admin,
        public: format!("https://{PANEL_HOST}:{}", https.socket_addr().port()),
        plain: format!("http://{PANEL_HOST}:{}", http.socket_addr().port()),
        root: Arc::new(Cert::new_ca()?),
    };
    let storage = panel_storage(&panel, &https, &http, admin_trusted_proxies, username)?;
    Ok((panel, storage))
}

/// Creates a temporary directory with a log database for the admin API.
async fn panel_dir() -> anyhow::Result<TempDir> {
    let unique = hex::encode(rand::random::<[u8; 8]>());
    let dir = TempDir(std::env::temp_dir().join(format!("r3v3rs3-panel-proxy-{unique}")));
    std::fs::create_dir_all(&dir.0)?;
    DatabaseLayer::new(&dir.0.join("log.db"), LevelFilter::INFO).await?;
    Ok(dir)
}

/// Builds the storage of the panel: the public URL with MCP on, the admin account, the HTTPS
/// and HTTP ports, the panel proxy and a certificate for the panel host from the panel root.
fn panel_storage(
    panel: &Panel,
    https: &TestPort,
    http: &TestPort,
    admin_trusted_proxies: Vec<IpNet>,
    username: &str,
) -> anyhow::Result<TestStorage> {
    let root = &panel.root;
    let cert = Arc::new(Cert::new_self_signed(&[PANEL_HOST.parse()?], root)?);

    let mut config = AppConfig {
        public_url: Some(panel.public.parse()?),
        mcp: McpConfig {
            enabled: true,
            clients: Vec::new(),
        },
        ..Default::default()
    };
    config.admin.trusted_proxies = admin_trusted_proxies;
    let mut proxy = http_proxy_entry("panel", "https", panel_proxy(panel.admin));
    proxy.proxy.ports.push("http".parse()?);
    Ok(TestStorage::builder()
        .config(config)
        .account(username, PASSWORD, Role::Admin, None)
        .ports(vec![https_port(https), http_port_entry("http", http)])
        .proxies(vec![proxy])
        .certs(
            [(root.id, root.clone()), (cert.id, cert.clone())]
                .into_iter()
                .collect(),
        )
        .build())
}

/// Starts the admin API with its TCP listener, or in process only when `listen` is off.
fn spawn_admin(listen: bool, dir: &std::path::Path, admin: SocketAddr, channels: ServerChannels) {
    let app_info = new_appinfo(dir, dir);
    if listen {
        tokio::spawn(start_admin(
            app_info,
            admin,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
        ));
    } else {
        let (upgrade, _) = tokio::sync::mpsc::channel(1);
        tokio::spawn(start_admin_in_process(
            app_info,
            admin,
            channels.command,
            channels.callback,
            channels.event.clone(),
            channels.accounts.clone(),
            upgrade,
        ));
    }
}

/// Waits until the proxy forwards to the admin API.
async fn wait_for_panel(panel: &Panel) -> anyhow::Result<()> {
    let client = panel.browser("203.0.113.1")?;
    let url = panel.url("/.well-known/oauth-protected-resource");
    let ready = || {
        let (client, url) = (client.clone(), url.clone());
        async move {
            Ok(client
                .get(url)
                .send()
                .await
                .is_ok_and(|res| res.status() == StatusCode::OK))
        }
    };
    wait_until(ready, "the proxy does not forward to the admin API").await
}

/// Signs in through the proxy and returns the `Set-Cookie` header of the session.
async fn sign_in(panel: &Panel, browser: &Client, username: &str) -> anyhow::Result<String> {
    let response = browser
        .post(panel.url("/api/login"))
        .json(&json!({"username": username, "method": "password", "password": PASSWORD}))
        .send()
        .await?;
    anyhow::ensure!(
        response.status() == StatusCode::OK,
        "the sign-in answered {}",
        response.status()
    );
    let set_cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .ok_or_else(|| anyhow::anyhow!("the sign-in set no cookie"))?
        .to_str()?
        .to_string();
    Ok(set_cookie)
}

/// The `name=value` part of a `Set-Cookie` header.
fn cookie_pair(set_cookie: &str) -> String {
    set_cookie.split(';').next().unwrap_or_default().to_string()
}

/// The client address of the newest audit entry of `username` with `action`. The server writes
/// an audit entry in the background after it answers, so this waits until the entry appears.
async fn audit_client(
    panel: &Panel,
    browser: &Client,
    cookie: &str,
    username: &str,
    action: &str,
) -> anyhow::Result<Option<String>> {
    audit_client_at(&panel.public, browser, cookie, username, action).await
}

/// [`audit_client`] through the admin API at `base`.
async fn audit_client_at(
    base: &str,
    browser: &Client,
    cookie: &str,
    username: &str,
    action: &str,
) -> anyhow::Result<Option<String>> {
    let found = std::sync::Mutex::new(None);
    let entry = || async {
        let entries: Vec<Value> = browser
            .get(format!("{base}/api/audit?username={username}&limit=50"))
            .header("cookie", cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let entry = entries.into_iter().find(|entry| entry["action"] == action);
        let ready = entry.is_some();
        *found.lock().unwrap() = entry;
        Ok(ready)
    };
    wait_until(entry, &format!("no audit entry {action} of {username}")).await?;
    let entry = found.lock().unwrap().take();
    Ok(entry.and_then(|entry| entry["client"].as_str().map(str::to_string)))
}

/// A signed-in account changes the state through the proxy: the cookie is `Secure`, the
/// `Origin` check accepts the public URL, and the audit log records the internet address.
async fn the_session_works_through_the_proxy(panel: Panel, username: &str) -> anyhow::Result<()> {
    let client = "203.0.113.51";
    let browser = panel.browser(client)?;
    let set_cookie = sign_in(&panel, &browser, username).await?;
    assert!(
        set_cookie.contains("Secure"),
        "a sign-in over HTTPS must set a Secure cookie: {set_cookie}"
    );
    let cookie = cookie_pair(&set_cookie);

    let added = browser
        .post(panel.url("/api/accounts"))
        .header("cookie", &cookie)
        .json(&json!({"username": format!("{username}-viewer"), "password": PASSWORD, "role": "viewer"}))
        .send()
        .await?;
    assert_eq!(added.status(), StatusCode::OK, "{}", added.text().await?);

    for action in ["login", "add_account"] {
        assert_eq!(
            audit_client(&panel, &browser, &cookie, username, action)
                .await?
                .as_deref(),
            Some(client),
            "the audit entry {action} must record the internet address"
        );
    }
    Ok(())
}

/// The fields that a browser submits: every `<input>` between `<form` and `</form>`.
fn form_fields(page: &str) -> Vec<(String, String)> {
    let start = page.find("<form").unwrap_or(page.len());
    let end = page.find("</form>").unwrap_or(page.len());
    page[start..end]
        .split("<input ")
        .skip(1)
        .filter_map(|tag| Some((attribute(tag, "name")?, attribute(tag, "value")?)))
        .collect()
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let tag = &tag[..tag.find('>')?];
    let rest = tag.split(&format!("{name}=\"")).nth(1)?;
    Some(rest[..rest.find('"')?].to_string())
}

/// Registers a client the way Claude Code does and returns its id.
async fn register(panel: &Panel, browser: &Client) -> anyhow::Result<String> {
    let registered: Value = browser
        .post(panel.url("/oauth/register"))
        .json(&json!({"client_name": "Claude Code", "redirect_uris": [REDIRECT_URI]}))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(registered["client_id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no client_id"))?
        .to_string())
}

/// Approves the client on the consent page and returns the authorization code.
async fn approve(
    panel: &Panel,
    browser: &Client,
    cookie: &str,
    client_id: &str,
    challenge: &str,
) -> anyhow::Result<String> {
    let page = browser
        .get(panel.url(&format!(
            "/oauth/authorize?response_type=code&client_id={client_id}\
             &redirect_uri={REDIRECT_URI}&state=st&code_challenge={challenge}\
             &code_challenge_method=S256"
        )))
        .header("cookie", cookie)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let mut form = form_fields(&page);
    form.push(("decision".to_string(), "approve".to_string()));
    let consent = browser
        .post(panel.url("/oauth/authorize"))
        .header("cookie", cookie)
        .form(&form)
        .send()
        .await?;
    let location = consent
        .headers()
        .get("location")
        .ok_or_else(|| anyhow::anyhow!("the consent answered {}", consent.status()))?
        .to_str()?
        .to_string();
    location
        .split("code=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("the consent sent no code: {location}"))
}

/// Checks that the authorization server metadata names the public URL of the panel.
async fn check_metadata(panel: &Panel, browser: &Client) -> anyhow::Result<()> {
    let metadata: Value = browser
        .get(panel.url("/.well-known/oauth-authorization-server"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(metadata["issuer"], json!(panel.public));
    assert_eq!(metadata["token_endpoint"], json!(panel.url("/oauth/token")));
    Ok(())
}

/// Exchanges the authorization code with the PKCE verifier and returns the token answer.
async fn exchange_code(
    panel: &Panel,
    browser: &Client,
    code: &str,
    client_id: &str,
    verifier: &str,
) -> anyhow::Result<Value> {
    Ok(browser
        .post(panel.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id),
            ("code_verifier", verifier),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// Checks that the access token opens the MCP endpoint with an `initialize` request.
async fn check_mcp_initialize(
    panel: &Panel,
    browser: &Client,
    tokens: &Value,
) -> anyhow::Result<()> {
    let mcp = browser
        .post(panel.url("/api/mcp"))
        .bearer_auth(tokens["access_token"].as_str().unwrap_or_default())
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-03-26", "capabilities": {},
                       "clientInfo": {"name": "test", "version": "1"}}
        }))
        .send()
        .await?;
    assert_eq!(mcp.status(), StatusCode::OK);
    Ok(())
}

/// An MCP host signs in through the proxy: the metadata names the public URL, the consent
/// grants both scopes, the token opens the MCP endpoint, and the audit log records the internet
/// address of the browser that approved it.
async fn an_mcp_client_signs_in_through_the_proxy(
    panel: Panel,
    username: &str,
) -> anyhow::Result<()> {
    let client = "203.0.113.52";
    let browser = panel.browser(client)?;
    check_metadata(&panel, &browser).await?;

    let client_id = register(&panel, &browser).await?;
    let cookie = cookie_pair(&sign_in(&panel, &browser, username).await?);
    let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let code = approve(&panel, &browser, &cookie, &client_id, &challenge).await?;

    let tokens = exchange_code(&panel, &browser, &code, &client_id, &verifier).await?;
    assert_eq!(tokens["scope"], json!("read edit"));

    check_mcp_initialize(&panel, &browser, &tokens).await?;

    assert_eq!(
        audit_client(&panel, &browser, &cookie, username, "authorize_mcp_client")
            .await?
            .as_deref(),
        Some(client)
    );
    Ok(())
}

fn loopback() -> Vec<IpNet> {
    vec!["127.0.0.1/32".parse().unwrap()]
}

#[tokio::test]
async fn a_session_works_through_the_proxy_with_the_production_settings() -> anyhow::Result<()> {
    with_panel(loopback(), "prod-session", |panel| {
        the_session_works_through_the_proxy(panel, "prod-session")
    })
    .await
}

#[tokio::test]
async fn an_mcp_client_signs_in_through_the_proxy_with_the_production_settings()
-> anyhow::Result<()> {
    with_panel(loopback(), "prod-mcp", |panel| {
        an_mcp_client_signs_in_through_the_proxy(panel, "prod-mcp")
    })
    .await
}

/// Without a trusted proxy list the panel must not believe a forged `X-Forwarded-For`, even
/// from a loopback address: another process on the host can send it.
#[tokio::test]
async fn a_direct_loopback_request_cannot_forge_the_client_address() -> anyhow::Result<()> {
    with_panel(Vec::new(), "direct", |panel| async move {
        let forged = Client::builder()
            .default_headers({
                let mut headers = HeaderMap::new();
                headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.7"));
                headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
                headers
            })
            .build()?;
        let response = forged
            .post(format!("http://{}/api/login", panel.admin))
            .json(&json!({"username": "direct", "method": "password", "password": PASSWORD}))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .map(|value| value.to_str().unwrap_or_default().to_string())
            .unwrap_or_default();
        assert!(
            !set_cookie.contains("Secure"),
            "a forged X-Forwarded-Proto must not mark the cookie Secure: {set_cookie}"
        );
        let cookie = cookie_pair(&set_cookie);
        let base = format!("http://{}", panel.admin);
        let login = audit_client_at(&base, &Client::new(), &cookie, "direct", "login").await?;
        assert_eq!(
            login.as_deref(),
            Some("127.0.0.1"),
            "a forged X-Forwarded-For must not reach the audit log"
        );
        Ok(())
    })
    .await
}

// The proxy serves the panel in process: the panel learns the client from the proxy itself,
// so the production behavior needs no trusted proxy list and no forwarded header that another
// process could forge.

#[tokio::test]
async fn a_session_works_through_the_proxy_without_a_trusted_proxy_list() -> anyhow::Result<()> {
    with_panel(Vec::new(), "target-session", |panel| {
        the_session_works_through_the_proxy(panel, "target-session")
    })
    .await
}

/// Without the TCP listener nothing answers on the admin address, and the proxy still serves the
/// whole panel in process.
#[tokio::test]
async fn the_panel_works_without_its_tcp_listener() -> anyhow::Result<()> {
    with_panel_listening(false, Vec::new(), "no-listener", |panel| async move {
        assert!(
            tokio::net::TcpStream::connect(panel.admin).await.is_err(),
            "the admin address must not accept connections"
        );
        the_session_works_through_the_proxy(panel, "no-listener").await
    })
    .await
}

/// A sign-in over plain HTTP gets no `Secure` cookie: the browser would drop it, and the panel
/// must take the TLS state from the proxy, not from the client address.
#[tokio::test]
async fn a_sign_in_over_plain_http_sets_no_secure_cookie() -> anyhow::Result<()> {
    with_panel(Vec::new(), "plain-session", |panel| async move {
        let browser = panel.browser_from("203.0.113.53", &panel.plain)?;
        let response = browser
            .post(format!("{}/api/login", panel.plain))
            .json(&json!({"username": "plain-session", "method": "password", "password": PASSWORD}))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .ok_or_else(|| anyhow::anyhow!("the sign-in set no cookie"))?
            .to_str()?
            .to_string();
        assert!(
            !set_cookie.contains("Secure"),
            "a sign-in over plain HTTP must not set a Secure cookie: {set_cookie}"
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn an_mcp_client_signs_in_through_the_proxy_without_a_trusted_proxy_list()
-> anyhow::Result<()> {
    with_panel(Vec::new(), "target-mcp", |panel| {
        an_mcp_client_signs_in_through_the_proxy(panel, "target-mcp")
    })
    .await
}
