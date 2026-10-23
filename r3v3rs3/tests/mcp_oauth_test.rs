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
    match session_cookie(*addr, "admin", "password").await {
        Ok(cookie) => Ok(cookie),
        Err(err) if err.to_string().contains("429") => {
            let _ = client;
            login_when_allowed(*addr, "admin", "password").await
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
        let text = response.text().await?;
        assert!(text.contains("r3v3rs3 MCP client"), "{text}");
        assert!(text.contains("admin"), "{text}");
        assert!(text.contains("Permissions Requested"), "{text}");
        assert!(text.contains("value=\"approve\""), "{text}");
        assert!(text.contains("value=\"deny\""), "{text}");
        Ok(())
    })
    .await
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
    let consent = no_redirect()
        .post(format!("http://{addr}/oauth/authorize"))
        .header("cookie", cookie)
        .form(&[
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("state", "st4te"),
            ("code_challenge", challenge),
            ("scope", "read edit"),
            ("decision", "approve"),
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
        let consent = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .form(&[
                ("client_id", CLIENT_ID),
                ("redirect_uri", REDIRECT_URI),
                ("state", "s9"),
                ("code_challenge", challenge.as_str()),
                ("scope", "read"),
                ("decision", "deny"),
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

#[tokio::test]
async fn a_dynamically_registered_client_runs_the_flow() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-dcr")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        // The registration needs no session and returns a fresh client id.
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

        // The registered client runs the same consent flow as a configured one.
        let cookie = session(&client, &addr).await?;
        let (verifier, challenge) = pkce();
        let redirect_uri = "http://127.0.0.1:49153/callback";
        let consent = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .form(&[
                ("client_id", client_id.as_str()),
                ("redirect_uri", redirect_uri),
                ("state", "dcr"),
                ("code_challenge", challenge.as_str()),
                ("scope", "read"),
                ("decision", "approve"),
            ])
            .send()
            .await?;
        assert_eq!(
            consent.status(),
            StatusCode::SEE_OTHER,
            "{}",
            consent.text().await.unwrap_or_default()
        );
        let location = consent.headers().get("location").unwrap().to_str()?;
        assert!(location.starts_with(redirect_uri), "{location}");
        let code = location
            .split("code=")
            .nth(1)
            .and_then(|rest| rest.split('&').next())
            .unwrap()
            .to_string();

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
    let client = Client::new();
    let cookie = session(&client, addr).await?;
    let (verifier, challenge) = pkce();
    let consent = no_redirect()
        .post(format!("http://{addr}/oauth/authorize"))
        .header("cookie", &cookie)
        .form(&[
            ("client_id", CLIENT_ID),
            ("redirect_uri", REDIRECT_URI),
            ("state", "st"),
            ("code_challenge", challenge.as_str()),
            ("scope", "read edit"),
            ("decision", "approve"),
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
    use rmcp::model::CallToolRequestParams;
    use rmcp::service::serve_client;
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/api/mcp"))
            .auth_header(token),
    );
    let client = serve_client(TestClient, transport).await?;

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

    let result = client
        .call_tool(CallToolRequestParams::new("server_status"))
        .await?;
    let text = result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default();
    assert!(text.contains("workers"), "{text}");

    let metrics_result = client
        .call_tool(CallToolRequestParams::new("get_system_metrics"))
        .await?;
    let metrics_text = metrics_result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default();
    assert!(
        metrics_text.contains("event_loop_lag_seconds"),
        "{metrics_text}"
    );

    let store_result = client
        .call_tool(CallToolRequestParams::new("get_store"))
        .await?;
    let store_text = store_result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default();
    assert!(store_text.contains("catalog"), "{store_text}");

    let ports_result = client
        .call_tool(CallToolRequestParams::new("list_ports"))
        .await?;
    let ports_text = ports_result
        .content
        .first()
        .and_then(|content| content.as_text())
        .map(|text| text.text.clone())
        .unwrap_or_default();
    assert!(ports_text.starts_with('['), "{ports_text}");

    client.cancel().await?;
    Ok(())
}

/// The command line client connects with an explicit `--token` and prints the tools.
#[tokio::test]
async fn the_client_binary_lists_the_tools() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-cli")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token(&addr).await?;

        // The client binary of the workspace, built up to date before the test runs it.
        let binary = match std::env::var("CARGO_TARGET_DIR") {
            Ok(dir) => std::path::PathBuf::from(dir).join("debug/r3v3rs3-cli"),
            Err(_) => {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/debug/r3v3rs3-cli")
            }
        };
        let status = std::process::Command::new("cargo")
            .args(["build", "-p", "r3v3rs3-cli"])
            .status()
            .context("failed to run cargo")?;
        assert!(status.success());
        // The child waits for the server of this test process, so the blocking wait runs on a
        // separate thread and the runtime keeps serving.
        async fn run_client(
            binary: &std::path::Path,
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
            tokio::task::spawn_blocking(move || {
                command.output().context("failed to run the client binary")
            })
            .await
            .map_err(|err| anyhow::anyhow!("the client task failed: {err}"))?
        }
        let output = run_client(&binary, &addr, &token, &["tools"]).await?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            output.status.success(),
            "{} {}",
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(stdout.contains("server_status"), "{stdout}");
        assert!(stdout.contains("list_proxies"), "{stdout}");

        let output = run_client(&binary, &addr, &token, &["call", "server_status"]).await?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        assert!(
            output.status.success() && stdout.contains("workers"),
            "{stdout} {}",
            String::from_utf8_lossy(&output.stderr)
        );
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
