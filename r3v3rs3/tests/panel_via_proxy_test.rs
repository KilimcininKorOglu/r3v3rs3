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
use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
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
use common::{TestPort, TestStorage, alloc_tcp_port, http_proxy_entry, http_route, wait_until};

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
    root: Arc<Cert>,
}

impl Panel {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.public)
    }

    /// A browser on the internet with the address `client`: it trusts the certificate of the
    /// port, sends the `Origin` of the panel and does not follow redirects.
    fn browser(&self, client: &str) -> anyhow::Result<Client> {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_str(client)?);
        headers.insert("origin", HeaderValue::from_str(&self.public)?);
        Ok(Client::builder()
            .add_root_certificate(reqwest::Certificate::from_pem(&self.root.pem_chain)?)
            .default_headers(headers)
            .redirect(Policy::none())
            .build()?)
    }
}

/// The HTTPS port with a certificate for `localhost`.
fn https_port(https: &TestPort) -> PortEntry {
    PortEntry {
        id: "https".parse().unwrap(),
        port: Port {
            active: true,
            name: String::new(),
            listen: https.multiaddr_https(),
            opts: PortOptions {
                tls_termination: Some(TlsTermination {
                    server_names: vec!["localhost".into()],
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
        vhosts: vec!["localhost".parse().unwrap()],
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
    let unique = hex::encode(rand::random::<[u8; 8]>());
    let dir = TempDir(std::env::temp_dir().join(format!("r3v3rs3-panel-proxy-{unique}")));
    std::fs::create_dir_all(&dir.0)?;
    DatabaseLayer::new(&dir.0.join("log.db"), LevelFilter::INFO).await?;

    let https = alloc_tcp_port().await?;
    let admin = alloc_tcp_port().await?.socket_addr();
    let public = format!("https://localhost:{}", https.socket_addr().port());
    let root = Arc::new(Cert::new_ca()?);
    let cert = Arc::new(Cert::new_self_signed(&["localhost".parse()?], &root)?);

    let mut config = AppConfig {
        public_url: Some(public.parse()?),
        mcp: McpConfig {
            enabled: true,
            clients: Vec::new(),
        },
        ..Default::default()
    };
    config.admin.trusted_proxies = admin_trusted_proxies;
    let storage = TestStorage::builder()
        .config(config)
        .account(username, PASSWORD, Role::Admin, None)
        .ports(vec![https_port(&https)])
        .proxies(vec![http_proxy_entry("panel", "https", panel_proxy(admin))])
        .certs(
            [(root.id, root.clone()), (cert.id, cert.clone())]
                .into_iter()
                .collect(),
        )
        .build();

    let (server, channels) = Server::new(new_appinfo(&dir.0, &dir.0), storage).await;
    let event = channels.event.clone();
    let task = tokio::spawn(server.start());
    tokio::spawn(start_admin(
        new_appinfo(&dir.0, &dir.0),
        admin,
        channels.command,
        channels.callback,
        channels.event.clone(),
        channels.accounts.clone(),
    ));
    let panel = Panel {
        admin,
        public,
        root,
    };
    wait_for_panel(&panel).await?;
    let result = func(panel).await;
    event.send(r3v3rs3_api::event::ServerEvent::Shutdown)?;
    task.await??;
    result
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

/// The client address of the newest audit entry of `username` with `action`.
async fn audit_client(
    panel: &Panel,
    browser: &Client,
    cookie: &str,
    username: &str,
    action: &str,
) -> anyhow::Result<Option<String>> {
    let entries: Vec<Value> = browser
        .get(panel.url(&format!("/api/audit?username={username}&limit=50")))
        .header("cookie", cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(entries
        .iter()
        .find(|entry| entry["action"] == action)
        .and_then(|entry| entry["client"].as_str())
        .map(str::to_string))
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

/// An MCP host signs in through the proxy: the metadata names the public URL, the consent
/// grants both scopes, the token opens the MCP endpoint, and the audit log records the internet
/// address of the browser that approved it.
async fn an_mcp_client_signs_in_through_the_proxy(
    panel: Panel,
    username: &str,
) -> anyhow::Result<()> {
    let client = "203.0.113.52";
    let browser = panel.browser(client)?;
    let metadata: Value = browser
        .get(panel.url("/.well-known/oauth-authorization-server"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(metadata["issuer"], json!(panel.public));
    assert_eq!(metadata["token_endpoint"], json!(panel.url("/oauth/token")));

    let client_id = register(&panel, &browser).await?;
    let cookie = cookie_pair(&sign_in(&panel, &browser, username).await?);
    let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let code = approve(&panel, &browser, &cookie, &client_id, &challenge).await?;

    let tokens: Value = browser
        .post(panel.url("/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id.as_str()),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    assert_eq!(tokens["scope"], json!("read edit"));

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
        let entries: Vec<Value> = Client::new()
            .get(format!("http://{}/api/audit?username=direct", panel.admin))
            .header("cookie", &cookie)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let login = entries.iter().find(|entry| entry["action"] == "login");
        assert_eq!(
            login.and_then(|entry| entry["client"].as_str()),
            Some("127.0.0.1"),
            "a forged X-Forwarded-For must not reach the audit log"
        );
        Ok(())
    })
    .await
}

// The target of serving the panel inside the proxy: the panel learns the client from the
// proxy itself, so the production behavior needs no trusted proxy list and no forwarded
// header that another process could forge.

#[tokio::test]
#[ignore = "fails until the proxy serves the panel without the loopback hop"]
async fn a_session_works_through_the_proxy_without_a_trusted_proxy_list() -> anyhow::Result<()> {
    with_panel(Vec::new(), "target-session", |panel| {
        the_session_works_through_the_proxy(panel, "target-session")
    })
    .await
}

#[tokio::test]
#[ignore = "fails until the proxy serves the panel without the loopback hop"]
async fn an_mcp_client_signs_in_through_the_proxy_without_a_trusted_proxy_list()
-> anyhow::Result<()> {
    with_panel(Vec::new(), "target-mcp", |panel| {
        an_mcp_client_signs_in_through_the_proxy(panel, "target-mcp")
    })
    .await
}
