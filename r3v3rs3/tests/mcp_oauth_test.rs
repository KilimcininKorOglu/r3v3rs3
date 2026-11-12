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

mod common;
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

/// Runs a server and its admin API with the MCP clients of the test.
async fn with_admin<F, O>(dir: &Path, func: F) -> anyhow::Result<()>
where
    F: FnOnce(SocketAddr) -> O,
    O: Future<Output = anyhow::Result<()>>,
{
    let config = AppConfig {
        mcp: McpConfig {
            enabled: true,
            clients: vec![McpClientConfig {
                id: CLIENT_ID.to_string(),
                name: "r3v3rs3 MCP client".to_string(),
                redirect_uris: vec![REDIRECT_URI.to_string()],
            }],
        },
        ..Default::default()
    };
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
        assert_code_replay_and_bad_verifier(&client, addr, &code).await?;
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

async fn assert_code_replay_and_bad_verifier(
    client: &Client,
    addr: SocketAddr,
    code: &str,
) -> anyhow::Result<()> {
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

    let bad = client
        .post(format!("http://{addr}/oauth/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", "unused"),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", CLIENT_ID),
            (
                "code_verifier",
                "wrong-verifier-wrong-verifier-wrong-verifier",
            ),
        ])
        .send()
        .await?;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    Ok(())
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
    assert!(names.contains(&"generate_target_token"), "{names:?}");
    assert!(names.contains(&"upgrade_target"), "{names:?}");
    assert!(names.contains(&"upgrade_all_targets"), "{names:?}");
    assert!(names.contains(&"get_server_upgrade_status"), "{names:?}");
    assert!(names.contains(&"run_server_upgrade"), "{names:?}");
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

    Ok(port_id)
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
    Ok(port_id)
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

/// The command line client connects with an explicit `--token` and runs commands.
#[tokio::test]
async fn the_client_binary_runs_commands() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token(&addr).await?;

        // The client binary of the workspace, built up to date before the test runs it.
        let binary = match std::env::var("CARGO_TARGET_DIR") {
            Ok(dir) => std::path::PathBuf::from(dir).join("debug/r3"),
            Err(_) => std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug/r3"),
        };
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "r3v3rs3-cli"])
            .status()
            .context("failed to run cargo")?;
        assert!(status.success());

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
                .contains("the role of the account does not allow this action")
                || err.to_string().contains("-32603"),
            "{err}"
        );
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
        assert!(
            err_str.contains("edit scope") || err_str.contains("-32602"),
            "{err_str}"
        );
        client.cancel().await?;
        Ok(())
    })
    .await
}
