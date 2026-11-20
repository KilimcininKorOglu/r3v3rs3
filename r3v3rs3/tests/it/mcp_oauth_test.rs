//! The OAuth 2.1 authorization server of the admin API: the metadata documents, the consent
//! flow with PKCE and the token endpoint.

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
    assert!(names.contains(&"list_dr_backups"), "{names:?}");
    assert!(names.contains(&"get_system_metrics"), "{names:?}");
    assert!(names.contains(&"test_database_backup"), "{names:?}");
    assert!(names.contains(&"get_database_pitr_range"), "{names:?}");
    assert!(names.contains(&"restore_database_pitr"), "{names:?}");
    assert!(names.contains(&"archive_database_wal_segment"), "{names:?}");
    assert!(names.contains(&"list_database_wal_segments"), "{names:?}");
    for name in [
        "get_database_credentials",
        "list_database_tenants",
        "reset_database_password",
        "delete_database_tenant",
    ] {
        assert!(names.contains(&name), "{names:?}");
    }
    assert!(names.contains(&"generate_target_token"), "{names:?}");
    assert!(names.contains(&"upgrade_target"), "{names:?}");
    assert!(names.contains(&"upgrade_all_targets"), "{names:?}");
    assert!(names.contains(&"get_server_upgrade_status"), "{names:?}");
    assert!(names.contains(&"run_server_upgrade"), "{names:?}");
    assert!(names.contains(&"set_port_transparent"), "{names:?}");
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
        "add_port",
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

/// `set_port_transparent` changes only the transparent option of the port, and the change is
/// saved: `get_port` reads it back each time.
async fn assert_mcp_transparent_toggle(
    client: &rmcp::service::RunningService<rmcp::service::RoleClient, TestClient>,
    port_id: &str,
) -> anyhow::Result<()> {
    for enabled in [true, false] {
        let args = json!({ "id": port_id, "enabled": enabled });
        mcp_call(client, "set_port_transparent", args).await?;
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
        "add_proxy",
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
    let add_out = run_client(binary, addr, token, &["port", "add", &payload]).await?;
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
    let args = ["port", "set-early-data", &port_id, "--enable", "false"];
    let out = run_client(binary, addr, token, &args).await?;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(port_id)
}

/// `port set-transparent` saves the option, and `--enable false` clears it again.
async fn assert_cli_transparent(
    binary: &Path,
    addr: &SocketAddr,
    token: &str,
    port_id: &str,
) -> anyhow::Result<()> {
    for enabled in ["true", "false"] {
        let args = ["port", "set-transparent", port_id, "--enable", enabled];
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
    let add_out = run_client(binary, addr, token, &["proxy", "add", &payload]).await?;
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

    let rm_proxy = run_client(binary, addr, token, &["proxy", "rm", &proxy_id]).await?;
    assert!(rm_proxy.status.success());

    let rm_port = run_client(binary, addr, token, &["port", "rm", port_id]).await?;
    assert!(rm_port.status.success());
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
    cli_ok(binary, addr, token, &["acl", "add", &added.to_string()]).await?;
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
    cli_ok(binary, addr, token, &["dns", "add", &added.to_string()]).await?;
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
fn client_binary() -> anyhow::Result<std::path::PathBuf> {
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

/// `r3 target get` and `r3 target token` reach the target API (OpenAPI: `list_targets` and
/// `new_target_token` at `/api/targets/{id}/token`), and `r3 platform status` shows the release
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
            &["target", "add", "--name", "cli-edge"],
        )
        .await?;
        let id = added
            .split("id=")
            .nth(1)
            .and_then(|rest| rest.split(',').next())
            .context("no target id in the add output")?
            .to_string();
        let shown = cli_ok(&binary, &addr, &token, &["target", "get", &id]).await?;
        assert!(shown.contains("cli-edge"), "{shown}");
        let renewed = cli_ok(&binary, &addr, &token, &["target", "token", &id]).await?;
        assert!(renewed.contains("\"token\""), "{renewed}");
        let status = cli_ok(&binary, &addr, &token, &["platform", "status"]).await?;
        assert!(status.contains("\"version\""), "{status}");
        assert!(status.contains("\"mode\""), "{status}");
        Ok(())
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
        Ok(())
    })
    .await
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
            (
                "create_auth_token",
                json!({ "name": "ci", "scope": "write" }),
            ),
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

/// An argument that the server needs is required in the tool schema, so an agent sends it instead
/// of learning about it from an error.
#[tokio::test]
async fn needed_arguments_are_required_in_the_schema() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-required-arguments")?;
    with_admin(&dir.0, |addr| async move {
        let client = mcp_client(addr, &access_token(&addr).await?).await?;
        let tools = client.list_all_tools().await?;
        let expected = [
            ("create_dr_backup", "passphrase"),
            ("restore_dr_backup", "passphrase"),
            ("add_git_connection", "client_secret"),
            ("add_registry", "password"),
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
            .call_tool(CallToolRequestParams::new("run_server_upgrade"))
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
            "add_project",
            "delete_project",
            "add_env_group",
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
            "add_port",
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
