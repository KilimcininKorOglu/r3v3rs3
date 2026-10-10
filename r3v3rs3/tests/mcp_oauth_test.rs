//! The OAuth 2.1 authorization server of the admin API: the metadata documents, the consent
//! flow with PKCE and the token endpoint.

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

const CLIENT_ID: &str = "r3v3rs3-mcp";
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
async fn the_authorization_code_flow_issues_tokens() -> anyhow::Result<()> {
    let dir = TempDir::new("mcp-flow")?;
    with_admin(&dir.0, |addr| async move {
        let client = Client::new();
        let cookie = session(&client, &addr).await?;
        let (verifier, challenge) = pkce();
        let state = "st4te";
        let consent = no_redirect()
            .post(format!("http://{addr}/oauth/authorize"))
            .header("cookie", &cookie)
            .form(&[
                ("client_id", CLIENT_ID),
                ("redirect_uri", REDIRECT_URI),
                ("state", state),
                ("code_challenge", challenge.as_str()),
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

        // The code trades for tokens with the right verifier.
        let tokens: Value = client
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
        assert_eq!(tokens["token_type"], json!("Bearer"));
        assert_eq!(tokens["scope"], json!("read edit"));
        let access = tokens["access_token"].as_str().unwrap().to_string();
        let refresh = tokens["refresh_token"].as_str().unwrap().to_string();

        // The same code trades only once.
        let replay: Value = client
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
        assert_eq!(replay["error"], json!("invalid_grant"));

        // A wrong verifier does not trade either.
        let (_, challenge) = pkce();
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
        let _ = challenge;

        // The refresh token rotates.
        let rotated: Value = client
            .post(format!("http://{addr}/oauth/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
            ])
            .send()
            .await?
            .json()
            .await?;
        assert_eq!(rotated["scope"], json!("read edit"));
        assert_ne!(rotated["refresh_token"], json!(refresh));
        assert_ne!(rotated["access_token"], json!(access));

        // The old refresh token is invalid after the rotation.
        let replayed = client
            .post(format!("http://{addr}/oauth/token"))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
            ])
            .send()
            .await?;
        assert_eq!(replayed.status(), StatusCode::BAD_REQUEST);
        Ok(())
    })
    .await
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
    use rmcp::model::CallToolRequestParams;
    use rmcp::service::serve_client;
    use rmcp::transport::streamable_http_client::{
        StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
    };

    let dir = TempDir::new("mcp-server")?;
    with_admin(&dir.0, |addr| async move {
        let token = access_token(&addr).await?;

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
        eprintln!("raw initialize: status={status} body={body}");
        assert!(status.is_success(), "{status} {body}");

        let transport = StreamableHttpClientTransport::from_config(
            StreamableHttpClientTransportConfig::with_uri(format!("http://{addr}/api/mcp"))
                // The transport adds the `Bearer ` prefix itself.
                .auth_header(&token),
        );
        let client = serve_client(TestClient, transport).await?;

        let tools = client.list_tools(Default::default()).await?;
        let names: Vec<&str> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert!(names.contains(&"list_proxies"), "{names:?}");
        assert!(names.contains(&"server_status"), "{names:?}");

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

        client.cancel().await?;
        Ok(())
    })
    .await
}
