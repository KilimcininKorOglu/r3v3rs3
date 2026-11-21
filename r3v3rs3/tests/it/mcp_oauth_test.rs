//! The OAuth 2.1 authorization server of the admin API: the metadata documents, the consent
//! flow with PKCE and the token endpoint.

/// The operation table of the CLI, shared so that this file checks it against the served
/// OpenAPI document.
#[path = "../../../r3v3rs3-cli/src/operations.rs"]
mod cli_operations;

use anyhow::Context as _;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use r3v3rs3::server::Server;
use r3v3rs3::{admin::start_admin, config::new_appinfo, log::DatabaseLayer};
use r3v3rs3_api::app::{AppConfig, McpClientConfig, McpConfig};
use r3v3rs3_api::auth::Role;
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tracing_subscriber::filter::LevelFilter;

use crate::common;
use common::{TestStorage, alloc_tcp_port, login_when_allowed, session_cookie, wait_for_listener};

const CLIENT_ID: &str = "r3v3rs3-cli";
const REDIRECT_URI: &str = "http://127.0.0.1:49152/callback";

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

/// The configuration with the MCP clients of the test.
fn mcp_config() -> AppConfig {
    AppConfig {
        mcp: McpConfig {
            enabled: true,
            clients: vec![McpClientConfig {
                id: CLIENT_ID.to_string(),
                name: "r3v3rs3 MCP client".to_string(),
                redirect_uris: vec![REDIRECT_URI.to_string()],
            }],
        },
        ..Default::default()
    }
}

/// Runs a server and its admin API with the MCP clients of the test.
async fn with_admin<F, O>(dir: &Path, func: F) -> anyhow::Result<()>
where
    F: FnOnce(SocketAddr) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    with_admin_config(dir, mcp_config(), func).await
}

/// Runs a server and its admin API with a configuration.
async fn with_admin_config<F, O>(dir: &Path, config: AppConfig, func: F) -> anyhow::Result<()>
where
    F: FnOnce(SocketAddr) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    let storage = TestStorage::builder()
        .config(config)
        .account("admin", "password", Role::Admin, None)
        .account("editor", "password", Role::Editor, None)
        .build();
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

/// A PKCE verifier with its S256 challenge.
fn pkce() -> (String, String) {
    let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// A client that does not follow the redirect to the client's callback address.
fn no_redirect() -> Client {
    Client::builder().redirect(Policy::none()).build().unwrap()
}

/// Signs in and returns the session cookie.
async fn session(client: &Client, addr: &SocketAddr) -> anyhow::Result<String> {
    let _ = client;
    session_as(addr, "admin", "password").await
}

async fn session_as(addr: &SocketAddr, username: &str, password: &str) -> anyhow::Result<String> {
    match session_cookie(*addr, username, password).await {
        Ok(cookie) => Ok(cookie),
        Err(err) if err.to_string().contains("429") => {
            login_when_allowed(*addr, username, password).await
        }
        Err(err) => Err(err),
    }
}

#[tokio::test]
async fn the_metadata_documents_list_the_endpoints() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-metadata")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let metadata: Value = client
            .get(format!(
                "http://{addr}/.well-known/oauth-authorization-server"
            ))
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(
            metadata["authorization_endpoint"],
            json!(format!("http://{addr}/oauth/authorize"))
        );
        assert_eq!(
            metadata["token_endpoint"],
            json!(format!("http://{addr}/oauth/token"))
        );
        assert_eq!(
            metadata["code_challenge_methods_supported"],
            json!(["S256"])
        );
        let resource: Value = client
            .get(format!(
                "http://{addr}/.well-known/oauth-protected-resource"
            ))
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(
            resource["resource"],
            json!(format!("http://{addr}/api/mcp"))
        );
        assert_eq!(
            resource["authorization_servers"],
            json!([format!("http://{addr}")])
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn an_authorization_request_without_pkce_is_refused() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-pkce")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let url = format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
             &redirect_uri={REDIRECT_URI}&state=s1&code_challenge=abc\
             &code_challenge_method=plain"
        );
        let response = client.get(&url).header("cookie", &cookie).send().await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn an_unknown_client_is_refused() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-client")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (_, challenge) = pkce();
        let url = format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id=unknown\
             &redirect_uri={REDIRECT_URI}&state=s1&code_challenge={challenge}\
             &code_challenge_method=S256"
        );
        let response = client.get(&url).header("cookie", &cookie).send().await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn an_authorization_request_without_session_redirects_to_login() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-unauth")?;
    with_admin(&dir.0, |addr| async move {
        let client = no_redirect();
        let (_, challenge) = pkce();
        let url = format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
             &redirect_uri={REDIRECT_URI}&state=s1&code_challenge={challenge}\
             &code_challenge_method=S256"
        );
        let response = client.get(&url).send().await?;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        let location = response.headers().get("location").unwrap().to_str()?;
        assert!(location.starts_with("/login?redirect="), "{location}");
        assert!(location.contains("oauth%2Fauthorize"), "{location}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn an_authenticated_authorization_request_renders_the_consent_page() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-consent-page")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (_, challenge) = pkce();
        let url = format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
             &redirect_uri={REDIRECT_URI}&state=s1&code_challenge={challenge}\
             &code_challenge_method=S256&scope=read%20edit"
        );
        let response = client.get(&url).header("cookie", &cookie).send().await?;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("content-security-policy")
                .and_then(|v| v.to_str().ok()),
            Some("frame-ancestors 'none'")
        );
        assert_eq!(
            response
                .headers()
                .get("x-frame-options")
                .and_then(|v| v.to_str().ok()),
            Some("DENY")
        );
        let text = response.text().await?;
        assert!(text.contains("r3v3rs3 MCP client"), "{text}");
        assert!(text.contains("admin"), "{text}");
        assert!(text.contains("Permissions Requested"), "{text}");
        assert!(text.contains("value=\"approve\""), "{text}");
        assert!(text.contains("value=\"deny\""), "{text}");
        assert!(text.contains("name=\"csrf_token\""), "{text}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn the_consent_page_follows_the_webui_language_and_theme() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-consent-preferences")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (_, challenge) = pkce();
        let url = format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
             &redirect_uri={REDIRECT_URI}&state=s1&code_challenge={challenge}\
             &code_challenge_method=S256&scope=read"
        );
        let cookies = format!("{cookie}; r3v3rs3_lang=tr; r3v3rs3_theme=dark");
        let text = client
            .get(&url)
            .header("cookie", cookies)
            .send()
            .await?
            .text()
            .await?;
        assert!(
            text.contains("<html lang=\"tr\" data-theme=\"dark\">"),
            "{text}"
        );
        assert!(text.contains("İstenen İzinler"), "{text}");
        assert!(text.contains("read (salt okunur)"), "{text}");
        // The scope checkbox stays inside the form, so the browser submits it.
        let form = &text[text.find("<form").context("no form")?..];
        assert!(
            form.contains("name=\"grant_read\" value=\"1\" checked"),
            "{form}"
        );
        Ok(())
    })
    .await
}

fn extract_csrf_token(html: &str) -> anyhow::Result<String> {
    let marker = "name=\"csrf_token\" value=\"";
    let start = html
        .find(marker)
        .context("consent page does not contain csrf_token input")?
        + marker.len();
    let end = html[start..]
        .find('"')
        .context("unterminated csrf_token attribute in consent form")?;
    Ok(html[start..start + end].to_string())
}

#[tokio::test]
async fn the_authorization_code_flow_issues_tokens() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-flow")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (verifier, challenge) = pkce();

        let code = authorize_and_get_code(addr, &cookie, &challenge).await?;
        let (access, refresh) = trade_code_for_tokens(&client, addr, &code, &verifier).await?;
        assert_code_replay(&client, addr, &code).await?;
        assert_refresh_token_rotation(&client, addr, &refresh, &access).await?;

        Ok(())
    })
    .await
}

async fn authorize_and_get_code(
    addr: SocketAddr,
    cookie: &str,
    challenge: &str,
) -> anyhow::Result<String> {
    let client = Client::new();
    let url = format!(
        "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
         &redirect_uri={REDIRECT_URI}&state=st4te&code_challenge={challenge}\
         &code_challenge_method=S256&scope=read%20edit"
    );
    let page = client
        .get(&url)
        .header("cookie", cookie)
        .send()
        .await?
        .text()
        .await?;
    let csrf_token = extract_csrf_token(&page)?;

    let consent = no_redirect()
        .post(format!("http://{addr}/oauth/authorize"))
        .header("cookie", cookie)
        .header("origin", format!("http://{addr}"))
        .form(&[
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("state", "st4te"),
            ("code_challenge", challenge),
            ("scope", "read edit"),
            ("decision", "approve"),
            ("csrf_token", &csrf_token),
        ])
        .send()
        .await?;
    assert_eq!(consent.status(), StatusCode::SEE_OTHER);
    let location = consent.headers().get("location").unwrap().to_str()?;
    assert!(location.starts_with(REDIRECT_URI), "{location}");
    let code = location
        .split("code=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_string();
    Ok(code)
}

async fn trade_code_for_tokens(
    client: &Client,
    addr: SocketAddr,
    code: &str,
    verifier: &str,
) -> anyhow::Result<(String, String)> {
    let tokens: Value = client
        .post(format!("http://{addr}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(tokens["token_type"], json!("Bearer"));
    assert_eq!(tokens["scope"], json!("read edit"));
    let access = tokens["access_token"].as_str().unwrap().to_string();
    let refresh = tokens["refresh_token"].as_str().unwrap().to_string();
    Ok((access, refresh))
}

async fn assert_code_replay(client: &Client, addr: SocketAddr, code: &str) -> anyhow::Result<()> {
    let replay: Value = client
        .post(format!("http://{addr}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", "any-verifier"),
        ])
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(replay["error"], json!("invalid_grant"));
    Ok(())
}

/// Exchanges a fresh, valid code with one changed parameter and returns the status and body.
async fn exchange_altered_code(
    addr: SocketAddr,
    cookie: &str,
    field: &str,
    value: &str,
) -> anyhow::Result<(StatusCode, Value)> {
    let (verifier, challenge) = pkce();
    let code = authorize_and_get_code(addr, cookie, &challenge).await?;
    let mut form = vec![
        ("grant_type", "authorization_code".to_string()),
        ("code", code),
        ("redirect_uri", REDIRECT_URI.to_string()),
        ("client_id", CLIENT_ID.to_string()),
        ("code_verifier", verifier),
    ];
    for entry in &mut form {
        if entry.0 == field {
            entry.1 = value.to_string();
        }
    }
    let response = Client::new()
        .post(format!("http://{addr}/oauth/token"))
        .form(&form)
        .send()
        .await?;
    Ok((response.status(), response.json().await?))
}

/// A valid code gives no token with another verifier (RFC 7636 section 4.6), another redirect
/// URI (RFC 6749 section 4.1.3) or another client (RFC 6749 section 5.2).
#[tokio::test]
async fn a_code_works_only_with_its_verifier_redirect_and_client() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-code-binding")?;
    // A second registered client with the same redirect URI, so only the binding of the code to
    // its client can refuse the exchange.
    let mut config = mcp_config();
    config.mcp.clients.push(McpClientConfig {
        id: "another-client".to_string(),
        name: "another MCP client".to_string(),
        redirect_uris: vec![REDIRECT_URI.to_string()],
    });
    with_admin_config(&dir.0, config, |addr| async move {
        let cookie = session(&Client::new(), &addr).await?;
        let wrong_verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
        let altered = [
            ("code_verifier", wrong_verifier.as_str()),
            ("redirect_uri", "http://127.0.0.1:49153/callback"),
            ("client_id", "another-client"),
        ];
        for (field, value) in altered {
            let (status, body) = exchange_altered_code(addr, &cookie, field, value).await?;
            assert!(status.is_client_error(), "{field}: {status} {body}");
            assert!(body.get("access_token").is_none(), "{field}: {body}");
        }
        Ok(())
    })
    .await
}

async fn assert_refresh_token_rotation(
    client: &Client,
    addr: SocketAddr,
    refresh: &str,
    access: &str,
) -> anyhow::Result<()> {
    let rotated: Value = client
        .post(format!("http://{addr}/oauth/token"))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh)])
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(rotated["scope"], json!("read edit"));
    assert_ne!(rotated["refresh_token"], json!(refresh));
    assert_ne!(rotated["access_token"], json!(access));

    let replayed = client
        .post(format!("http://{addr}/oauth/token"))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh)])
        .send()
        .await?;
    assert_eq!(replayed.status(), StatusCode::BAD_REQUEST);
    Ok(())
}

#[tokio::test]
async fn a_denied_consent_sends_an_access_denied_error() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-deny")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (_, challenge) = pkce();
        let url = format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
             &redirect_uri={REDIRECT_URI}&state=s9&code_challenge={challenge}\
             &code_challenge_method=S256&scope=read"
        );
        let page = client
            .get(&url)
            .header("cookie", &cookie)
            .send()
            .await?
            .text()
            .await?;
        let csrf_token = extract_csrf_token(&page)?;

        let consent = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .header("origin", format!("http://{addr}"))
            .form(&[
                ("client_id", CLIENT_ID),
                ("redirect_uri", REDIRECT_URI),
                ("state", "s9"),
                ("code_challenge", challenge.as_str()),
                ("scope", "read"),
                ("decision", "deny"),
                ("csrf_token", &csrf_token),
            ])
            .send()
            .await?;
        assert_eq!(consent.status(), StatusCode::SEE_OTHER);
        let location = consent.headers().get("location").unwrap().to_str()?;
        assert!(location.contains("error=access_denied"), "{location}");
        Ok(())
    })
    .await
}

async fn register_dcr_client(client: &Client, addr: SocketAddr) -> anyhow::Result<String> {
    let registered: Value = client
        .post(format!("http://{addr}/oauth/register"))
        .json(&json!({
            "client_name": "Claude Code",
            "redirect_uris": ["http://127.0.0.1:49153/callback"],
        }))
        .send()
        .await?
        .json()
        .await?;
    let client_id = registered["client_id"]
        .as_str()
        .context("the registration returned no client_id")?
        .to_string();
    assert!(client_id.starts_with("mcp-"), "{client_id}");
    assert_eq!(registered["client_name"], json!("Claude Code"));
    Ok(client_id)
}

async fn approve_dcr_client(
    client: &Client,
    addr: SocketAddr,
    cookie: &str,
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
) -> anyhow::Result<String> {
    let url = format!(
        "http://{addr}/oauth/authorize?response_type=code&client_id={client_id}\
         &redirect_uri={redirect_uri}&state=dcr&code_challenge={challenge}\
         &code_challenge_method=S256&scope=read"
    );
    let page = client
        .get(&url)
        .header("cookie", cookie)
        .send()
        .await?
        .text()
        .await?;
    let csrf_token = extract_csrf_token(&page)?;

    let consent = no_redirect()
        .post(format!("http://{addr}/oauth/authorize"))
        .header("cookie", cookie)
        .header("origin", format!("http://{addr}"))
        .form(&[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("state", "dcr"),
            ("code_challenge", challenge),
            ("scope", "read"),
            ("decision", "approve"),
            ("csrf_token", &csrf_token),
        ])
        .send()
        .await?;
    assert_eq!(consent.status(), StatusCode::SEE_OTHER);
    let location = consent.headers().get("location").unwrap().to_str()?;
    assert!(location.starts_with(redirect_uri), "{location}");
    let code = location
        .split("code=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_string();
    Ok(code)
}

#[tokio::test]
async fn a_dynamically_registered_client_runs_the_flow() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-dcr")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let client_id = register_dcr_client(&client, addr).await?;

        // The registered client runs the same consent flow as a configured one.
        let cookie = session(&client, &addr).await?;
        let (verifier, challenge) = pkce();
        let redirect_uri = "http://127.0.0.1:49153/callback";
        let code = approve_dcr_client(&client, addr, &cookie, &client_id, redirect_uri, &challenge)
            .await?;

        // The code trades for tokens under the dynamic client id.
        let tokens: Value = client
            .post(format!("http://{addr}/oauth/token"))
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", code.as_str()),
                ("redirect_uri", redirect_uri),
                ("client_id", client_id.as_str()),
                ("code_verifier", verifier.as_str()),
            ])
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(tokens["token_type"], json!("Bearer"));
        assert_eq!(tokens["scope"], json!("read"));
        Ok(())
    })
    .await
}

/// Runs the consent of a request without a scope parameter, as Claude Code sends it, with the
/// given scope checkboxes, and returns the scope of the issued token.
async fn scope_after_consent(addr: SocketAddr, checked: &[&str]) -> anyhow::Result<Value> {
    let client = Client::new();
    let cookie = session(&client, &addr).await?;
    let (verifier, challenge) = pkce();
    let page = client
        .get(format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
             &redirect_uri={REDIRECT_URI}&state=ns&code_challenge={challenge}\
             &code_challenge_method=S256"
        ))
        .header("cookie", &cookie)
        .send()
        .await?
        .text()
        .await?;
    // Both scopes are offered and checked, so the account can grant edit.
    assert!(
        page.contains("name=\"grant_read\" value=\"1\" checked"),
        "{page}"
    );
    assert!(
        page.contains("name=\"grant_edit\" value=\"1\" checked"),
        "{page}"
    );
    assert!(
        page.contains("name=\"scope\" value=\"read edit\""),
        "{page}"
    );
    // Submit what a browser submits: the fields inside the form, without the unchecked boxes.
    let mut form: Vec<(String, String)> = browser_form_fields(&page)
        .into_iter()
        .filter(|(name, _)| {
            name.strip_prefix("grant_")
                .is_none_or(|scope| checked.contains(&scope))
        })
        .collect();
    form.push(("decision".to_string(), "approve".to_string()));
    let consent = no_redirect()
        .post(format!("http://{addr}/oauth/authorize"))
        .header("cookie", &cookie)
        .header("origin", format!("http://{addr}"))
        .form(&form)
        .send()
        .await?;
    let location = consent.headers().get("location").unwrap().to_str()?;
    match location
        .split("code=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
    {
        Some(code) => token_scope(&client, addr, code, &verifier).await,
        None => Ok(json!(location.contains("error=access_denied"))),
    }
}

/// The `name` and `value` of every `<input>` between `<form` and `</form>`, the fields that a
/// browser submits. An input outside the form element is not submitted.
fn browser_form_fields(page: &str) -> Vec<(String, String)> {
    let start = page.find("<form").unwrap_or(page.len());
    let end = page.find("</form>").unwrap_or(page.len());
    page[start..end]
        .split("<input ")
        .skip(1)
        .filter_map(|tag| Some((attribute(tag, "name")?, attribute(tag, "value")?)))
        .collect()
}

/// The value of an HTML attribute at the start of a tag, without entity decoding.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let tag = &tag[..tag.find('>')?];
    let rest = tag.split(&format!("{name}=\"")).nth(1)?;
    Some(rest[..rest.find('"')?].to_string())
}

/// Trades a code of the default client for tokens and returns their scope.
async fn token_scope(
    client: &Client,
    addr: SocketAddr,
    code: &str,
    verifier: &str,
) -> anyhow::Result<Value> {
    let tokens: Value = client
        .post(format!("http://{addr}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier),
        ])
        .send()
        .await?
        .json()
        .await?;
    Ok(tokens["scope"].clone())
}

#[tokio::test]
async fn a_request_without_a_scope_lets_the_account_pick_the_scopes() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-noscope")?;
    with_admin(&dir.0, |addr| async move {
        assert_eq!(
            scope_after_consent(addr, &["read", "edit"]).await?,
            json!("read edit")
        );
        assert_eq!(scope_after_consent(addr, &["read"]).await?, json!("read"));
        // Unchecking every scope refuses the client like the Cancel button.
        assert_eq!(scope_after_consent(addr, &[]).await?, json!(true));

        // The 401 challenge names the scopes, so a client can request them itself.
        let challenge = Client::new()
            .post(format!("http://{addr}/api/mcp"))
            .send()
            .await?;
        let header = challenge
            .headers()
            .get("www-authenticate")
            .unwrap()
            .to_str()?
            .to_string();
        assert!(header.contains("scope=\"read edit\""), "{header}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn a_registration_without_a_redirect_uri_is_refused() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-dcr-invalid")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let response = client
            .post(format!("http://{addr}/oauth/register"))
            .json(&json!({ "client_name": "broken", "redirect_uris": [] }))
            .send()
            .await?;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error: Value = response.json().await?;
        assert_eq!(error["error"], json!("invalid_redirect_uri"));
        Ok(())
    })
    .await
}

/// Completes the authorization flow and returns an access token with the edit scope.
async fn access_token(addr: &SocketAddr) -> anyhow::Result<String> {
    access_token_for(addr, "admin", "password").await
}

async fn access_token_with_scope(addr: &SocketAddr, scope: &str) -> anyhow::Result<String> {
    access_token_for_scope(addr, "admin", "password", scope).await
}

async fn access_token_for(
    addr: &SocketAddr,
    username: &str,
    password: &str,
) -> anyhow::Result<String> {
    access_token_for_scope(addr, username, password, "read edit").await
}

async fn access_token_for_scope(
    addr: &SocketAddr,
    username: &str,
    password: &str,
    scope: &str,
) -> anyhow::Result<String> {
    let client = Client::new();
    let cookie = session_as(addr, username, password).await?;
    let (verifier, challenge) = pkce();
    let encoded_scope = scope.replace(' ', "%20");
    let url = format!(
        "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
         &redirect_uri={REDIRECT_URI}&state=st&code_challenge={challenge}\
         &code_challenge_method=S256&scope={encoded_scope}"
    );
    let page = client
        .get(&url)
        .header("cookie", &cookie)
        .send()
        .await?
        .text()
        .await?;
    let csrf_token = extract_csrf_token(&page)?;

    let consent = no_redirect()
        .post(format!("http://{addr}/oauth/authorize"))
        .header("cookie", &cookie)
        .header("origin", format!("http://{addr}"))
        .form(&[
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("state", "st"),
            ("code_challenge", challenge.as_str()),
            ("scope", scope),
            ("decision", "approve"),
            ("csrf_token", &csrf_token),
        ])
        .send()
        .await?;
    let location = consent
        .headers()
        .get("location")
        .unwrap()
        .to_str()?
        .to_string();
    let code = location
        .split("code=")
        .nth(1)
        .and_then(|rest| rest.split('&').next())
        .unwrap()
        .to_string();
    let tokens: Value = Client::new()
        .post(format!("http://{addr}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier.as_str()),
        ])
        .send()
        .await?
        .json()
        .await?;
    Ok(tokens["access_token"].as_str().unwrap().to_string())
}

/// A silent MCP client for the test.
#[derive(Clone, Default)]
struct TestClient;

impl rmcp::ClientHandler for TestClient {}

#[tokio::test]
async fn the_mcp_endpoint_serves_the_tools() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-server")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token(&addr).await?;
        assert_mcp_anonymous_and_raw(addr, &token).await?;
        assert_mcp_client_tools(addr, &token).await?;
        Ok(())
    })
    .await
}

async fn assert_mcp_anonymous_and_raw(addr: SocketAddr, token: &str) -> anyhow::Result<()> {
    // Without a token the endpoint points at the resource metadata.
    let anonymous = Client::new()
        .post(format!("http://{addr}/api/mcp"))
        .header("accept", "application/json")
        .body("{}")
        .send()
        .await?;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    let www = anonymous
        .headers()
        .get("www-authenticate")
        .unwrap()
        .to_str()?;
    assert!(www.contains("oauth-protected-resource"), "{www}");

    // The raw request with the token must pass the middleware.
    let authorized = Client::new()
        .post(format!("http://{addr}/api/mcp"))
        .header("authorization", format!("Bearer {token}"))
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "r3v3rs3-test", "version": "0.0.1"},
            }
        }))
        .send()
        .await?;
    let status = authorized.status();
    let body = authorized.text().await?;
    assert!(status.is_success(), "{status} {body}");
    Ok(())
}

async fn assert_mcp_client_tools(addr: SocketAddr, token: &str) -> anyhow::Result<()> {
    use rmcp::service::serve_client;
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/api/mcp"))
            .auth_header(token),
    );
    let client = serve_client(TestClient, transport).await?;

    assert_mcp_tool_names(&client).await?;
    assert_mcp_read_tools(&client).await?;

    let port_id = assert_mcp_port_mutations(&client).await?;
    assert_mcp_proxy_mutations(&client, &port_id).await?;
    assert_mcp_admin_mutations(&client).await?;

    client.cancel().await?;
    Ok(())
}

async fn assert_mcp_tool_names(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let tools = client.list_tools(Default::default()).await?;
    let names: Vec<&str> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert!(names.contains(&"list_proxies"), "{names:?}");
    assert!(names.contains(&"server_status"), "{names:?}");
    assert!(names.contains(&"list_ports"), "{names:?}");
    assert!(names.contains(&"list_apps"), "{names:?}");
    assert!(names.contains(&"get_store"), "{names:?}");
    assert!(names.contains(&"install_store_app"), "{names:?}");
    assert!(names.contains(&"list_databases"), "{names:?}");
    assert!(names.contains(&"list_database_engines"), "{names:?}");
    assert!(names.contains(&"list_targets"), "{names:?}");
    assert!(names.contains(&"list_projects"), "{names:?}");
    assert!(names.contains(&"list_env_groups"), "{names:?}");
    assert!(
        names.contains(&"list_disaster_recovery_backups"),
        "{names:?}"
    );
    assert!(names.contains(&"get_system_metrics"), "{names:?}");
    assert!(names.contains(&"test_database_backup"), "{names:?}");
    assert!(names.contains(&"get_database_pitr_range"), "{names:?}");
    assert!(names.contains(&"restore_database_pitr"), "{names:?}");
    assert!(names.contains(&"archive_database_wal"), "{names:?}");
    assert!(names.contains(&"list_database_wal_segments"), "{names:?}");
    for name in [
        "get_database_credentials",
        "list_database_tenants",
        "reset_database_password",
        "delete_database_tenant",
    ] {
        assert!(names.contains(&name), "{names:?}");
    }
    assert!(names.contains(&"create_target_token"), "{names:?}");
    assert!(names.contains(&"upgrade_target"), "{names:?}");
    assert!(names.contains(&"upgrade_all_targets"), "{names:?}");
    assert!(names.contains(&"get_upgrade_status"), "{names:?}");
    assert!(names.contains(&"upgrade_server"), "{names:?}");
    assert!(names.contains(&"update_port_transparent"), "{names:?}");
    Ok(())
}

async fn assert_mcp_read_tools(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let text = mcp_call(client, "server_status", json!({})).await?;
    assert!(text.contains("workers"), "{text}");

    let metrics_text = mcp_call(client, "get_system_metrics", json!({})).await?;
    assert!(
        metrics_text.contains("event_loop_lag_seconds"),
        "{metrics_text}"
    );

    let store_text = mcp_call(client, "get_store", json!({})).await?;
    assert!(store_text.contains("catalog"), "{store_text}");

    let ports_text = mcp_call(client, "list_ports", json!({})).await?;
    assert!(ports_text.starts_with('['), "{ports_text}");
    assert_theme_list_fits_an_mcp_client(client).await
}

/// The built-in themes hold 26 to 49 KB of HTML each, and the full list of 183 KB was more than
/// an MCP client reads in one answer. The list names each theme and its status codes, and
/// `get_error_page_theme` gives the HTML of one theme.
async fn assert_theme_list_fits_an_mcp_client(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let list_text = mcp_call(client, "list_error_page_themes", json!({})).await?;
    let themes: Value = serde_json::from_str(&list_text)?;
    let themes = themes.as_array().context("the theme list is no array")?;
    assert!(!themes.is_empty(), "{list_text}");
    assert!(list_text.len() < 16 * 1024, "{} bytes", list_text.len());
    let first = &themes[0];
    let page = &first["pages"][0];
    assert!(page["status"].is_u64(), "{list_text}");
    assert!(page.get("body").is_none(), "{list_text}");

    let id = first["id"].as_str().context("a theme without an id")?;
    let theme_text = mcp_call(client, "get_error_page_theme", json!({ "id": id })).await?;
    let theme: Value = serde_json::from_str(&theme_text)?;
    assert!(
        theme["pages"][0]["body"]
            .as_str()
            .is_some_and(|body| !body.is_empty())
    );
    Ok(())
}

async fn mcp_call(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    name: &'static str,
    args: Value,
) -> anyhow::Result<String> {
    use rmcp::model::CallToolRequestParams;
    let mut params = CallToolRequestParams::new(name);
    if let Some(map) = args.as_object() {
        params = params.with_arguments(map.clone());
    }
    let result = client.call_tool(params).await?;
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    Ok(text)
}

async fn assert_mcp_port_mutations(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<String> {
    let add_res = mcp_call(
        client,
        "create_port",
        json!({
            "port": {
                "name": "p-mcp",
                "listen": "/ip4/127.0.0.1/tcp/0"
            }
        }),
    )
    .await?;
    assert!(add_res.contains("p-mcp"), "{add_res}");

    let list_res = mcp_call(client, "list_ports", json!({})).await?;
    let ports: Value = serde_json::from_str(&list_res)?;
    let port_id = ports
        .as_array()
        .and_then(|arr| arr.iter().find(|p| p["name"] == "p-mcp"))
        .and_then(|p| p["id"].as_str())
        .context("port id not found")?
        .to_string();

    let get_res = mcp_call(client, "get_port", json!({ "id": &port_id })).await?;
    assert!(get_res.contains("p-mcp"), "{get_res}");

    let update_res = mcp_call(
        client,
        "update_port",
        json!({
            "entry": {
                "id": &port_id,
                "name": "p-mcp-upd",
                "listen": "/ip4/127.0.0.1/tcp/0"
            }
        }),
    )
    .await?;
    assert!(update_res.contains("true"), "{update_res}");

    let reset_res = mcp_call(client, "reset_port", json!({ "id": &port_id })).await?;
    assert!(reset_res.contains(&port_id), "{reset_res}");

    assert_mcp_transparent_toggle(client, &port_id).await?;
    Ok(port_id)
}

/// `update_port_transparent` changes only the transparent option of the port, and the change is
/// saved: `get_port` reads it back each time.
async fn assert_mcp_transparent_toggle(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    port_id: &str,
) -> anyhow::Result<()> {
    for enabled in [true, false] {
        let args = json!({ "id": port_id, "enabled": enabled });
        mcp_call(client, "update_port_transparent", args).await?;
        let port: Value =
            serde_json::from_str(&mcp_call(client, "get_port", json!({ "id": port_id })).await?)?;
        // The port JSON leaves out an option that is off.
        let transparent = port["transparent"].as_bool().unwrap_or(false);
        assert_eq!(transparent, enabled, "{port}");
        assert_eq!(port["name"], "p-mcp-upd", "another setting changed: {port}");
    }
    Ok(())
}

async fn assert_mcp_proxy_mutations(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    port_id: &str,
) -> anyhow::Result<()> {
    let add_res = mcp_call(
        client,
        "create_proxy",
        json!({
            "proxy": {
                "name": "px-mcp",
                "protocol": "http",
                "routes": [],
                "ports": [port_id]
            }
        }),
    )
    .await?;
    assert!(add_res.contains("created"), "{add_res}");

    let list_res = mcp_call(client, "list_proxies", json!({})).await?;
    let proxies: Value = serde_json::from_str(&list_res)?;
    let proxy_id = proxies
        .as_array()
        .and_then(|arr| arr.iter().find(|p| p["name"] == "px-mcp"))
        .and_then(|p| p["id"].as_str())
        .context("proxy id not found")?
        .to_string();

    let get_res = mcp_call(client, "get_proxy", json!({ "id": &proxy_id })).await?;
    assert!(get_res.contains("px-mcp"), "{get_res}");

    let update_res = mcp_call(
        client,
        "update_proxy",
        json!({
            "entry": {
                "id": &proxy_id,
                "name": "px-mcp-upd",
                "protocol": "http",
                "routes": [],
                "ports": [port_id]
            }
        }),
    )
    .await?;
    assert!(update_res.contains("true"), "{update_res}");

    let purge_res = mcp_call(client, "purge_proxy_cache", json!({ "id": &proxy_id })).await?;
    assert!(purge_res.contains("true"), "{purge_res}");

    let del_proxy = mcp_call(client, "delete_proxy", json!({ "id": &proxy_id })).await?;
    assert!(del_proxy.contains(&proxy_id), "{del_proxy}");

    let del_port = mcp_call(client, "delete_port", json!({ "id": port_id })).await?;
    assert!(del_port.contains(port_id), "{del_port}");

    Ok(())
}

async fn assert_mcp_admin_mutations(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let unban = mcp_call(client, "unban_ip", json!({ "ip": "10.0.0.1" })).await?;
    assert!(unban.contains("10.0.0.1"), "{unban}");
    assert_mcp_firewall(client).await?;
    assert_mcp_config_backup(client).await?;
    assert_mcp_dns_credentials(client).await
}

/// `backup_config` and `restore_config` reach the panel's backup API (OpenAPI `backup_config`,
/// `restore_config`): a setting changed after the backup returns to its saved value after the
/// restore. `test_notification` refuses to run without a webhook.
async fn assert_mcp_config_backup(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let backup: Value = serde_json::from_str(&mcp_call(client, "backup_config", json!({})).await?)?;
    let saved = backup["config"]["firewall"]["extra_ports"].to_string();
    assert!(saved.contains("udp/51820"), "{backup}");
    mcp_call(
        client,
        "update_firewall",
        json!({ "extra_ports": ["udp/60000"] }),
    )
    .await?;
    mcp_call(client, "restore_config", json!({ "backup": backup })).await?;
    let config: Value = serde_json::from_str(&mcp_call(client, "get_config", json!({})).await?)?;
    assert_eq!(
        config["firewall"]["extra_ports"].to_string(),
        saved,
        "{config}"
    );
    let refused = mcp_call(client, "test_notification", json!({})).await;
    assert!(
        refused.is_err(),
        "a test without a webhook was accepted: {refused:?}"
    );
    Ok(())
}

/// The DNS credential tools reach the same API as the panel (OpenAPI `create_dns_credential`,
/// `get_dns_credential`, `update_dns_credential`, `delete_dns_credential`): a credential is
/// added, read without its token, renamed and deleted.
async fn assert_mcp_dns_credentials(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let secret = "mcp-dns-secret-token";
    let provider =
        json!({ "provider": "webhook", "url": "https://dns.example.com/hook", "token": secret });
    let request = json!({ "name": "mcp-dns", "dns_provider": provider });
    let added: Value = serde_json::from_str(
        &mcp_call(
            client,
            "create_dns_credential",
            json!({ "request": request }),
        )
        .await?,
    )?;
    let id = added["id"]
        .as_str()
        .context("no id in the answer")?
        .to_string();
    let renamed = json!({ "id": id, "request": { "name": "mcp-dns-renamed" } });
    mcp_call(client, "update_dns_credential", renamed).await?;
    let shown = mcp_call(client, "get_dns_credential", json!({ "id": id })).await?;
    assert!(shown.contains("mcp-dns-renamed"), "{shown}");
    assert!(!shown.contains(secret), "{shown}");
    mcp_call(client, "delete_dns_credential", json!({ "id": id })).await?;
    let listed = mcp_call(client, "list_dns_credentials", json!({})).await?;
    assert!(!listed.contains("mcp-dns"), "{listed}");
    Ok(())
}

/// `update_firewall` saves the extra ports of the host firewall table, and a port outside
/// 1-65535 is refused before anything changes. The table stays off, so no `nft` runs.
async fn assert_mcp_firewall(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let ports = json!({ "enabled": false, "extra_ports": ["udp/51820", "tcp/8000-8010"] });
    mcp_call(client, "update_firewall", ports).await?;
    let config: Value = serde_json::from_str(&mcp_call(client, "get_config", json!({})).await?)?;
    let saved = config["firewall"]["extra_ports"].to_string();
    assert!(
        saved.contains("udp/51820") && saved.contains("tcp/8000-8010"),
        "{config}"
    );
    let bad = json!({ "extra_ports": ["udp/99999"] });
    let refused = mcp_call(client, "update_firewall", bad).await;
    assert!(
        refused.is_err(),
        "an invalid port was accepted: {refused:?}"
    );
    let config: Value = serde_json::from_str(&mcp_call(client, "get_config", json!({})).await?)?;
    assert_eq!(
        config["firewall"]["extra_ports"].to_string(),
        saved,
        "{config}"
    );
    Ok(())
}

async fn run_client(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    args: &[&str],
) -> anyhow::Result<std::process::Output> {
    let mut command = std::process::Command::new(binary);
    command
        .arg(format!("http://{addr}"))
        .arg("--token")
        .arg(token);
    command.args(args);
    tokio::task::spawn_blocking(move || command.output().context("failed to run the client binary"))
        .await
        .map_err(|err| anyhow::anyhow!("the client task failed: {err}"))?
}

async fn assert_cli_port_mutations(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
) -> anyhow::Result<String> {
    let payload = json!({
        "name": "cli-port",
        "listen": "/ip4/127.0.0.1/tcp/0"
    })
    .to_string();
    let add_out = run_client(binary, addr, token, &["port", "create", &payload]).await?;
    assert!(
        add_out.status.success(),
        "{}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    let list_out = run_client(binary, addr, token, &["port", "list"]).await?;
    let stdout = String::from_utf8_lossy(&list_out.stdout).to_string();
    assert!(list_out.status.success());
    assert!(stdout.contains("/ip4/127.0.0.1/tcp/0"), "{stdout}");

    let port_id = stdout
        .lines()
        .find(|line| line.contains("/ip4/127.0.0.1/tcp/0"))
        .and_then(|line| line.split_whitespace().next())
        .context("port id not found in cli output")?
        .to_string();

    let get_out = run_client(binary, addr, token, &["port", "get", &port_id]).await?;
    assert!(get_out.status.success());
    assert_cli_transparent(binary, addr, token, &port_id).await?;
    // Turning early data off must be possible; the port has no TLS, so off is its only value.
    let args = ["port", "update-early-data", &port_id, "--enable", "false"];
    let out = run_client(binary, addr, token, &args).await?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_early_data_needs_tls(binary, addr, token, &port_id).await?;
    Ok(port_id)
}

/// OpenAPI `update_port_early_data`: TLS 1.3 early data (RFC 8446 2.3) exists only inside a TLS
/// handshake, so the server refuses to turn it on for a port without TLS termination.
async fn assert_early_data_needs_tls(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    port_id: &str,
) -> anyhow::Result<()> {
    let args = ["port", "update-early-data", port_id, "--enable", "true"];
    let out = run_client(binary, addr, token, &args).await?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(
        stderr.contains("tls_termination_config_missing"),
        "{stderr}"
    );
    Ok(())
}

/// `port update-transparent` (OpenAPI `update_port_transparent`) saves the option, and
/// `--enable false` clears it again.
async fn assert_cli_transparent(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    port_id: &str,
) -> anyhow::Result<()> {
    for enabled in ["true", "false"] {
        let args = ["port", "update-transparent", port_id, "--enable", enabled];
        let out = run_client(binary, addr, token, &args).await?;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let get_out = run_client(binary, addr, token, &["port", "get", port_id]).await?;
        let port = String::from_utf8_lossy(&get_out.stdout).to_string();
        assert_eq!(
            port.contains("\"transparent\": true"),
            enabled == "true",
            "{port}"
        );
    }
    Ok(())
}

async fn assert_cli_proxy_mutations(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    port_id: &str,
) -> anyhow::Result<()> {
    let payload = json!({
        "name": "cli-proxy",
        "protocol": "http",
        "routes": [],
        "ports": [port_id]
    })
    .to_string();
    let add_out = run_client(binary, addr, token, &["proxy", "create", &payload]).await?;
    assert!(
        add_out.status.success(),
        "{}",
        String::from_utf8_lossy(&add_out.stderr)
    );

    let list_out = run_client(binary, addr, token, &["proxy", "list"]).await?;
    let stdout = String::from_utf8_lossy(&list_out.stdout).to_string();
    assert!(list_out.status.success());
    assert!(stdout.contains("cli-proxy"), "{stdout}");

    let proxy_id = stdout
        .lines()
        .find(|line| line.contains("cli-proxy"))
        .and_then(|line| line.split_whitespace().next())
        .context("proxy id not found in cli output")?
        .to_string();

    let get_out = run_client(binary, addr, token, &["proxy", "get", &proxy_id]).await?;
    assert!(get_out.status.success());
    assert_cli_dictionaries(binary, addr, token, &proxy_id).await?;
    assert_cli_stream_priority(binary, addr, token, &proxy_id).await?;

    let rm_proxy = run_client(binary, addr, token, &["proxy", "rm", &proxy_id]).await?;
    assert!(rm_proxy.status.success());

    let rm_port = run_client(binary, addr, token, &["port", "rm", port_id]).await?;
    assert!(rm_port.status.success());
    Ok(())
}

/// `r3 proxy dictionary` reaches the dictionary API (OpenAPI
/// `create_proxy_compression_dictionary`, `list_proxy_compression_dictionaries`,
/// `get_proxy_compression_dictionary`, `delete_proxy_compression_dictionary`): the server
/// computes the RFC 9842 SHA-256 hash of the content, an unknown dictionary id is 404, and an
/// invalid dictionary (a serving path without a leading `/`) is refused.
/// `r3 proxy update-stream-priority` (OpenAPI `update_proxy_stream_priority`) stores the given
/// switches, so `get-stream-priority` shows them, and an unknown proxy fails with not found.
async fn assert_cli_stream_priority(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    proxy_id: &str,
) -> anyhow::Result<()> {
    // Adaptive concurrency defaults to on, so turning it off proves the flag reached the server.
    let flags = ["--enable", "true", "--adaptive-concurrency", "false"];
    let update = [&["proxy", "update-stream-priority", proxy_id][..], &flags].concat();
    cli_ok(binary, addr, token, &update).await?;
    let shown = cli_ok(
        binary,
        addr,
        token,
        &["proxy", "get-stream-priority", proxy_id],
    )
    .await?;
    let config: Value = serde_json::from_str(&shown)?;
    assert_eq!(config["enabled"], json!(true), "{shown}");
    assert_eq!(config["adaptive_concurrency"], json!(false), "{shown}");
    let missing = [&["proxy", "update-stream-priority", MISSING_DB][..], &flags].concat();
    let out = run_client(binary, addr, token, &missing).await?;
    assert_not_found(
        "update-stream-priority",
        &String::from_utf8_lossy(&out.stderr),
    );
    Ok(())
}

async fn assert_cli_dictionaries(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    proxy_id: &str,
) -> anyhow::Result<()> {
    let base = ["proxy", "dictionary", "create", proxy_id, "--name", "api"];
    let created = cli_ok(
        binary,
        addr,
        token,
        &[
            &base[..],
            &[
                "--match-pattern",
                "/api/*",
                "--path",
                "/d.dict",
                "--data",
                "abc",
            ],
        ]
        .concat(),
    )
    .await?;
    // SHA-256("abc") in the RFC 8941 byte sequence form that Available-Dictionary carries.
    assert!(
        created.contains(":ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=:"),
        "{created}"
    );
    let listing = cli_ok(
        binary,
        addr,
        token,
        &["proxy", "dictionary", "list", proxy_id],
    )
    .await?;
    let listed: Value = serde_json::from_str(&listing)?;
    let dict_id = listed[0]["id"].as_str().context("no dictionary id")?;
    let shown = cli_ok(
        binary,
        addr,
        token,
        &["proxy", "dictionary", "get", proxy_id, dict_id],
    )
    .await?;
    assert!(shown.contains("/d.dict"), "{shown}");
    let bad = &[
        &base[..],
        &["--match-pattern", "/x", "--path", "rel", "--data", "x"],
    ]
    .concat();
    let out = run_client(binary, addr, token, bad).await?;
    assert!(
        !out.status.success(),
        "a relative serving path was accepted"
    );
    cli_ok(
        binary,
        addr,
        token,
        &["proxy", "dictionary", "delete", proxy_id, dict_id],
    )
    .await?;
    let out = run_client(
        binary,
        addr,
        token,
        &["proxy", "dictionary", "get", proxy_id, dict_id],
    )
    .await?;
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("404"),
        "{out:?}"
    );
    Ok(())
}

/// Runs the client and fails with its stderr when the command does not succeed.
async fn cli_ok(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    args: &[&str],
) -> anyhow::Result<String> {
    let out = run_client(binary, addr, token, args).await?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    anyhow::ensure!(
        out.status.success(),
        "r3 {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(stdout)
}

/// The id in the first column of the list line that names `name`.
fn listed_id(listing: &str, name: &str) -> anyhow::Result<String> {
    listing
        .lines()
        .find(|line| line.contains(name))
        .and_then(|line| line.split_whitespace().next())
        .map(str::to_string)
        .with_context(|| format!("{name} is not in the list: {listing}"))
}

/// `r3 acl` reaches the access list API (OpenAPI paths `/api/access_lists` and
/// `/api/access_lists/{id}`): a list it adds is listed, read, changed and deleted.
async fn assert_cli_access_lists(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
) -> anyhow::Result<()> {
    let added = json!({ "name": "cli-acl", "ip_filter": { "allow": ["10.0.0.0/8"] } });
    cli_ok(binary, addr, token, &["acl", "create", &added.to_string()]).await?;
    let id = listed_id(
        &cli_ok(binary, addr, token, &["acl", "list"]).await?,
        "cli-acl",
    )?;
    let changed = json!({ "name": "cli-acl", "ip_filter": { "deny": ["192.0.2.0/24"] } });
    cli_ok(
        binary,
        addr,
        token,
        &["acl", "update", &id, &changed.to_string()],
    )
    .await?;
    let shown = cli_ok(binary, addr, token, &["acl", "get", &id]).await?;
    assert!(shown.contains("192.0.2.0/24"), "{shown}");
    assert!(!shown.contains("10.0.0.0/8"), "{shown}");
    cli_ok(binary, addr, token, &["acl", "delete", &id]).await?;
    let listing = cli_ok(binary, addr, token, &["acl", "list"]).await?;
    assert!(!listing.contains("cli-acl"), "{listing}");
    // OpenAPI `get_access_list`: the server answers the deleted id with 404 itself.
    let out = run_client(binary, addr, token, &["acl", "get", &id]).await?;
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("404"),
        "{out:?}"
    );
    Ok(())
}

/// `r3 dns` reaches the DNS credential API (OpenAPI paths `/api/dns_credentials` and
/// `/api/dns_credentials/{id}`): a credential it adds is listed, read, renamed and deleted, and
/// the read never shows its token, because the API returns no secret.
async fn assert_cli_dns_credentials(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
) -> anyhow::Result<()> {
    let secret = "cli-dns-secret-token";
    let provider =
        json!({ "provider": "webhook", "url": "https://dns.example.com/hook", "token": secret });
    let added = json!({ "name": "cli-dns", "dns_provider": provider });
    cli_ok(binary, addr, token, &["dns", "create", &added.to_string()]).await?;
    let id = listed_id(
        &cli_ok(binary, addr, token, &["dns", "list"]).await?,
        "cli-dns",
    )?;
    let renamed = json!({ "name": "cli-dns-renamed" });
    cli_ok(
        binary,
        addr,
        token,
        &["dns", "update", &id, &renamed.to_string()],
    )
    .await?;
    let shown = cli_ok(binary, addr, token, &["dns", "get", &id]).await?;
    assert!(shown.contains("cli-dns-renamed"), "{shown}");
    assert!(!shown.contains(secret), "{shown}");
    cli_ok(binary, addr, token, &["dns", "delete", &id]).await?;
    let listing = cli_ok(binary, addr, token, &["dns", "list"]).await?;
    assert!(!listing.contains("cli-dns"), "{listing}");
    Ok(())
}

/// The client binary of the workspace, built up to date before a test runs it.
pub(crate) fn client_binary() -> anyhow::Result<std::path::PathBuf> {
    let binary = match std::env::var("CARGO_TARGET_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir).join("debug/r3"),
        Err(_) => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug/r3"),
    };
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "r3v3rs3-cli"])
        .status()
        .context("failed to run cargo")?;
    anyhow::ensure!(status.success(), "the client binary did not build");
    Ok(binary)
}

/// `r3 target get` and `r3 target create-token` reach the target API (OpenAPI: `get_target` and
/// `create_target_token` at `/api/targets/{id}/token`), and `r3 platform get-app-info` shows the release
/// version and the worker mode, the same data as the MCP tool `server_status`.
#[tokio::test]
async fn the_client_binary_reads_targets_and_the_server_status() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-targets")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    config.platform.agent_port = Some(alloc_tcp_port().await?.socket_addr().port());
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        let binary = client_binary()?;
        let added = cli_ok(
            &binary,
            &addr,
            &token,
            &["target", "create", "--name", "cli-edge"],
        )
        .await?;
        let id = added
            .split("id=")
            .nth(1)
            .and_then(|rest| rest.split(',').next())
            .context("no target id in the add output")?
            .to_string();
        assert_cli_target_get(&binary, &addr, &token, &id).await?;
        let renewed = cli_ok(&binary, &addr, &token, &["target", "create-token", &id]).await?;
        assert!(renewed.contains("\"token\""), "{renewed}");
        let status = cli_ok(&binary, &addr, &token, &["platform", "get-app-info"]).await?;
        assert!(status.contains("\"version\""), "{status}");
        assert!(status.contains("\"mode\""), "{status}");
        Ok(())
    })
    .await
}

/// `r3 target get` (OpenAPI `get_target`) shows the target it names, and the server itself
/// answers an unknown id with 404.
async fn assert_cli_target_get(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    id: &str,
) -> anyhow::Result<()> {
    let shown = cli_ok(binary, addr, token, &["target", "get", id]).await?;
    assert!(shown.contains("cli-edge"), "{shown}");
    let out = run_client(binary, addr, token, &["target", "get", MISSING_DB]).await?;
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("404"),
        "{out:?}"
    );
    Ok(())
}

/// An id that names no database, so every call that takes it must answer "not found".
const MISSING_DB: &str = "zzz-zzz";

/// Asserts that a call failed with the API's not-found answer (OpenAPI `NotFoundResponse`),
/// which proves the tool or command reached the endpoint instead of failing before it.
fn assert_not_found(what: &str, err: &str) {
    assert!(err.to_lowercase().contains("not found"), "{what}: {err}");
    assert!(!err.contains("tool not found"), "{what}: {err}");
}

/// The database and backup tools reach their endpoints (OpenAPI `list_all_backups`,
/// `get_backups_overview`, `list_database_replicas`, `clone_database`,
/// `create_database_replica`, `delete_database`, `restore_database_backup`).
async fn assert_mcp_databases(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let all: Value = serde_json::from_str(&mcp_call(client, "list_all_backups", json!({})).await?)?;
    assert!(all.is_array(), "{all}");
    let overview: Value =
        serde_json::from_str(&mcp_call(client, "get_backups_overview", json!({})).await?)?;
    assert!(overview.is_object(), "{overview}");
    let named = json!({ "name": "copy" });
    let calls = [
        ("list_database_replicas", json!({ "id": MISSING_DB })),
        ("delete_database", json!({ "id": MISSING_DB })),
        (
            "clone_database",
            json!({ "id": MISSING_DB, "request": named }),
        ),
        (
            "create_database_replica",
            json!({ "id": MISSING_DB, "request": named }),
        ),
        (
            "restore_database_backup",
            json!({ "db_id": MISSING_DB, "backup_id": "b1" }),
        ),
    ];
    for (name, args) in calls {
        let err = mcp_call(client, name, args)
            .await
            .err()
            .map(|e| e.to_string());
        assert_not_found(name, &err.unwrap_or_default());
    }
    // Without S3 settings both S3 calls are refused (`S3NotConfigured`).
    let s3_calls = [
        ("list_s3_database_backups", json!({})),
        (
            "restore_s3_database_backup",
            json!({ "request": { "key": "x.sql.gz" } }),
        ),
    ];
    for (name, args) in s3_calls {
        let err = mcp_call(client, name, args)
            .await
            .err()
            .map(|e| e.to_string());
        assert_s3_not_configured(name, &err.unwrap_or_default());
    }
    Ok(())
}

fn assert_s3_not_configured(what: &str, err: &str) {
    assert!(err.to_lowercase().contains("s3"), "{what}: {err}");
    assert!(
        !err.contains("tool not found") && !err.contains("unrecognized"),
        "{what}: {err}"
    );
}

/// The CLI reaches the same endpoints, plus the S3 list and the backup run.
async fn assert_cli_databases(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    let all = cli_ok(binary, addr, token, &["backup", "list-all"]).await?;
    assert!(all.trim_start().starts_with('['), "{all}");
    let overview = cli_ok(binary, addr, token, &["backup", "get-overview"]).await?;
    assert!(overview.trim_start().starts_with('{'), "{overview}");
    let calls: [&[&str]; 2] = [
        &["db", "list-replicas", MISSING_DB],
        &["backup", "run-now", MISSING_DB],
    ];
    for args in calls {
        let out = run_client(binary, addr, token, args).await?;
        assert_not_found(&args.join(" "), &String::from_utf8_lossy(&out.stderr));
    }
    let s3_calls: [&[&str]; 2] = [
        &["backup", "list-s3"],
        &["backup", "restore-s3", "x.sql.gz"],
    ];
    for args in s3_calls {
        let out = run_client(binary, addr, token, args).await?;
        assert_s3_not_configured(&args.join(" "), &String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// `refresh_database_engines` (MCP) and `r3 db refresh-engines` change the catalog, so a token
/// with only the read scope is refused (403 forbidden), never let through to the download.
async fn assert_engine_refresh_needs_edit(
    binary: &Path,
    addr: &SocketAddr,
    read_token: &str,
) -> anyhow::Result<()> {
    let client = mcp_client(*addr, read_token).await?;
    let err = mcp_call(&client, "refresh_database_engines", json!({}))
        .await
        .err();
    let err = err.map(|e| e.to_string()).unwrap_or_default();
    assert!(
        err.contains("scope") && !err.contains("tool not found"),
        "{err}"
    );
    let out = run_client(binary, addr, read_token, &["db", "refresh-engines"]).await?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("403"), "{stderr}");
    Ok(())
}

/// Database and backup operations are reachable from MCP and the CLI.
#[tokio::test]
async fn the_database_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-databases")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        let client = mcp_client(addr, &token).await?;
        assert_mcp_databases(&client).await?;
        let binary = client_binary()?;
        assert_cli_databases(&binary, &addr, &token).await?;
        let read_token = access_token_with_scope(&addr, "read").await?;
        assert_engine_refresh_needs_edit(&binary, &addr, &read_token).await
    })
    .await
}

/// Asserts that a call reached its endpoint: it succeeded, or the API itself refused it.
/// A missing tool ("tool not found"), a missing command ("unrecognized subcommand") or a
/// missing route (`{"message":"NOT_FOUND"`) fails the assertion.
fn assert_reached(what: &str, outcome: &Result<String, String>) {
    if let Err(err) = outcome {
        for missing in ["tool not found", "unrecognized subcommand", "\"NOT_FOUND\""] {
            assert!(!err.contains(missing), "{what}: {err}");
        }
    }
}

const GIT_TOKEN: &str = "ghp-test-token-never-shown";

/// A Git app that is created but never deployed, so no Docker engine is needed.
fn git_app(name: &str) -> Value {
    json!({ "name": name, "target": "local", "spec": {
        "source": { "type": "git", "repository": "https://git.example.com/team/app" },
        "port": 8080 } })
}

/// The app and deployment tools reach their endpoints (OpenAPI `update_app_git_token`,
/// `delete_app_git_token`, `create_app_webhook_secret`, `delete_app_webhook_secret`,
/// `get_deployment`, `approve_deployment`, `reject_deployment`, `get_app_metrics`,
/// `get_app_services`, `get_app_topology`, `upgrade_app_catalog`, `install_app_hook`).
/// OpenAPI `update_app_hooks` replaces the hooks of an app; `get_app_hooks` reads them back.
async fn assert_mcp_hooks(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    id: &str,
) -> anyhow::Result<()> {
    let hooks = json!({ "pre_deploy": "migrate-now" });
    let args = json!({ "id": id, "hooks": hooks });
    mcp_call(client, "update_app_hooks", args).await?;
    let shown = mcp_call(client, "get_app_hooks", json!({ "id": id })).await?;
    assert_eq!(serde_json::from_str::<Value>(&shown)?, hooks);
    Ok(())
}

/// The Git token and webhook secret tools of an app; the stored token never comes back.
async fn assert_mcp_app_secrets(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    id: &str,
) -> anyhow::Result<()> {
    let with_token = mcp_call(
        client,
        "update_app_git_token",
        json!({ "id": id, "token": GIT_TOKEN }),
    )
    .await?;
    assert!(!with_token.contains(GIT_TOKEN), "{with_token}");
    mcp_call(client, "delete_app_git_token", json!({ "id": id })).await?;
    let secret = mcp_call(client, "create_app_webhook_secret", json!({ "id": id })).await?;
    assert!(secret.contains("secret"), "{secret}");
    mcp_call(client, "delete_app_webhook_secret", json!({ "id": id })).await?;
    Ok(())
}

async fn assert_mcp_apps(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let added: Value = serde_json::from_str(
        &mcp_call(client, "create_app", json!({ "app": git_app("mcp-app") })).await?,
    )?;
    let id = added["id"].as_str().context("no app id")?.to_string();
    assert_mcp_hooks(client, &id).await?;
    assert_mcp_app_secrets(client, &id).await?;
    for name in ["get_deployment", "approve_deployment", "reject_deployment"] {
        let err = mcp_call(client, name, json!({ "id": MISSING_DB }))
            .await
            .err();
        assert_not_found(name, &err.map(|e| e.to_string()).unwrap_or_default());
    }
    for name in [
        "get_app_metrics",
        "get_app_services",
        "get_app_topology",
        "upgrade_app_catalog",
        "install_app_hook",
    ] {
        let outcome = mcp_call(client, name, json!({ "id": id }))
            .await
            .map_err(|e| e.to_string());
        assert_reached(name, &outcome);
    }
    Ok(())
}

/// Runs the client with `input` on its standard input.
pub(crate) async fn run_client_stdin(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    args: &[&str],
    input: &'static str,
) -> anyhow::Result<std::process::Output> {
    use std::io::Write as _;
    let mut command = std::process::Command::new(binary);
    command
        .arg(format!("http://{addr}"))
        .arg("--token")
        .arg(token)
        .args(args);
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    tokio::task::spawn_blocking(move || {
        let mut child = command.spawn().context("failed to run the client binary")?;
        child
            .stdin
            .take()
            .context("no stdin")?
            .write_all(input.as_bytes())?;
        child
            .wait_with_output()
            .context("the client did not finish")
    })
    .await
    .map_err(|err| anyhow::anyhow!("the client task failed: {err}"))?
}

/// Runs `r3 app update-hooks <id> <flags>`, then returns what `r3 app get-hooks <id>` shows.
async fn cli_hooks_after(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    id: &str,
    flags: &[&str],
) -> anyhow::Result<Value> {
    let update = [&["app", "update-hooks", id][..], flags].concat();
    cli_ok(binary, addr, token, &update).await?;
    let shown = cli_ok(binary, addr, token, &["app", "get-hooks", id]).await?;
    Ok(serde_json::from_str(&shown)?)
}

/// `r3 app update-hooks` and `get-hooks` (OpenAPI `update_app_hooks`, `get_app_hooks`): an
/// update replaces both hooks with the given ones, so a hook left out is removed, and an
/// unknown app is not found.
async fn assert_cli_hooks(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    id: &str,
) -> anyhow::Result<()> {
    let both = ["--pre-deploy", "migrate-now", "--post-deploy", "warm-up"];
    assert_eq!(
        cli_hooks_after(binary, addr, token, id, &both).await?,
        json!({ "pre_deploy": "migrate-now", "post_deploy": "warm-up" })
    );
    let pre = ["--pre-deploy", "migrate-again"];
    assert_eq!(
        cli_hooks_after(binary, addr, token, id, &pre).await?,
        json!({ "pre_deploy": "migrate-again" })
    );
    assert_eq!(
        cli_hooks_after(binary, addr, token, id, &[]).await?,
        json!({})
    );
    let out = run_client(binary, addr, token, &["app", "get-hooks", MISSING_DB]).await?;
    assert_not_found("app get-hooks", &String::from_utf8_lossy(&out.stderr));
    Ok(())
}

/// The CLI reaches the same app and deployment endpoints; the Git token is read from
/// standard input, so it never appears in the process list.
async fn assert_cli_apps(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    cli_ok(
        binary,
        addr,
        token,
        &["app", "create", &git_app("cli-app").to_string()],
    )
    .await?;
    let id = listed_id(
        &cli_ok(binary, addr, token, &["app", "list"]).await?,
        "cli-app",
    )?;
    assert_cli_hooks(binary, addr, token, &id).await?;
    let out = run_client_stdin(
        binary,
        addr,
        token,
        &["app", "update-git-token", &id],
        GIT_TOKEN,
    )
    .await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!stdout.contains(GIT_TOKEN), "{stdout}");
    cli_ok(binary, addr, token, &["app", "delete-git-token", &id]).await?;
    let secret = cli_ok(binary, addr, token, &["app", "create-webhook-secret", &id]).await?;
    assert!(secret.contains("secret"), "{secret}");
    cli_ok(binary, addr, token, &["app", "delete-webhook-secret", &id]).await?;
    assert_cli_app_reads(binary, addr, token, &id).await
}

/// The deployment and state commands of `r3 app` reach their endpoints.
async fn assert_cli_app_reads(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    id: &str,
) -> anyhow::Result<()> {
    let deployments = cli_ok(binary, addr, token, &["app", "list-deployments", id]).await?;
    assert!(deployments.trim_start().starts_with('['), "{deployments}");
    for cmd in ["get-deployment", "approve-deployment", "reject-deployment"] {
        let out = run_client(binary, addr, token, &["app", cmd, MISSING_DB]).await?;
        assert_not_found(cmd, &String::from_utf8_lossy(&out.stderr));
    }
    for cmd in [
        "get-metrics",
        "get-services",
        "get-topology",
        "upgrade-catalog",
        "install-hook",
        "check-update",
    ] {
        let out = run_client(binary, addr, token, &["app", cmd, id]).await?;
        let outcome = if out.status.success() {
            Ok(String::new())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).to_string())
        };
        assert_reached(cmd, &outcome);
    }
    Ok(())
}

/// App and deployment operations are reachable from MCP and the CLI.
#[tokio::test]
async fn the_app_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-apps")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        let client = mcp_client(addr, &token).await?;
        assert_mcp_apps(&client).await?;
        assert_cli_apps(&client_binary()?, &addr, &token).await
    })
    .await
}

const ENV_SECRET: &str = "env-secret-value-never-shown";

/// The id field of a JSON answer.
fn id_of(answer: &str) -> anyhow::Result<String> {
    let value: Value = serde_json::from_str(answer)?;
    Ok(value["id"]
        .as_str()
        .context("no id in the answer")?
        .to_string())
}

/// The project tools read and rename a project (OpenAPI `get_project`, `update_project`).
async fn assert_mcp_project(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<String> {
    let spec = json!({ "name": "mcp-proj", "default_environments": [] });
    let id = id_of(&mcp_call(client, "create_project", json!({ "spec": spec })).await?)?;
    let renamed = json!({ "name": "mcp-proj-2", "default_environments": [] });
    mcp_call(
        client,
        "update_project",
        json!({ "id": id, "spec": renamed }),
    )
    .await?;
    let shown = mcp_call(client, "get_project", json!({ "id": id })).await?;
    assert!(shown.contains("mcp-proj-2"), "{shown}");
    Ok(id)
}

/// The environment tools read, rename, compare and delete environments, and the variables
/// of an environment never show a secret value (OpenAPI `get_environment`,
/// `update_environment`, `get_environment_env`, `diff_environments`, `delete_environment`).
async fn assert_mcp_environments(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    project: &str,
) -> anyhow::Result<()> {
    let create = |name: &str| json!({ "project_id": project, "name": name });
    let env = id_of(&mcp_call(client, "create_environment", create("staging")).await?)?;
    let other = id_of(&mcp_call(client, "create_environment", create("prod")).await?)?;
    let pair = json!({ "project_id": project, "environment_id": env });
    let rename =
        json!({ "project_id": project, "environment_id": env, "request": { "name": "stage" } });
    mcp_call(client, "update_environment", rename).await?;
    let shown = mcp_call(client, "get_environment", pair.clone()).await?;
    assert!(shown.contains("stage"), "{shown}");
    assert_mcp_environment_vars(client, project, &env, &other).await
}

/// The variables of an environment and the comparison of two environments never show a
/// secret value, and a deleted environment is not found.
async fn assert_mcp_environment_vars(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    project: &str,
    env: &str,
    other: &str,
) -> anyhow::Result<()> {
    let pair = json!({ "project_id": project, "environment_id": env });
    let vars = json!([{ "key": "API_KEY", "value": ENV_SECRET, "secret": true }]);
    mcp_call(
        client,
        "update_environment_env",
        json!({ "environment_id": env, "variables": vars }),
    )
    .await?;
    let listed = mcp_call(client, "get_environment_env", pair.clone()).await?;
    assert!(
        listed.contains("API_KEY") && !listed.contains(ENV_SECRET),
        "{listed}"
    );
    let diff = json!({ "project_id": project, "source_id": env, "target_id": other });
    let report = mcp_call(client, "diff_environments", diff).await?;
    assert!(
        report.contains("API_KEY") && !report.contains(ENV_SECRET),
        "{report}"
    );
    mcp_call(client, "delete_environment", pair.clone()).await?;
    let gone = mcp_call(client, "get_environment", pair).await.err();
    assert_not_found(
        "get_environment",
        &gone.map(|e| e.to_string()).unwrap_or_default(),
    );
    Ok(())
}

/// The env group tools read and rename a group without its secret values (OpenAPI
/// `get_env_group`, `update_env_group`).
async fn assert_mcp_env_groups(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let vars = json!([{ "key": "TOKEN", "value": ENV_SECRET, "secret": true }]);
    let group = json!({ "name": "mcp-grp", "variables": vars });
    let id = id_of(&mcp_call(client, "create_env_group", json!({ "group": group })).await?)?;
    let shown = mcp_call(client, "get_env_group", json!({ "id": id })).await?;
    assert!(
        shown.contains("TOKEN") && !shown.contains(ENV_SECRET),
        "{shown}"
    );
    let renamed = json!({ "id": id, "group": { "name": "mcp-grp-2", "variables": [] } });
    mcp_call(client, "update_env_group", renamed).await?;
    let shown = mcp_call(client, "get_env_group", json!({ "id": id })).await?;
    assert!(shown.contains("mcp-grp-2"), "{shown}");
    Ok(())
}

/// `r3 project` and `r3 env-group` reach the same endpoints, without secret values.
async fn assert_cli_projects(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    cli_ok(
        binary,
        addr,
        token,
        &["project", "create", "--name", "cli-proj"],
    )
    .await?;
    let id = listed_id(
        &cli_ok(binary, addr, token, &["project", "list"]).await?,
        "cli-proj",
    )?;
    cli_ok(
        binary,
        addr,
        token,
        &["project", "update", &id, "--name", "cli-proj-2"],
    )
    .await?;
    let shown = cli_ok(binary, addr, token, &["project", "get", &id]).await?;
    assert!(shown.contains("cli-proj-2"), "{shown}");
    assert_cli_project_envs(binary, addr, token, &id).await?;
    assert_cli_env_groups(binary, addr, token).await
}

/// `r3 project env-*` creates, renames and reads environments of the project `id`.
async fn assert_cli_project_envs(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    id: &str,
) -> anyhow::Result<()> {
    let env = id_of(
        &cli_ok(
            binary,
            addr,
            token,
            &["project", "create-environment", id, "--name", "qa"],
        )
        .await?,
    )?;
    let other = id_of(
        &cli_ok(
            binary,
            addr,
            token,
            &["project", "create-environment", id, "--name", "live"],
        )
        .await?,
    )?;
    cli_ok(
        binary,
        addr,
        token,
        &["project", "update-environment", id, &env, "--name", "qa-2"],
    )
    .await?;
    let shown = cli_ok(
        binary,
        addr,
        token,
        &["project", "get-environment", id, &env],
    )
    .await?;
    assert!(shown.contains("qa-2"), "{shown}");
    assert_cli_env_vars(binary, addr, token, id, (&env, &other)).await
}

/// `r3 project update-environment-env`, `env-vars` and `env-diff` never show a secret value, and
/// `env-delete` removes the environment.
async fn assert_cli_env_vars(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    id: &str,
    (env, other): (&str, &str),
) -> anyhow::Result<()> {
    let vars = json!({ "variables": [{ "key": "API_KEY", "value": ENV_SECRET, "secret": true }] });
    cli_ok(
        binary,
        addr,
        token,
        &[
            "project",
            "update-environment-env",
            id,
            env,
            &vars.to_string(),
        ],
    )
    .await?;
    let listed = cli_ok(
        binary,
        addr,
        token,
        &["project", "get-environment-env", id, env],
    )
    .await?;
    assert!(
        listed.contains("API_KEY") && !listed.contains(ENV_SECRET),
        "{listed}"
    );
    let report = cli_ok(
        binary,
        addr,
        token,
        &["project", "diff-environments", id, env, other],
    )
    .await?;
    assert!(!report.contains(ENV_SECRET), "{report}");
    cli_ok(
        binary,
        addr,
        token,
        &["project", "delete-environment", id, env],
    )
    .await?;
    Ok(())
}

async fn assert_cli_env_groups(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
) -> anyhow::Result<()> {
    let vars = json!([{ "key": "TOKEN", "value": ENV_SECRET, "secret": true }]);
    let group = json!({ "name": "cli-grp", "variables": vars });
    cli_ok(
        binary,
        addr,
        token,
        &["env-group", "create", &group.to_string()],
    )
    .await?;
    let id = listed_id(
        &cli_ok(binary, addr, token, &["env-group", "list"]).await?,
        "cli-grp",
    )?;
    let shown = cli_ok(binary, addr, token, &["env-group", "get", &id]).await?;
    assert!(
        shown.contains("TOKEN") && !shown.contains(ENV_SECRET),
        "{shown}"
    );
    let renamed = json!({ "name": "cli-grp-2", "variables": [] });
    cli_ok(
        binary,
        addr,
        token,
        &["env-group", "update", &id, &renamed.to_string()],
    )
    .await?;
    cli_ok(binary, addr, token, &["env-group", "delete", &id]).await?;
    let listing = cli_ok(binary, addr, token, &["env-group", "list"]).await?;
    assert!(!listing.contains("cli-grp"), "{listing}");
    Ok(())
}

/// Project, environment and env group operations are reachable from MCP and the CLI.
#[tokio::test]
async fn the_project_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-projects")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        let client = mcp_client(addr, &token).await?;
        let project = assert_mcp_project(&client).await?;
        assert_mcp_environments(&client, &project).await?;
        assert_mcp_env_groups(&client).await?;
        assert_cli_projects(&client_binary()?, &addr, &token).await
    })
    .await
}

const GIT_SECRET: &str = "gitea-client-secret-never-shown";
const CALLBACK: &str = "https://panel.example.com/oauth/git/callback";

/// The Git connection tools rename a connection without showing its client secret, start the
/// OAuth authorization (RFC 6749 4.1.1: the URL carries the `client_id`), create a GitHub App
/// manifest form and refuse a GitHub installation on a Gitea connection (OpenAPI
/// `update_git_connection`, `authorize_git_connection`, `create_github_app`,
/// `install_github_app`).
async fn assert_mcp_git(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let add = json!({ "name": "mcp-gitea", "provider": "gitea", "url": "https://gitea.example.com",
        "client_id": "mcp-client-id", "client_secret": GIT_SECRET });
    let id = id_of(&mcp_call(client, "create_git_connection", add).await?)?;
    let request = json!({ "name": "mcp-gitea-2", "provider": "gitea",
        "url": "https://gitea.example.com", "client_id": "mcp-client-id" });
    let updated = mcp_call(
        client,
        "update_git_connection",
        json!({ "id": id, "request": request }),
    )
    .await?;
    assert!(
        updated.contains("mcp-gitea-2") && !updated.contains(GIT_SECRET),
        "{updated}"
    );
    let auth = json!({ "id": id, "request": { "redirect_uri": CALLBACK } });
    let url = mcp_call(client, "authorize_git_connection", auth).await?;
    assert!(
        url.contains("https://gitea.example.com") && url.contains("mcp-client-id"),
        "{url}"
    );
    let form = mcp_call(
        client,
        "create_github_app",
        json!({ "request": { "name": "mcp-gh", "redirect_uri": CALLBACK } }),
    )
    .await?;
    assert!(form.contains("manifest"), "{form}");
    let refused = mcp_call(client, "install_github_app", json!({ "id": id })).await;
    assert!(
        refused.is_err(),
        "a Gitea connection took a GitHub installation"
    );
    assert_reached("install_github_app", &refused.map_err(|e| e.to_string()));
    Ok(())
}

/// `r3 git` reaches the same endpoints.
async fn assert_cli_git(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    let add = [
        "git",
        "create",
        "--name",
        "cli-gitea",
        "--provider",
        "gitea",
        "--url",
        "https://gitea.example.com",
        "--client-id",
        "cli-client-id",
        "--client-secret",
        GIT_SECRET,
    ];
    cli_ok(binary, addr, token, &add).await?;
    let id = listed_id(
        &cli_ok(binary, addr, token, &["git", "list"]).await?,
        "cli-gitea",
    )?;
    let request = json!({ "name": "cli-gitea-2", "provider": "gitea",
        "url": "https://gitea.example.com", "client_id": "cli-client-id" });
    let updated = cli_ok(
        binary,
        addr,
        token,
        &["git", "update", &id, &request.to_string()],
    )
    .await?;
    assert!(
        updated.contains("cli-gitea-2") && !updated.contains(GIT_SECRET),
        "{updated}"
    );
    let url = cli_ok(
        binary,
        addr,
        token,
        &["git", "authorize", &id, "--redirect-uri", CALLBACK],
    )
    .await?;
    assert!(url.contains("cli-client-id"), "{url}");
    let form = cli_ok(
        binary,
        addr,
        token,
        &[
            "git",
            "create-github-app",
            "--name",
            "cli-gh",
            "--redirect-uri",
            CALLBACK,
        ],
    )
    .await?;
    assert!(form.contains("manifest"), "{form}");
    let out = run_client(binary, addr, token, &["git", "install-github-app", &id]).await?;
    assert!(!out.status.success());
    assert_reached(
        "git install",
        &Err(String::from_utf8_lossy(&out.stderr).to_string()),
    );
    Ok(())
}

/// Git connection operations are reachable from MCP and the CLI.
#[tokio::test]
async fn the_git_connection_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-git")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        assert_mcp_git(&mcp_client(addr, &token).await?).await?;
        assert_cli_git(&client_binary()?, &addr, &token).await
    })
    .await
}

/// A PEM certificate and its private key for `name`.
fn pem_pair(name: &str) -> anyhow::Result<(String, String)> {
    let pair = rcgen::generate_simple_self_signed(vec![name.to_string()])?;
    Ok((pair.cert.pem(), pair.signing_key.serialize_pem()))
}

/// The id of the certificate whose SAN list names `name`.
fn cert_id(listing: &str, name: &str) -> anyhow::Result<String> {
    let certs: Value = serde_json::from_str(listing)?;
    let found = certs.as_array().into_iter().flatten().find(|cert| {
        cert["san"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|san| san == name)
    });
    Ok(found.context(format!("no certificate for {name}"))?["id"]
        .as_str()
        .context("no id")?
        .to_string())
}

/// The certificate tools sign, upload, read and delete certificates. A downloaded chain never
/// holds the private key, a broken PEM is refused, and a missing ACME entry is not found
/// (OpenAPI `create_self_signed_cert`, `upload_cert`, `download_cert`, `delete_many_certs`, `verify_cname`,
/// `update_acme`).
async fn assert_mcp_certs(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let sign = json!({ "request": { "san": ["signed.example.test"] } });
    mcp_call(client, "create_self_signed_cert", sign).await?;
    let (chain, key) = pem_pair("uploaded.example.test")?;
    mcp_call(client, "upload_cert", json!({ "chain": chain, "key": key })).await?;
    let broken = mcp_call(
        client,
        "upload_cert",
        json!({ "chain": "not a pem", "key": key }),
    )
    .await;
    assert!(broken.is_err(), "a broken PEM was stored");
    assert_reached("upload_cert", &broken.map_err(|e| e.to_string()));
    assert_mcp_cert_download(client).await
}

/// `download_cert` returns the chain without the private key, and `delete_many_certs` removes both
/// certificates.
async fn assert_mcp_cert_download(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let listing = mcp_call(client, "list_certs", json!({})).await?;
    let uploaded = cert_id(&listing, "uploaded.example.test")?;
    let signed = cert_id(&listing, "signed.example.test")?;
    let pem = mcp_call(client, "download_cert", json!({ "id": uploaded })).await?;
    assert!(
        pem.contains("BEGIN CERTIFICATE") && !pem.contains("PRIVATE KEY"),
        "{pem}"
    );
    let deleted = json!({ "request": { "ids": [uploaded, signed] } });
    mcp_call(client, "delete_many_certs", deleted).await?;
    let listing = mcp_call(client, "list_certs", json!({})).await?;
    assert!(!listing.contains("uploaded.example.test"), "{listing}");
    assert_mcp_cert_checks(client).await
}

/// `verify_cname` reports a domain without a CNAME as invalid (RFC 6761: `.invalid` never
/// resolves), and `update_acme` on a missing entry is not found.
async fn assert_mcp_cert_checks(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let check = json!({ "domain": "cname.invalid" });
    let cname = mcp_call(client, "verify_cname", check).await?;
    assert!(cname.contains("\"is_valid\": false"), "{cname}");
    let acme = json!({ "id": MISSING_DB, "request": {} });
    let missing = mcp_call(client, "update_acme", acme).await.err();
    assert_not_found(
        "update_acme",
        &missing.map(|e| e.to_string()).unwrap_or_default(),
    );
    Ok(())
}

/// `r3 cert` signs, downloads, deletes and checks certificates the same way.
async fn assert_cli_certs(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    cli_ok(
        binary,
        addr,
        token,
        &["cert", "create-self-signed", "--san", "cli.example.test"],
    )
    .await?;
    let listing = cli_ok(binary, addr, token, &["cert", "list"]).await?;
    let id = listed_id(&listing, "cli.example.test")?;
    let out = std::env::temp_dir().join(format!("r3-cert-{id}.tar.gz"));
    let out_path = out.to_string_lossy().to_string();
    cli_ok(
        binary,
        addr,
        token,
        &["cert", "download", &id, "--output", &out_path],
    )
    .await?;
    let archive = std::fs::read(&out)?;
    std::fs::remove_file(&out)?;
    assert_eq!(
        archive.get(..2),
        Some(&[0x1f, 0x8b][..]),
        "not a gzip archive (RFC 1952)"
    );
    let cname = cli_ok(
        binary,
        addr,
        token,
        &["cert", "verify-cname", "--domain", "cname.invalid"],
    )
    .await?;
    assert!(cname.contains("\"is_valid\": false"), "{cname}");
    cli_ok(binary, addr, token, &["cert", "delete-many", &id]).await?;
    let listing = cli_ok(binary, addr, token, &["cert", "list"]).await?;
    assert!(!listing.contains("cli.example.test"), "{listing}");
    Ok(())
}

/// Certificate operations are reachable from MCP and the CLI.
#[tokio::test]
async fn the_certificate_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-certs")?;
    with_admin_config(&dir.0, mcp_config(), |addr| async move {
        let token = access_token(&addr).await?;
        assert_mcp_certs(&mcp_client(addr, &token).await?).await?;
        assert_cli_certs(&client_binary()?, &addr, &token).await
    })
    .await
}

/// A proxy that sends `/api` of app.example.test to a closed local port.
fn dry_run_request() -> Value {
    json!({
        "proxy": { "vhosts": ["app.example.test"],
            "routes": [{ "path": "/api", "servers": [{ "url": "http://127.0.0.1:9" }] }] },
        "url": "http://app.example.test/api/items"
    })
}

/// A replay to a loopback address without `allow_internal`, which must be refused (OWASP
/// SSRF prevention).
fn loopback_replay() -> Value {
    json!({ "method": "GET", "url": "http://127.0.0.1:9/", "headers": [] })
}

/// The traffic tools filter the access log, do not find a missing entry and refuse a replay to
/// a loopback address (OpenAPI `get_traffic`, `get_traffic_entry`, `get_traffic_geo`,
/// `replay_traffic_request`).
async fn assert_mcp_traffic(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let filter = json!({ "resource_id": MISSING_DB });
    let listed = mcp_call(client, "get_traffic", filter.clone()).await?;
    assert_eq!(
        serde_json::from_str::<Value>(&listed)?,
        json!([]),
        "{listed}"
    );
    mcp_call(client, "get_traffic_geo", filter).await?;
    let missing = mcp_call(client, "get_traffic_entry", json!({ "id": u64::MAX }))
        .await
        .err();
    assert_not_found(
        "get_traffic_entry",
        &missing.map(|e| e.to_string()).unwrap_or_default(),
    );
    let replay = json!({ "request": loopback_replay() });
    let refused = mcp_call(client, "replay_traffic_request", replay).await;
    assert!(refused.is_err(), "a replay to a loopback address was sent");
    assert_reached(
        "replay_traffic_request",
        &refused.map_err(|e| e.to_string()),
    );
    Ok(())
}

/// The anomaly rule tools store the rules they are given, the header analysis reports a missing
/// Strict-Transport-Security header (OWASP Secure Headers), and the dry run matches the route
/// (OpenAPI `get_anomaly_rules`, `update_anomaly_rules`, `dismiss_anomalies`,
/// `analyze_security_headers`, `preview_proxy`).
async fn assert_mcp_checks(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let rules = json!({ "enabled": true, "max_p99_latency_ms": 1234, "max_5xx_percent": 7,
        "max_request_rate_rps": null, "max_event_loop_lag_ms": null });
    mcp_call(client, "update_anomaly_rules", json!({ "request": rules })).await?;
    let stored = mcp_call(client, "get_anomaly_rules", json!({})).await?;
    assert!(
        stored.contains("1234") && stored.contains("\"max_5xx_percent\": 7"),
        "{stored}"
    );
    mcp_call(client, "dismiss_anomalies", json!({})).await?;
    let audit = mcp_call(
        client,
        "analyze_security_headers",
        json!({ "request": { "rules": [] } }),
    )
    .await?;
    assert!(
        audit.to_lowercase().contains("strict-transport-security"),
        "{audit}"
    );
    let run = mcp_call(
        client,
        "preview_proxy",
        json!({ "request": dry_run_request() }),
    )
    .await?;
    assert!(
        run.contains("\"matched\": true") && run.contains("127.0.0.1:9"),
        "{run}"
    );
    Ok(())
}

/// `r3 traffic`, `r3 security` and `r3 proxy preview` reach the same endpoints.
async fn assert_cli_traffic(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    let listed = cli_ok(
        binary,
        addr,
        token,
        &["traffic", "get", "--resource-id", MISSING_DB],
    )
    .await?;
    assert_eq!(
        serde_json::from_str::<Value>(&listed)?,
        json!([]),
        "{listed}"
    );
    let out = run_client(
        binary,
        addr,
        token,
        &["traffic", "get-entry", "18446744073709551615"],
    )
    .await?;
    assert_not_found("traffic get-entry", &String::from_utf8_lossy(&out.stderr));
    let out = run_client(
        binary,
        addr,
        token,
        &["traffic", "replay-request", &loopback_replay().to_string()],
    )
    .await?;
    assert!(
        !out.status.success(),
        "a replay to a loopback address was sent"
    );
    assert_reached(
        "traffic replay-request",
        &Err(String::from_utf8_lossy(&out.stderr).to_string()),
    );
    assert_cli_security_checks(binary, addr, token).await
}

/// `r3 security` stores anomaly rules and grades headers, and `r3 proxy preview` matches the
/// route.
async fn assert_cli_security_checks(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
) -> anyhow::Result<()> {
    let rules = json!({ "enabled": true, "max_p99_latency_ms": 4321 });
    cli_ok(
        binary,
        addr,
        token,
        &["security", "update-anomaly-rules", &rules.to_string()],
    )
    .await?;
    let stored = cli_ok(binary, addr, token, &["security", "get-anomaly-rules"]).await?;
    assert!(stored.contains("4321"), "{stored}");
    cli_ok(binary, addr, token, &["security", "get-system-anomalies"]).await?;
    cli_ok(binary, addr, token, &["security", "dismiss-anomalies"]).await?;
    let audit = cli_ok(
        binary,
        addr,
        token,
        &["security", "analyze-headers", "{\"rules\":[]}"],
    )
    .await?;
    assert!(
        audit.to_lowercase().contains("strict-transport-security"),
        "{audit}"
    );
    let run = cli_ok(
        binary,
        addr,
        token,
        &["proxy", "preview", &dry_run_request().to_string()],
    )
    .await?;
    assert!(run.contains("\"matched\": true"), "{run}");
    Ok(())
}

/// Traffic, anomaly, header analysis and dry-run operations are reachable from MCP and the CLI.
#[tokio::test]
async fn the_traffic_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-traffic")?;
    with_admin_config(&dir.0, mcp_config(), |addr| async move {
        let token = access_token(&addr).await?;
        let client = mcp_client(addr, &token).await?;
        assert_mcp_traffic(&client).await?;
        assert_mcp_checks(&client).await?;
        assert_cli_traffic(&client_binary()?, &addr, &token).await
    })
    .await
}

/// The status tools read the CDN ranges, discovery sources, a log and the dashboard
/// (OpenAPI `get_cdn_status`, `list_discovery_status`, `get_logs`, `get_dashboard`).
async fn assert_mcp_status(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let cdn = mcp_call(client, "get_cdn_status", json!({})).await?;
    assert!(cdn.contains("\"providers\""), "{cdn}");
    let sources: Value =
        serde_json::from_str(&mcp_call(client, "list_discovery_status", json!({})).await?)?;
    assert!(sources.is_array(), "{sources}");
    let rows: Value = serde_json::from_str(
        &mcp_call(client, "get_logs", json!({ "id": "server", "limit": 5 })).await?,
    )?;
    assert!(
        rows.as_array().is_some_and(|rows| rows.len() <= 5),
        "{rows}"
    );
    let dashboard = mcp_call(client, "get_dashboard", json!({})).await?;
    assert!(
        dashboard.contains("\"ports\"") && dashboard.contains("\"proxies\""),
        "{dashboard}"
    );
    Ok(())
}

/// A viewer may read the CDN status but may not download new ranges (OpenAPI
/// `refresh_cdn_ranges` needs the edit permission).
async fn assert_viewer_cannot_refresh_cdn(addr: SocketAddr, binary: &Path) -> anyhow::Result<()> {
    create_account_as_admin(addr, "viewer", "viewer").await?;
    let token = access_token_for(&addr, "viewer", "password").await?;
    let client = mcp_client(addr, &token).await?;
    let refused = mcp_call(&client, "refresh_cdn_ranges", json!({})).await;
    assert!(refused.is_err(), "a viewer refreshed the CDN ranges");
    assert_reached("refresh_cdn_ranges", &refused.map_err(|e| e.to_string()));
    let out = run_client(binary, &addr, &token, &["cdn", "refresh-ranges"]).await?;
    assert!(!out.status.success(), "a viewer refreshed the CDN ranges");
    assert_reached(
        "cdn refresh",
        &Err(String::from_utf8_lossy(&out.stderr).to_string()),
    );
    Ok(())
}

/// `r3 cdn`, `discovery`, `logs`, `dashboard`, `audit` and `port` read the same endpoints.
async fn assert_cli_status(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    let cdn = cli_ok(binary, addr, token, &["cdn", "get-status"]).await?;
    assert!(cdn.contains("\"providers\""), "{cdn}");
    cli_ok(binary, addr, token, &["discovery", "list-status"]).await?;
    cli_ok(binary, addr, token, &["get-logs", "server", "--limit", "5"]).await?;
    let dashboard = cli_ok(binary, addr, token, &["get-dashboard"]).await?;
    assert!(dashboard.contains("\"proxies\""), "{dashboard}");
    let audit = cli_ok(binary, addr, token, &["audit", "list-log", "--limit", "5"]).await?;
    assert!(serde_json::from_str::<Value>(&audit)?.is_array(), "{audit}");
    cli_ok(binary, addr, token, &["port", "list-network-interfaces"]).await?;
    let out = run_client(binary, addr, token, &["port", "get-status", MISSING_DB]).await?;
    assert_not_found("port status", &String::from_utf8_lossy(&out.stderr));
    Ok(())
}

/// CDN, discovery, log, dashboard, audit and port status reads are reachable from MCP and the
/// CLI, and refreshing the CDN ranges needs the edit permission.
#[tokio::test]
async fn the_status_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-status")?;
    with_admin_config(&dir.0, mcp_config(), |addr| async move {
        let token = access_token(&addr).await?;
        assert_mcp_status(&mcp_client(addr, &token).await?).await?;
        let binary = client_binary()?;
        assert_cli_status(&binary, &addr, &token).await?;
        assert_viewer_cannot_refresh_cdn(addr, &binary).await
    })
    .await
}

/// A gRPC target on a closed local port: every call to it must fail with a gRPC error.
const CLOSED_GRPC: &str = "http://127.0.0.1:9";

/// The gRPC tools report a closed target as an error, and the proxy tools do not find a missing
/// proxy (OpenAPI `health_grpc`, `invoke_grpc`, `reflect_grpc`, `invoke_proxy_grpc`,
/// `reflect_proxy_grpc`).
async fn assert_mcp_grpc(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let invoke = json!({ "target_url": CLOSED_GRPC, "service": "demo.Echo", "method": "Say",
        "payload": {}, "metadata": {} });
    let calls = [
        (
            "check_grpc_health",
            json!({ "request": { "target_url": CLOSED_GRPC } }),
        ),
        ("invoke_grpc", json!({ "request": invoke })),
    ];
    for (tool, args) in calls {
        let failed = mcp_call(client, tool, args).await;
        assert!(failed.is_err(), "{tool} reached a closed port");
        assert_reached(tool, &failed.map_err(|e| e.to_string()));
    }
    let reflect = json!({ "request": { "target_url": CLOSED_GRPC } });
    let listed = mcp_call(client, "reflect_grpc", reflect).await;
    // A closed port is an error, not a server without services.
    assert!(
        listed.is_err(),
        "reflect_grpc listed a closed port: {listed:?}"
    );
    assert_reached("reflect_grpc", &listed.map_err(|e| e.to_string()));
    for tool in ["invoke_proxy_grpc", "reflect_proxy_grpc"] {
        let args = json!({ "id": MISSING_DB, "request": invoke });
        let missing = mcp_call(client, tool, args).await.err();
        assert_not_found(tool, &missing.map(|e| e.to_string()).unwrap_or_default());
    }
    Ok(())
}

/// `r3 grpc` and `r3 proxy grpc-*` reach the same endpoints.
async fn assert_cli_grpc(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    let target = json!({ "target_url": CLOSED_GRPC }).to_string();
    let invoke = json!({ "target_url": CLOSED_GRPC, "service": "demo.Echo", "method": "Say",
        "payload": {}, "metadata": {} })
    .to_string();
    let out = run_client(binary, addr, token, &["grpc", "reflect", &target]).await?;
    assert!(!out.status.success(), "grpc reflect listed a closed port");
    assert_reached(
        "grpc reflect",
        &Err(String::from_utf8_lossy(&out.stderr).to_string()),
    );
    for args in [
        ["grpc", "check-health", &target],
        ["grpc", "invoke", &invoke],
    ] {
        let out = run_client(binary, addr, token, &args).await?;
        assert!(!out.status.success(), "{args:?} reached a closed port");
        assert_reached(
            &format!("{args:?}"),
            &Err(String::from_utf8_lossy(&out.stderr).to_string()),
        );
    }
    for args in [
        ["proxy", "invoke-grpc", MISSING_DB, &invoke],
        ["proxy", "reflect-grpc", MISSING_DB, &target],
    ] {
        let out = run_client(binary, addr, token, &args).await?;
        assert_not_found(&format!("{args:?}"), &String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

/// gRPC health, invoke and reflection are reachable from MCP and the CLI.
#[tokio::test]
async fn the_grpc_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-grpc")?;
    with_admin_config(&dir.0, mcp_config(), |addr| async move {
        let token = access_token(&addr).await?;
        assert_mcp_grpc(&mcp_client(addr, &token).await?).await?;
        assert_cli_grpc(&client_binary()?, &addr, &token).await
    })
    .await
}

/// The platform tools rotate the master key, list the MCP sessions (the caller's own session
/// among them) and refuse to revoke a missing one, and return the server facts, the Prometheus
/// metrics and the Grafana dashboard (OpenAPI `rotate_platform_key`, `list_mcp_sessions`,
/// `revoke_mcp_session`, `get_app_info`, `get_metrics`, `get_grafana_dashboard`).
async fn assert_mcp_platform_admin(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let report = mcp_call(client, "rotate_platform_key", json!({})).await?;
    assert!(report.contains("\"new_key_id\""), "{report}");
    let sessions = mcp_call(client, "list_mcp_sessions", json!({})).await?;
    assert!(
        sessions.contains(&format!("\"client_id\": \"{CLIENT_ID}\"")),
        "{sessions}"
    );
    let missing = mcp_call(client, "revoke_mcp_session", json!({ "id": MISSING_DB }))
        .await
        .err();
    assert_not_found(
        "revoke_mcp_session",
        &missing.map(|e| e.to_string()).unwrap_or_default(),
    );
    let info = mcp_call(client, "get_app_info", json!({})).await?;
    assert!(info.contains("\"version\""), "{info}");
    let metrics = mcp_call(client, "get_metrics", json!({})).await?;
    assert!(metrics.contains("r3v3rs3_rx_bytes_total"), "{metrics}");
    let grafana: Value =
        serde_json::from_str(&mcp_call(client, "get_grafana_dashboard", json!({})).await?)?;
    assert!(grafana["panels"].is_array(), "{grafana}");
    Ok(())
}

/// `r3 backup update-config` stores the configuration that `r3 backup get-config` shows, and
/// `r3 platform` lists MCP sessions and saves the Grafana dashboard.
async fn assert_cli_platform_admin(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
) -> anyhow::Result<()> {
    let config = cli_ok(binary, addr, token, &["backup", "get-config"]).await?;
    let config: Value = serde_json::from_str(&config)?;
    cli_ok(
        binary,
        addr,
        token,
        &["backup", "update-config", &config.to_string()],
    )
    .await?;
    let sessions = cli_ok(binary, addr, token, &["platform", "list-mcp-sessions"]).await?;
    assert!(sessions.contains(CLIENT_ID), "{sessions}");
    let out = run_client(
        binary,
        addr,
        token,
        &["platform", "revoke-mcp-session", MISSING_DB],
    )
    .await?;
    assert_not_found("revoke-mcp-session", &String::from_utf8_lossy(&out.stderr));
    let file = std::env::temp_dir().join(format!("r3-grafana-{}.json", std::process::id()));
    let path = file.to_string_lossy().to_string();
    cli_ok(
        binary,
        addr,
        token,
        &["platform", "get-grafana-dashboard", "--output", &path],
    )
    .await?;
    let dashboard: Value = serde_json::from_slice(&std::fs::read(&file)?)?;
    std::fs::remove_file(&file)?;
    assert!(dashboard["panels"].is_array(), "{dashboard}");
    Ok(())
}

/// Key rotation, MCP sessions, backup settings, metrics and the Grafana dashboard are reachable
/// from MCP and the CLI.
#[tokio::test]
async fn the_platform_admin_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-platform-admin")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        assert_mcp_platform_admin(&mcp_client(addr, &token).await?).await?;
        assert_cli_platform_admin(&client_binary()?, &addr, &token).await
    })
    .await
}

/// The app file tools reach their endpoints: a missing app is not found (OpenAPI
/// `list_app_files`, `download_app_file`, `upload_app_file`, `delete_app_file`).
async fn assert_mcp_app_files(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let path = json!({ "id": MISSING_DB, "path": "/data/app.txt" });
    let upload = json!({ "id": MISSING_DB, "path": "/data/app.txt", "content": "hello" });
    let calls = [
        (
            "list_app_files",
            json!({ "id": MISSING_DB, "path": "/data" }),
        ),
        ("download_app_file", path.clone()),
        ("upload_app_file", upload),
        ("delete_app_file", path),
    ];
    for (tool, args) in calls {
        let missing = mcp_call(client, tool, args).await.err();
        assert_not_found(tool, &missing.map(|e| e.to_string()).unwrap_or_default());
    }
    let oversized = "x".repeat(1024 * 1024 + 1);
    let big = json!({ "id": MISSING_DB, "path": "/data/big.txt", "content": oversized });
    let refused = mcp_call(client, "upload_app_file", big).await.err();
    let refused = refused.map(|e| e.to_string()).unwrap_or_default();
    assert!(refused.contains("larger than 1 MiB"), "{refused}");
    Ok(())
}

/// `r3 app files` reaches the same endpoints.
async fn assert_cli_app_files(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    let local = std::env::temp_dir().join(format!("r3-upload-{}.txt", std::process::id()));
    std::fs::write(&local, b"hello")?;
    let local_path = local.to_string_lossy().to_string();
    let commands: [&[&str]; 4] = [
        &["app", "files", "list", MISSING_DB, "--path", "/data"],
        &[
            "app",
            "files",
            "download",
            MISSING_DB,
            "--path",
            "/data/app.txt",
        ],
        &[
            "app",
            "files",
            "upload",
            MISSING_DB,
            "--path",
            "/data/app.txt",
            "--file",
            &local_path,
        ],
        &[
            "app",
            "files",
            "delete",
            MISSING_DB,
            "--path",
            "/data/app.txt",
        ],
    ];
    for args in commands {
        let out = run_client(binary, addr, token, args).await?;
        assert_not_found(&format!("{args:?}"), &String::from_utf8_lossy(&out.stderr));
    }
    std::fs::remove_file(&local)?;
    Ok(())
}

/// The volume backup tools reach their endpoints: a missing app is not found (OpenAPI
/// `list_volume_backups`, `create_volume_backup`, `restore_volume_backup`,
/// `delete_volume_backup`).
async fn assert_mcp_volume_backups(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let volume = json!({ "id": MISSING_DB, "volume": "data" });
    let backup = json!({ "id": MISSING_DB, "volume": "data", "backup_id": "b1" });
    let calls = [
        ("list_volume_backups", volume.clone()),
        ("create_volume_backup", volume),
        ("restore_volume_backup", backup.clone()),
        ("delete_volume_backup", backup),
    ];
    for (tool, args) in calls {
        let missing = mcp_call(client, tool, args).await.err();
        assert_not_found(tool, &missing.map(|e| e.to_string()).unwrap_or_default());
    }
    Ok(())
}

/// `r3 app volumes` reaches the volume backup and archive endpoints, including the binary
/// downloads and the archive import that only the CLI offers.
async fn assert_cli_volumes(binary: &Path, addr: &SocketAddr, token: &str) -> anyhow::Result<()> {
    let local = std::env::temp_dir().join(format!("r3-archive-{}.tar.gz", std::process::id()));
    std::fs::write(&local, [0x1f, 0x8b])?;
    let local_path = local.to_string_lossy().to_string();
    let commands: [&[&str]; 7] = [
        &["app", "volumes", "list-backups", MISSING_DB, "data"],
        &["app", "volumes", "create-backup", MISSING_DB, "data"],
        &["app", "volumes", "restore-backup", MISSING_DB, "data", "b1"],
        &["app", "volumes", "delete-backup", MISSING_DB, "data", "b1"],
        &[
            "app",
            "volumes",
            "download-backup",
            MISSING_DB,
            "data",
            "b1",
        ],
        &["app", "volumes", "export-archive", MISSING_DB, "data"],
        &[
            "app",
            "volumes",
            "import-archive",
            MISSING_DB,
            "data",
            "--file",
            &local_path,
        ],
    ];
    for args in commands {
        let out = run_client(binary, addr, token, args).await?;
        assert_not_found(&format!("{args:?}"), &String::from_utf8_lossy(&out.stderr));
    }
    std::fs::remove_file(&local)?;
    Ok(())
}

/// `r3 backup download` and `r3 backup download-s3` reach the binary backup downloads that only
/// the CLI offers (OpenAPI `download_database_backup`, `download_s3_database_backup`): a missing
/// database is not found and an unset S3 bucket is reported.
#[tokio::test]
async fn the_backup_downloads_are_reachable_from_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("cli-backup-downloads")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        let binary = client_binary()?;
        let out = run_client(
            &binary,
            &addr,
            &token,
            &["backup", "download", MISSING_DB, "b1"],
        )
        .await?;
        assert_not_found("backup download", &String::from_utf8_lossy(&out.stderr));
        let args = ["backup", "download-s3", "--key", "db/b1.sql.gz"];
        let out = run_client(&binary, &addr, &token, &args).await?;
        assert!(
            !out.status.success(),
            "a download without an S3 bucket succeeded"
        );
        assert_s3_not_configured("backup s3-download", &String::from_utf8_lossy(&out.stderr));
        Ok(())
    })
    .await
}

/// Starts `r3 <args>` and returns the first line it writes to stderr, then stops it. A live
/// stream never ends by itself, so the first line is the proof that the stream opened.
async fn first_stderr_line(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    args: &[&str],
) -> anyhow::Result<String> {
    let mut child = tokio::process::Command::new(binary)
        .arg(format!("http://{addr}"))
        .args(["--token", token])
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let stderr = child.stderr.take().context("no stderr")?;
    let mut lines = tokio::io::AsyncBufReadExt::lines(tokio::io::BufReader::new(stderr));
    let line =
        tokio::time::timeout(std::time::Duration::from_secs(20), lines.next_line()).await??;
    child.kill().await?;
    line.context("the client closed stderr without a line")
}

/// `r3 traffic stream` opens the access log stream (OpenAPI `stream_traffic`, SSE) and
/// `r3 app stream-logs` refuses a missing app (OpenAPI `app_logs_live`, WebSocket).
#[tokio::test]
async fn the_live_streams_are_reachable_from_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("cli-live-streams")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        let binary = client_binary()?;
        let line = first_stderr_line(&binary, &addr, &token, &["traffic", "stream"]).await?;
        assert!(line.contains("streaming"), "{line}");
        let out = run_client(&binary, &addr, &token, &["app", "stream-logs", MISSING_DB]).await?;
        assert!(!out.status.success());
        assert_not_found("app logs-live", &String::from_utf8_lossy(&out.stderr));
        // OpenAPI `open_app_terminal`: the terminal of a missing app is not found.
        let out = run_client(
            &binary,
            &addr,
            &token,
            &["app", "open-terminal", MISSING_DB],
        )
        .await?;
        assert!(!out.status.success());
        assert_not_found("app terminal", &String::from_utf8_lossy(&out.stderr));
        Ok(())
    })
    .await
}

/// A DR backup or restore without a passphrase is a client error (RFC 9110 15.5.1, 400 Bad
/// Request) that names the missing passphrase, not a 500 from the server.
#[tokio::test]
async fn a_dr_request_without_a_passphrase_is_refused_as_bad_request() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-dr-passphrase")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token(&addr).await?;
        let base = format!("http://{addr}/api/disaster-recovery");
        let cases = [
            (format!("{base}/backups"), json!({})),
            (format!("{base}/backups"), json!({ "passphrase": "   " })),
            (format!("{base}/backups/{MISSING_DB}/restore"), json!({})),
        ];
        for (url, body) in cases {
            let res = Client::new()
                .post(&url)
                .bearer_auth(&token)
                .json(&body)
                .send()
                .await?;
            let status = res.status().as_u16();
            let text = res.text().await?;
            assert_eq!(status, 400, "{url} {body}: {text}");
            assert!(text.contains("passphrase"), "{url} {body}: {text}");
        }
        let upload = Client::new()
            .post(format!("{base}/restore-upload"))
            .bearer_auth(&token)
            .body(b"not a bundle".to_vec())
            .send()
            .await?;
        assert_eq!(
            upload.status().as_u16(),
            400,
            "restore-upload without x-passphrase"
        );
        Ok(())
    })
    .await
}

/// Sends `method` to `url` and returns the status with the body text.
async fn dr_status(
    method: reqwest::Method,
    url: &str,
    token: &str,
) -> anyhow::Result<(u16, String)> {
    let body = json!({ "passphrase": "p" });
    let res = Client::new()
        .request(method, url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await?;
    Ok((res.status().as_u16(), res.text().await?))
}

/// A DR backup id that names no backup is 404 Not Found for download, delete and restore, and
/// an id that is not a plain name (a path escape attempt) is 400 Bad Request (RFC 9110 15.5).
#[tokio::test]
async fn an_unknown_or_invalid_dr_backup_id_is_refused() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-dr-ids")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token(&addr).await?;
        let base = format!("http://{addr}/api/disaster-recovery/backups");
        let calls = [
            (reqwest::Method::GET, "download"),
            (reqwest::Method::DELETE, ""),
            (reqwest::Method::POST, "restore"),
        ];
        for (method, tail) in calls {
            for (id, expected) in [("dr-1-missing", 404), ("..%2e", 400)] {
                let url = format!("{base}/{id}/{tail}");
                let url = url.trim_end_matches('/');
                let (status, text) = dr_status(method.clone(), url, &token).await?;
                assert_eq!(status, expected, "{method} {url}: {text}");
            }
        }
        Ok(())
    })
    .await
}

/// Downloads the first DR bundle to `path` with the CLI, compares it with the bytes the API
/// serves, and removes the file.
async fn assert_dr_download_matches(
    binary: &Path,
    addr: SocketAddr,
    token: &str,
    path: &str,
) -> anyhow::Result<()> {
    let client = mcp_client(addr, token).await?;
    let listed: Value = serde_json::from_str(
        &mcp_call(&client, "list_disaster_recovery_backups", json!({})).await?,
    )?;
    let id = listed[0]["id"].as_str().context("no DR backup listed")?;
    cli_ok(
        binary,
        &addr,
        token,
        &["dr", "download", id, "--output", path],
    )
    .await?;
    let url = format!("http://{addr}/api/disaster-recovery/backups/{id}/download");
    let served = Client::new()
        .get(url)
        .bearer_auth(token)
        .send()
        .await?
        .bytes()
        .await?;
    assert_eq!(
        std::fs::read(path)?,
        served.to_vec(),
        "the saved bundle differs"
    );
    std::fs::remove_file(path)?;
    Ok(())
}

/// `r3 dr download` saves the same bytes the API serves (OpenAPI
/// `download_disaster_recovery_backup`), and a failed download leaves no file behind.
#[tokio::test]
async fn the_dr_download_saves_the_whole_bundle() -> anyhow::Result<()> {
    let dir = TempDir::new("cli-dr-download")?;
    with_admin_config(&dir.0, mcp_config(), |addr| async move {
        let token = access_token(&addr).await?;
        let binary = client_binary()?;
        let create = [
            "dr",
            "create",
            "--no-s3",
            "--passphrase",
            "dr-test-passphrase",
        ];
        cli_ok(&binary, &addr, &token, &create).await?;
        let file = std::env::temp_dir().join(format!("r3-dr-{}.r3bundle", std::process::id()));
        let path = file.to_string_lossy().to_string();
        assert_dr_download_matches(&binary, addr, &token, &path).await?;
        let out = run_client(
            &binary,
            &addr,
            &token,
            &["dr", "download", MISSING_DB, "--output", &path],
        )
        .await?;
        assert!(!out.status.success(), "a missing bundle was saved");
        assert!(!file.exists(), "a failed download left a file");
        Ok(())
    })
    .await
}

/// Volume backup and archive operations are reachable from MCP and the CLI.
#[tokio::test]
async fn the_volume_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-volumes")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        assert_mcp_volume_backups(&mcp_client(addr, &token).await?).await?;
        assert_cli_volumes(&client_binary()?, &addr, &token).await
    })
    .await
}

/// App file operations are reachable from MCP and the CLI.
#[tokio::test]
async fn the_app_file_operations_are_reachable_from_mcp_and_the_cli() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-app-files")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        assert_mcp_app_files(&mcp_client(addr, &token).await?).await?;
        assert_cli_app_files(&client_binary()?, &addr, &token).await
    })
    .await
}

/// The command line client connects with an explicit `--token` and runs commands.
#[tokio::test]
async fn the_client_binary_runs_commands() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token(&addr).await?;
        let binary = client_binary()?;

        let output = run_client(&binary, &addr, &token, &["config", "get"]).await?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            output.status.success(),
            "{} {}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );

        let port_id = assert_cli_port_mutations(&binary, &addr, &token).await?;
        assert_cli_proxy_mutations(&binary, &addr, &token, &port_id).await?;
        assert_cli_access_lists(&binary, &addr, &token).await?;
        assert_cli_dns_credentials(&binary, &addr, &token).await?;
        // POST /api/config/notifications/test answers notification_webhook_missing
        // while no webhook is saved; the command must reach that endpoint.
        let out = run_client(&binary, &addr, &token, &["config", "test-notification"]).await?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{stderr}");
        assert!(stderr.contains("notification_webhook_missing"), "{stderr}");
        Ok(())
    })
    .await
}

/// AGENTS.md: every Admin API operation is a CLI command. `r3 config update` saves the settings
/// that `r3 config get` then shows, and a read-scope token cannot change them (403).
/// `r3 platform get-workers` and `r3 db list-engines` answer with JSON, and the engine catalog
/// holds the PostgreSQL engine that `r3 db create --engine postgres` names.
#[tokio::test]
async fn the_client_updates_settings_and_lists_workers_and_engines() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli-settings")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    let file = dir.0.join("settings.json");
    // `r3-server start` installs the embedded catalog before it serves; this harness starts the
    // server without `main`, so it takes the same step.
    r3v3rs3::engines::install_embedded();
    with_admin_config(&dir.0, config, |addr| async move {
        let binary = client_binary()?;
        let token = access_token(&addr).await?;
        let read_token = access_token_with_scope(&addr, "read").await?;
        assert_cli_config_update(&binary, &addr, &token, &read_token, &file).await?;
        cli_json(&binary, &addr, &read_token, &["platform", "get-workers"]).await?;
        let engines = cli_json(&binary, &addr, &read_token, &["db", "list-engines"]).await?;
        assert!(engines.to_string().contains("postgres"), "{engines}");
        Ok(())
    })
    .await
}

/// `r3 config update` refuses a read-scope token (403) and saves what `r3 config get` shows.
async fn assert_cli_config_update(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    read_token: &str,
    file: &Path,
) -> anyhow::Result<()> {
    let mut settings = cli_json(binary, addr, token, &["config", "get"]).await?;
    settings["shutdown_graceful_timeout"] = json!("17s");
    std::fs::write(file, serde_json::to_vec(&settings)?)?;
    let path = file.to_string_lossy().to_string();

    let out = run_client(binary, addr, read_token, &["config", "update", &path]).await?;
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success() && stderr.contains("403"), "{stderr}");

    cli_json_or_text(binary, addr, token, &["config", "update", &path]).await?;
    let saved = cli_json(binary, addr, token, &["config", "get"]).await?;
    assert_eq!(saved["shutdown_graceful_timeout"], json!("17s"));
    Ok(())
}

/// Runs a client command that must succeed and returns its standard output.
async fn cli_json_or_text(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    args: &[&str],
) -> anyhow::Result<String> {
    let out = run_client(binary, addr, token, args).await?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    anyhow::ensure!(
        out.status.success(),
        "{args:?}: {stdout} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(stdout)
}

/// Runs a client command that must succeed and print JSON.
async fn cli_json(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    args: &[&str],
) -> anyhow::Result<serde_json::Value> {
    let stdout = cli_json_or_text(binary, addr, token, args).await?;
    Ok(serde_json::from_str(&stdout)?)
}

async fn obtain_tokens_in_flow(addr: SocketAddr) -> anyhow::Result<(String, String)> {
    let client = Client::new();
    let cookie = session(&client, &addr).await?;
    let (verifier, challenge) = pkce();
    let code = authorize_and_get_code(addr, &cookie, &challenge).await?;
    trade_code_for_tokens(&client, addr, &code, &verifier).await
}

/// Changes the password of the admin through the HTTP API.
async fn change_admin_password(addr: SocketAddr, cookie: &str) -> anyhow::Result<()> {
    let res = Client::new()
        .put(format!("http://{addr}/api/accounts/admin"))
        .header("Cookie", cookie)
        .json(&json!({"role": "admin", "password": "new-password"}))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::OK);
    Ok(())
}

/// The JSON-RPC messages of one stateless `tools/call` (MCP `2026-07-28`), in the order of the
/// SSE events of the answer. `meta` adds keys to the `_meta` of the request.
async fn stateless_call_events(
    addr: SocketAddr,
    access: &str,
    meta: Value,
) -> anyhow::Result<Vec<Value>> {
    let mut request_meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
    });
    if let (Some(base), Some(extra)) = (request_meta.as_object_mut(), meta.as_object()) {
        base.extend(extra.clone());
    }
    let response = Client::new()
        .post(format!("http://{addr}/api/mcp"))
        .header("Authorization", format!("Bearer {access}"))
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "server_status")
        .json(&json!({
            "jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": { "name": "server_status", "arguments": {}, "_meta": request_meta },
        }))
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await?;
    let mut events = body
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .filter(|data| !data.trim().is_empty())
        .map(|data| serde_json::from_str(data.trim()))
        .collect::<Result<Vec<Value>, _>>()?;
    // A reply that is a single message comes as plain JSON instead of an event stream.
    if events.is_empty() {
        events.push(serde_json::from_str(&body).with_context(|| format!("{status}: {body}"))?);
    }
    Ok(events)
}

/// A tool call answers with a notification before its result, so the response headers leave at
/// once: a reverse proxy in front of the panel refuses an upstream that sends no headers within
/// its request timeout (60 s by default), and a PITR restore or a deployment runs longer. The
/// client sends a progress token, as Claude Code does with every call, so the first event is a
/// progress notification for that token (MCP `notifications/progress`).
#[tokio::test]
async fn a_tool_call_announces_itself_before_its_result() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-announce")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token(&addr).await?;
        let events = stateless_call_events(addr, &token, json!({ "progressToken": "p-2" })).await?;
        let last = events.last().context("no event")?;
        assert_eq!(last["id"], json!(7), "{events:?}");
        assert!(last.get("result").is_some(), "{events:?}");
        assert_eq!(
            events[0]["method"],
            json!("notifications/progress"),
            "{events:?}"
        );
        assert_eq!(
            events[0]["params"]["progressToken"],
            json!("p-2"),
            "{events:?}"
        );
        Ok(())
    })
    .await
}

/// The HTTP status of a `tools/list` call with the access token.
async fn tools_list_status(addr: SocketAddr, access: &str) -> anyhow::Result<StatusCode> {
    Ok(Client::new()
        .post(format!("http://{addr}/api/mcp"))
        .header("Authorization", format!("Bearer {access}"))
        .header("Accept", "application/json, text/event-stream")
        .json(&json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }))
        .send()
        .await?
        .status())
}

/// A refresh token of a consent that a password change ended must not issue new tokens, or the
/// client keeps its access after the change.
#[tokio::test]
async fn a_password_change_ends_the_refresh_token() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-refresh-revoke")?;
    with_admin(&dir.0, |addr| async move {
        let (_, refresh) = obtain_tokens_in_flow(addr).await?;
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        change_admin_password(addr, &cookie).await?;
        let res = client
            .post(format!("http://{addr}/oauth/token"))
            .form(&[("grant_type", "refresh_token"), ("refresh_token", &refresh)])
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let body: Value = res.json().await?;
        assert_eq!(body["error"], json!("invalid_grant"));
        Ok(())
    })
    .await
}

/// An authorization code that the account approved before a password change gives no access when
/// the client trades it after the change.
#[tokio::test]
async fn a_password_change_ends_an_unused_authorization_code() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-code-revoke")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (verifier, challenge) = pkce();
        let code = authorize_and_get_code(addr, &cookie, &challenge).await?;
        change_admin_password(addr, &cookie).await?;
        // The trade happens in a later second than the change, so a token time taken at the
        // trade would pass the check.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let (access, _) = trade_code_for_tokens(&client, addr, &code, &verifier).await?;
        assert_eq!(
            tools_list_status(addr, &access).await?,
            StatusCode::UNAUTHORIZED
        );
        Ok(())
    })
    .await
}

/// A consent right after a change of the account, within the same second, gives a working token.
#[tokio::test]
async fn a_token_right_after_an_account_change_works() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-token-after-change")?;
    with_admin(&dir.0, |addr| async move {
        create_account_as_admin(addr, "fresh", "editor").await?;
        let access = access_token_for(&addr, "fresh", "password").await?;
        // A bare tools/list without a session gets 400 after the token check; only 401 means
        // that the token was refused.
        assert_ne!(
            tools_list_status(addr, &access).await?,
            StatusCode::UNAUTHORIZED
        );
        Ok(())
    })
    .await
}

async fn check_post_restart_flow(
    client: &Client,
    addr: SocketAddr,
    access: &str,
    refresh: &str,
) -> anyhow::Result<()> {
    let res = client
        .post(format!("http://{addr}/api/mcp"))
        .header("Authorization", format!("Bearer {access}"))
        .json(&json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }))
        .send()
        .await?;
    assert_ne!(res.status(), StatusCode::UNAUTHORIZED);

    let cookie = session(client, &addr).await?;
    let rotated: Value = client
        .post(format!("http://{addr}/oauth/token"))
        .form(&[("grant_type", "refresh_token"), ("refresh_token", refresh)])
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(rotated["token_type"], json!("Bearer"));
    let new_access = rotated["access_token"].as_str().unwrap().to_string();

    let sessions: Vec<r3v3rs3_api::app::McpSession> = client
        .get(format!("http://{addr}/api/mcp/sessions"))
        .header("Cookie", &cookie)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(sessions.len(), 1);
    let session_id = sessions[0].id.clone();

    let del = client
        .delete(format!("http://{addr}/api/mcp/sessions/{session_id}"))
        .header("Cookie", &cookie)
        .send()
        .await?;
    assert_eq!(del.status(), StatusCode::OK);

    let res = client
        .post(format!("http://{addr}/api/mcp"))
        .header("Authorization", format!("Bearer {new_access}"))
        .json(&json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }))
        .send()
        .await?;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    Ok(())
}

#[tokio::test]
async fn the_mcp_sessions_survive_server_restart_and_can_be_revoked() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-persist")?;
    let tokens_cell = std::sync::Arc::new(tokio::sync::Mutex::new((String::new(), String::new())));

    let tokens_clone = tokens_cell.clone();
    with_admin(&dir.0, |addr| async move {
        let (access, refresh) = obtain_tokens_in_flow(addr).await?;
        *tokens_clone.lock().await = (access, refresh);
        Ok(())
    })
    .await?;

    let (access, refresh) = tokens_cell.lock().await.clone();
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        check_post_restart_flow(&client, addr, &access, &refresh).await
    })
    .await
}

#[tokio::test]
async fn changing_password_revokes_mcp_access_token() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-pwd-revoke")?;
    with_admin(&dir.0, |addr| async move {
        let (access, _) = obtain_tokens_in_flow(addr).await?;
        let client = Client::new();

        let res = client
            .post(format!("http://{addr}/api/mcp"))
            .header("Authorization", format!("Bearer {access}"))
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }))
            .send()
            .await?;
        assert_ne!(res.status(), StatusCode::UNAUTHORIZED);

        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        let cookie = session(&client, &addr).await?;
        let update = json!({"role": "admin", "password": "new-password"});
        let put_res = client
            .put(format!("http://{addr}/api/accounts/admin"))
            .header("Cookie", &cookie)
            .json(&update)
            .send()
            .await?;
        assert_eq!(put_res.status(), StatusCode::OK);

        let res = client
            .post(format!("http://{addr}/api/mcp"))
            .header("Authorization", format!("Bearer {access}"))
            .header("Accept", "application/json, text/event-stream")
            .json(&json!({ "jsonrpc": "2.0", "method": "tools/list", "id": 1 }))
            .send()
            .await?;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn consent_rejects_cross_origin_request() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-csrf-origin")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (_, challenge) = pkce();
        let url = format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
             &redirect_uri={REDIRECT_URI}&state=s1&code_challenge={challenge}\
             &code_challenge_method=S256&scope=read"
        );
        let page = client
            .get(&url)
            .header("cookie", &cookie)
            .send()
            .await?
            .text()
            .await?;
        let csrf_token = extract_csrf_token(&page)?;

        let consent = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .header("origin", "http://evil.com")
            .form(&[
                ("client_id", CLIENT_ID),
                ("redirect_uri", REDIRECT_URI),
                ("state", "s1"),
                ("code_challenge", challenge.as_str()),
                ("scope", "read"),
                ("decision", "approve"),
                ("csrf_token", &csrf_token),
            ])
            .send()
            .await?;
        assert_eq!(consent.status(), StatusCode::UNAUTHORIZED);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn consent_rejects_missing_or_invalid_csrf_token() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-csrf-invalid")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (_, challenge) = pkce();

        let consent_missing = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .header("origin", format!("http://{addr}"))
            .form(&[
                ("client_id", CLIENT_ID),
                ("redirect_uri", REDIRECT_URI),
                ("state", "s1"),
                ("code_challenge", challenge.as_str()),
                ("scope", "read"),
                ("decision", "approve"),
            ])
            .send()
            .await?;
        assert_eq!(consent_missing.status(), StatusCode::UNAUTHORIZED);

        let consent_bad = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .header("origin", format!("http://{addr}"))
            .form(&[
                ("client_id", CLIENT_ID),
                ("redirect_uri", REDIRECT_URI),
                ("state", "s1"),
                ("code_challenge", challenge.as_str()),
                ("scope", "read"),
                ("decision", "approve"),
                ("csrf_token", "invalid_forged_token"),
            ])
            .send()
            .await?;
        assert_eq!(consent_bad.status(), StatusCode::UNAUTHORIZED);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn consent_rejects_replayed_csrf_token() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-csrf-replay")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (_, challenge) = pkce();
        let url = format!(
            "http://{addr}/oauth/authorize?response_type=code&client_id={CLIENT_ID}\
             &redirect_uri={REDIRECT_URI}&state=s1&code_challenge={challenge}\
             &code_challenge_method=S256&scope=read"
        );
        let page = client
            .get(&url)
            .header("cookie", &cookie)
            .send()
            .await?
            .text()
            .await?;
        let csrf_token = extract_csrf_token(&page)?;

        let form_data = [
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("state", "s1"),
            ("code_challenge", challenge.as_str()),
            ("scope", "read"),
            ("decision", "approve"),
            ("csrf_token", &csrf_token),
        ];

        let consent1 = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .header("origin", format!("http://{addr}"))
            .form(&form_data)
            .send()
            .await?;
        assert_eq!(consent1.status(), StatusCode::SEE_OTHER);

        let consent2 = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .header("origin", format!("http://{addr}"))
            .form(&form_data)
            .send()
            .await?;
        assert_eq!(consent2.status(), StatusCode::UNAUTHORIZED);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn dynamic_registration_rate_limit_throttles_rapid_requests() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-dcr-ratelimit")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let payload = json!({
            "client_name": "Burst Client",
            "redirect_uris": ["http://127.0.0.1:49153/callback"],
        });

        let mut got_429 = false;
        for _ in 0..15 {
            let res = client
                .post(format!("http://{addr}/oauth/register"))
                .json(&payload)
                .send()
                .await?;
            if res.status() == StatusCode::TOO_MANY_REQUESTS {
                got_429 = true;
                break;
            }
        }
        assert!(
            got_429,
            "expected at least one 429 Too Many Requests response during burst"
        );
        Ok(())
    })
    .await
}

#[tokio::test]
async fn token_rate_limit_throttles_rapid_requests() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-token-ratelimit")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let mut got_429 = false;
        for i in 0..25 {
            let res = client
                .post(format!("http://{addr}/oauth/token"))
                .form(&[
                    ("grant_type", "authorization_code"),
                    ("code", &format!("bogus_{i}")),
                    ("redirect_uri", REDIRECT_URI),
                    ("client_id", CLIENT_ID),
                    ("code_verifier", "verifier"),
                ])
                .send()
                .await?;
            if res.status() == StatusCode::TOO_MANY_REQUESTS {
                got_429 = true;
                break;
            }
        }
        assert!(
            got_429,
            "expected at least one 429 Too Many Requests response during burst"
        );
        Ok(())
    })
    .await
}

/// An unknown role or token scope is refused, because a fallback to a default choice turned a
/// typo into an admin account.
#[tokio::test]
async fn unknown_choices_are_refused() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-unknown-choices")?;
    with_admin(&dir.0, |addr| async move {
        let client = mcp_client(addr, &access_token(&addr).await?).await?;
        let calls = [
            (
                "create_account",
                json!({ "username": "typo", "password": "password", "role": "viewr" }),
            ),
            (
                "update_account",
                json!({ "username": "editor", "role": "edtor" }),
            ),
            ("create_token", json!({ "name": "ci", "scope": "write" })),
        ];
        for (tool, args) in calls {
            let err = mcp_call(&client, tool, args)
                .await
                .expect_err("an unknown choice was accepted");
            assert!(err.to_string().contains("unknown"), "{tool}: {err}");
        }
        let accounts = mcp_call(&client, "list_accounts", json!({})).await?;
        assert!(!accounts.contains("\"typo\""), "{accounts}");
        assert!(accounts.contains("\"editor\""), "{accounts}");
        client.cancel().await?;
        Ok(())
    })
    .await
}

/// An invalid proxy id in the proxy list of an account is refused. Dropping it gave the account a
/// shorter list than the caller asked for, or no limit at all when every id was invalid.
#[tokio::test]
async fn invalid_proxy_ids_of_an_account_are_refused() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-invalid-proxy-ids")?;
    with_admin(&dir.0, |addr| async move {
        let client = mcp_client(addr, &access_token(&addr).await?).await?;
        let calls = [
            (
                "create_account",
                json!({ "username": "scoped", "password": "password", "role": "viewer",
                        "proxies": ["not a proxy id"] }),
            ),
            (
                "update_account",
                json!({ "username": "editor", "role": "editor", "proxies": ["not a proxy id"] }),
            ),
        ];
        for (tool, args) in calls {
            mcp_call(&client, tool, args)
                .await
                .expect_err("an invalid proxy id was dropped");
        }
        let accounts = mcp_call(&client, "list_accounts", json!({})).await?;
        assert!(!accounts.contains("\"scoped\""), "{accounts}");
        client.cancel().await?;
        Ok(())
    })
    .await
}

/// MCP tools that have no API operation of their own, with the reason.
const MCP_ONLY_TOOLS: &[(&str, &str)] = &[
    ("server_status", "the version and the workers in one call"),
    (
        "get_system_metrics",
        "JSON of the process metrics that get_metrics serves as text",
    ),
    ("update_firewall", "the firewall field of update_config"),
    (
        "install_store_app",
        "create_app and deploy_app of a catalog entry",
    ),
    (
        "list_project_environments",
        "the environments of get_project",
    ),
];

/// API operations without an MCP tool. Out of scope by decision: browser sign-in, passkeys,
/// the OAuth server itself, provider callbacks, health probes and the event stream. CLI only:
/// binary files, live streams and the terminal (MCP returns small text and the last records).
/// `purge_proxy_cache_post` is the POST twin of `purge_proxy_cache`.
const OPERATIONS_WITHOUT_TOOL: &[&str] = &[
    "approve_mcp_client",
    "authorize_mcp_client",
    "complete_git_oauth",
    "delete_passkey",
    "download_database_backup",
    "download_disaster_recovery_backup",
    "download_s3_database_backup",
    "download_volume_backup",
    "export_volume_archive",
    "get_oauth_authorization_server_metadata",
    "get_oauth_protected_resource_metadata",
    "get_passkey_challenge",
    "get_session",
    "healthz",
    "import_volume_archive",
    "issue_oauth_token",
    "login",
    "logout",
    "open_app_terminal",
    "purge_proxy_cache_post",
    "readyz",
    "receive_app_webhook",
    "register_oauth_client",
    "register_passkey",
    "restore_disaster_recovery_upload",
    "stream_app_logs",
    "stream_traffic",
    "subscribe_events",
];

/// Project naming rule: an MCP tool carries the operation id of the API operation it runs, so
/// an agent, a CLI user and the OpenAPI document name one action the same way.
#[tokio::test]
async fn every_tool_is_named_after_its_api_operation() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-tool-names")?;
    with_admin(&dir.0, |addr| async move {
        let client = mcp_client(addr, &access_token(&addr).await?).await?;
        let tools = client.list_all_tools().await?;
        client.cancel().await?;
        let tools = tools
            .iter()
            .map(|tool| tool.name.to_string())
            .collect::<BTreeSet<_>>();
        let operations = operation_ids(addr).await?;
        let mcp_only = MCP_ONLY_TOOLS
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        let named_apart = tools
            .difference(&operations)
            .cloned()
            .collect::<BTreeSet<_>>();
        assert_eq!(
            named_apart, mcp_only,
            "tools named apart from an operation id"
        );
        let without_tool = operations
            .difference(&tools)
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected = OPERATIONS_WITHOUT_TOOL
            .iter()
            .map(|id| id.to_string())
            .collect();
        assert_eq!(without_tool, expected, "operations without a tool");
        assert_cli_operations_exist(&tools, &operations);
        Ok(())
    })
    .await
}

/// Every `r3` command names an operation that the server serves, or an MCP tool that combines
/// operations, so the CLI, MCP and the OpenAPI document share one name for an action.
fn assert_cli_operations_exist(tools: &BTreeSet<String>, operations: &BTreeSet<String>) {
    let mcp_only: BTreeSet<&str> = cli_operations::MCP_ONLY_OPERATIONS
        .iter()
        .copied()
        .collect();
    for name in &mcp_only {
        assert!(tools.contains(*name), "{name} is no MCP tool");
        assert!(!operations.contains(*name), "{name} is an API operation");
    }
    let unknown: Vec<_> = cli_operations::COMMAND_OPERATIONS
        .iter()
        .filter(|(_, id)| !operations.contains(*id) && !mcp_only.contains(id))
        .collect();
    assert!(
        unknown.is_empty(),
        "commands with an unknown operation: {unknown:?}"
    );
}

/// `update_config` saves the settings that `get_config` returned with a change. A masked secret
/// in that round trip keeps the saved secret (the API never returns secrets, so an agent can
/// only send the mask back), and an editor cannot replace the settings.
#[tokio::test]
async fn the_settings_round_trip_through_mcp_keeps_secrets() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-update-config")?;
    with_admin(&dir.0, |addr| async move {
        let client = mcp_client(addr, &access_token(&addr).await?).await?;
        save_a_secret_then_round_trip(&client).await?;
        let saved = rest_config(addr).await?;
        assert_eq!(saved["upgrade"]["check_interval"], "2h", "{saved}");
        assert_eq!(
            saved["geoip"]["license_key_set"], true,
            "the round trip erased the secret"
        );
        let workers = mcp_call(&client, "get_workers", json!({})).await?;
        assert!(serde_json::from_str::<Value>(&workers).is_ok(), "{workers}");
        client.cancel().await?;
        assert_editor_cannot_update_config(addr, saved).await
    })
    .await
}

/// An editor account is refused when it sends the settings back.
async fn assert_editor_cannot_update_config(addr: SocketAddr, config: Value) -> anyhow::Result<()> {
    let editor = mcp_client(addr, &access_token_for(&addr, "editor", "password").await?).await?;
    let refused = mcp_call(&editor, "update_config", json!({ "request": config })).await;
    assert!(refused.is_err(), "an editor replaced the settings");
    editor.cancel().await?;
    Ok(())
}

/// Saves a GeoIP license key, then sends the masked settings back with a changed interval.
async fn save_a_secret_then_round_trip(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let mut config: Value =
        serde_json::from_str(&mcp_call(client, "get_config", json!({})).await?)?;
    config["geoip"]["license_key"] = json!("license-secret");
    mcp_call(client, "update_config", json!({ "request": config })).await?;
    let mut config: Value =
        serde_json::from_str(&mcp_call(client, "get_config", json!({})).await?)?;
    assert!(
        !config.to_string().contains("license-secret"),
        "get_config shows the secret"
    );
    // A masked secret still tells the caller that one is stored, like `GET /api/config`.
    assert_eq!(config["geoip"]["license_key_set"], json!(true), "{config}");
    config["upgrade"]["check_interval"] = json!("2h");
    mcp_call(client, "update_config", json!({ "request": config })).await?;
    Ok(())
}

/// `get_access_list` and `get_target` (OpenAPI `get_access_list`, `get_target`) return the one
/// resource that the id names, and an unknown id is not found instead of an empty answer.
#[tokio::test]
async fn single_access_lists_and_targets_are_readable() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-get-one")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    config.platform.agent_port = Some(alloc_tcp_port().await?.socket_addr().port());
    with_admin_config(&dir.0, config, |addr| async move {
        let client = mcp_client(addr, &access_token(&addr).await?).await?;
        assert_one_access_list(&client).await?;
        assert_one_target(&client).await?;
        client.cancel().await?;
        Ok(())
    })
    .await
}

/// `get_access_list` returns the list it names and refuses an unknown id.
async fn assert_one_access_list(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let list = json!({ "name": "one-acl", "ip_filter": { "allow": ["10.0.0.0/8"] } });
    mcp_call(client, "create_access_list", json!({ "list": list })).await?;
    let lists: Value =
        serde_json::from_str(&mcp_call(client, "list_access_lists", json!({})).await?)?;
    let id = lists[0]["id"].as_str().context("no access list id")?;
    let shown = mcp_call(client, "get_access_list", json!({ "id": id })).await?;
    assert!(shown.contains("10.0.0.0/8"), "{shown}");
    assert_not_found(
        "get_access_list",
        &tool_error_text(client, "get_access_list").await,
    );
    Ok(())
}

/// `get_target` returns the target it names and refuses an unknown id.
async fn assert_one_target(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let target = json!({ "request": { "name": "one-target" } });
    let created: Value = serde_json::from_str(&mcp_call(client, "create_target", target).await?)?;
    let id = created["target"]["id"].as_str().context("no target id")?;
    let shown = mcp_call(client, "get_target", json!({ "id": id })).await?;
    assert!(shown.contains("one-target"), "{shown}");
    assert_not_found("get_target", &tool_error_text(client, "get_target").await);
    Ok(())
}

/// The error text of a read tool called with an id that names nothing.
async fn tool_error_text(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    name: &'static str,
) -> String {
    match mcp_call(client, name, json!({ "id": MISSING_DB })).await {
        Ok(found) => format!("unexpected answer: {found}"),
        Err(err) => format!("{err:#}"),
    }
}

/// The settings as `GET /api/config` returns them to the admin.
async fn rest_config(addr: SocketAddr) -> anyhow::Result<Value> {
    let cookie = session_cookie(addr, "admin", "password").await?;
    Ok(Client::new()
        .get(format!("http://{addr}/api/config"))
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

/// The operation ids of the OpenAPI document the server serves.
async fn operation_ids(addr: SocketAddr) -> anyhow::Result<BTreeSet<String>> {
    let cookie = session_cookie(addr, "admin", "password").await?;
    let document: Value = Client::new()
        .get(format!("http://{addr}/api/openapi.json"))
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let paths = document["paths"].as_object().context("no paths")?;
    Ok(paths
        .values()
        .filter_map(Value::as_object)
        .flat_map(|item| item.values())
        .filter_map(|operation| operation["operationId"].as_str())
        .map(str::to_string)
        .collect())
}

/// An argument that the server needs is required in the tool schema, so an agent sends it instead
/// of learning about it from an error.
#[tokio::test]
async fn needed_arguments_are_required_in_the_schema() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-required-arguments")?;
    with_admin(&dir.0, |addr| async move {
        let client = mcp_client(addr, &access_token(&addr).await?).await?;
        let tools = client.list_all_tools().await?;
        let expected = [
            ("create_disaster_recovery_backup", "passphrase"),
            ("restore_disaster_recovery_backup", "passphrase"),
            ("create_git_connection", "client_secret"),
            ("create_registry", "password"),
        ];
        for (name, field) in expected {
            let tool = tools
                .iter()
                .find(|tool| tool.name == name)
                .with_context(|| format!("the tool {name} is missing"))?;
            let required = tool
                .input_schema
                .get("required")
                .cloned()
                .unwrap_or_default();
            assert!(
                required
                    .as_array()
                    .is_some_and(|list| list.contains(&json!(field))),
                "{name} does not require {field}: {required}"
            );
        }
        client.cancel().await?;
        Ok(())
    })
    .await
}

/// A prune policy with a threshold of 0 or an unreadable interval is refused and not stored: a
/// threshold of 0 ran an emergency prune every 15 minutes.
#[tokio::test]
async fn an_invalid_prune_policy_is_refused() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-prune-policy")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let client = mcp_client(addr, &access_token(&addr).await?).await?;
        for args in [
            json!({ "disk_pressure_threshold": 0 }),
            json!({ "interval": "0h" }),
            json!({ "interval": "weekly" }),
        ] {
            mcp_call(&client, "update_prune_policy", args.clone())
                .await
                .expect_err(&format!("an invalid prune policy was stored: {args}"));
        }
        let policy: Value =
            serde_json::from_str(&mcp_call(&client, "get_prune_policy", json!({})).await?)?;
        assert_eq!(policy["interval"], json!("24h"), "{policy}");
        assert_eq!(policy["disk_pressure_threshold"], json!(85), "{policy}");
        client.cancel().await?;
        Ok(())
    })
    .await
}

/// The app store refresh changes the catalog of every account, so a viewer with a token of the
/// edit scope must not run it, the same as through the HTTP API.
#[tokio::test]
async fn refresh_store_rejects_a_viewer() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-store-refresh-auth")?;
    with_admin(&dir.0, |addr| async move {
        create_account_as_admin(addr, "viewer", "viewer").await?;
        let token = access_token_for(&addr, "viewer", "password").await?;
        let client = mcp_client(addr, &token).await?;
        let err = mcp_call(&client, "refresh_store", json!({}))
            .await
            .expect_err("a viewer refreshed the app store");
        assert!(
            err.to_string()
                .contains("the role of the account does not allow this action"),
            "{err}"
        );
        client.cancel().await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn run_server_upgrade_rejects_non_admin() -> anyhow::Result<()> {
    use rmcp::model::CallToolRequestParams;
    use rmcp::service::serve_client;
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };

    let dir = TempDir::new("mcp-upgrade-auth")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token_for(&addr, "editor", "password").await?;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/api/mcp"))
                .auth_header(&token),
        );
        let client = serve_client(TestClient, transport).await?;
        let err = client
            .call_tool(CallToolRequestParams::new("upgrade_server"))
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("the role of the account does not allow this action"),
            "{err}"
        );
        client.cancel().await?;
        Ok(())
    })
    .await
}

/// An MCP client that sends the access token.
async fn mcp_client(
    addr: SocketAddr,
    token: &str,
) -> anyhow::Result<rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>> {
    use rmcp::service::serve_client;
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };
    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/api/mcp"))
            .auth_header(token),
    );
    Ok(serve_client(TestClient, transport).await?)
}

/// Creates an account through the MCP tools of the admin.
async fn create_account_as_admin(
    addr: SocketAddr,
    username: &str,
    role: &str,
) -> anyhow::Result<()> {
    let admin = mcp_client(addr, &access_token(&addr).await?).await?;
    let answer = mcp_call(
        &admin,
        "create_account",
        json!({ "username": username, "password": "password", "role": role }),
    )
    .await?;
    // A tool answer tells the agent what happened; an empty object names nothing.
    let answer: Value = serde_json::from_str(&answer)?;
    assert_eq!(answer["created"], username, "{answer}");
    admin.cancel().await?;
    Ok(())
}

/// The actions of the audit log, newest first.
async fn audit_actions(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<Vec<String>> {
    let log: Value = serde_json::from_str(&mcp_call(client, "list_audit_log", json!({})).await?)?;
    Ok(log
        .as_array()
        .context("the audit log is not a list")?
        .iter()
        .filter_map(|entry| entry["action"].as_str().map(String::from))
        .collect())
}

/// Creates and deletes a project and an environment group through MCP.
async fn change_platform_through_mcp(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
) -> anyhow::Result<()> {
    let project: Value = serde_json::from_str(
        &mcp_call(
            client,
            "create_project",
            json!({ "spec": { "name": "shop" } }),
        )
        .await?,
    )?;
    let id = project["id"].as_str().context("the project id")?;
    mcp_call(client, "delete_project", json!({ "id": id })).await?;
    let group: Value = serde_json::from_str(
        &mcp_call(
            client,
            "create_env_group",
            json!({ "group": { "name": "shared" } }),
        )
        .await?,
    )?;
    let id = group["id"].as_str().context("the group id")?;
    mcp_call(client, "delete_env_group", json!({ "id": id })).await?;
    Ok(())
}

/// A change of the platform through MCP leaves the same audit entries as through the HTTP API.
#[tokio::test]
async fn mcp_platform_changes_are_audited() -> anyhow::Result<()> {
    use rmcp::service::serve_client;
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };

    let dir = TempDir::new("mcp-platform-audit")?;
    let mut config = mcp_config();
    config.platform.enabled = true;
    with_admin_config(&dir.0, config, |addr| async move {
        let token = access_token(&addr).await?;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/api/mcp"))
                .auth_header(&token),
        );
        let client = serve_client(TestClient, transport).await?;
        change_platform_through_mcp(&client).await?;
        // The audit log writes its entries in the background, so the test waits for them.
        let expected = [
            "create_project",
            "delete_project",
            "create_env_group",
            "delete_env_group",
        ];
        let client_ref = &client;
        common::wait_until(
            move || async move {
                let actions = audit_actions(client_ref).await?;
                Ok(expected.iter().all(|e| actions.iter().any(|a| a == e)))
            },
            "the audit log misses a platform change made through MCP",
        )
        .await?;
        client.cancel().await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn mcp_mutation_tools_enforce_edit_scope() -> anyhow::Result<()> {
    use rmcp::service::serve_client;
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };

    let dir = TempDir::new("mcp-scope-enforce")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token_with_scope(&addr, "read").await?;
        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/api/mcp"))
                .auth_header(&token),
        );
        let client = serve_client(TestClient, transport).await?;
        let err = mcp_call(
            &client,
            "create_port",
            json!({
                "port": {
                    "name": "p-fail",
                    "listen": "/ip4/127.0.0.1/tcp/0"
                }
            }),
        )
        .await
        .unwrap_err();
        let err_str = err.to_string();
        assert!(err_str.contains("edit scope"), "{err_str}");
        client.cancel().await?;
        Ok(())
    })
    .await
}
