//! The OAuth 2.1 authorization code flow with PKCE, and the storage of the tokens.

use anyhow::{Context, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use url::Url;

/// The client id that `config.toml` registers for this program.
pub const CLIENT_ID: &str = "r3v3rs3-mcp";

/// The scopes that the client asks for. The consent form can grant fewer.
pub const SCOPES: &str = "read edit";

const TOKEN_POLL_TIMEOUT: Duration = Duration::from_secs(120);

/// The tokens of one server, as they live in the token file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredTokens {
    pub server: String,
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds when the access token expires.
    pub expires_at: u64,
}

/// The path of the token file of one server.
pub fn token_path(server: &str) -> anyhow::Result<PathBuf> {
    let url = Url::parse(server)?;
    let host = url
        .host_str()
        .context("the server URL has no host")?
        .replace(':', "_");
    let port = url
        .port()
        .map_or_else(String::new, |port| format!("_{port}"));
    Ok(dirs::config_dir()
        .context("the config directory is unknown on this platform")?
        .join("r3v3rs3-mcp")
        .join(format!("{host}{port}.json")))
}

/// The tokens of a server, or none when the client never signed in.
pub fn load_tokens(server: &str) -> anyhow::Result<Option<StoredTokens>> {
    let path = token_path(server)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Ok(Some(serde_json::from_str(&text)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Saves the tokens of a server with mode `0600`.
pub fn save_tokens(tokens: &StoredTokens) -> anyhow::Result<()> {
    let path = token_path(&tokens.server)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec(tokens)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// A PKCE verifier with its S256 challenge.
fn pkce() -> (String, String) {
    let verifier = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>());
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

fn new_token() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 48)
}

/// The endpoint addresses of an authorization server, per RFC 8414.
#[derive(Debug, Deserialize)]
struct AuthorizationServerMetadata {
    authorization_endpoint: String,
    token_endpoint: String,
    #[serde(default)]
    code_challenge_methods_supported: Vec<String>,
}

async fn metadata(
    client: &reqwest::Client,
    server: &str,
) -> anyhow::Result<AuthorizationServerMetadata> {
    let metadata: AuthorizationServerMetadata = client
        .get(format!("{server}/.well-known/oauth-authorization-server"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if !metadata
        .code_challenge_methods_supported
        .iter()
        .any(|method| method == "S256")
    {
        bail!("the server does not announce PKCE with S256");
    }
    Ok(metadata)
}

async fn token_request(
    client: &reqwest::Client,
    token_endpoint: &str,
    form: &[(&str, &str)],
) -> anyhow::Result<serde_json::Value> {
    let response = client.post(token_endpoint).form(form).send().await?;
    let value: serde_json::Value = response.json().await?;
    if let Some(error) = value.get("error").and_then(|error| error.as_str()) {
        let description = value
            .get("error_description")
            .and_then(|text| text.as_str())
            .unwrap_or_default();
        return Err(anyhow!("the token request failed: {error} {description}"));
    }
    Ok(value)
}

fn tokens_from(value: &serde_json::Value, server: &str) -> anyhow::Result<StoredTokens> {
    let access_token = value["access_token"]
        .as_str()
        .context("the token response has no access_token")?
        .to_string();
    let refresh_token = value["refresh_token"]
        .as_str()
        .context("the token response has no refresh_token")?
        .to_string();
    let expires_in = value["expires_in"].as_u64().unwrap_or(3600);
    Ok(StoredTokens {
        server: server.to_string(),
        access_token,
        refresh_token,
        expires_at: unix_now() + expires_in.saturating_sub(60),
    })
}

/// The query values of the callback request.
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// Parses the query of a callback request line.
fn parse_callback(request: &str) -> anyhow::Result<CallbackQuery> {
    let target = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("the callback request has no path")?;
    let query = target.split('?').nth(1).unwrap_or_default();
    let mut values = CallbackQuery {
        code: None,
        state: None,
        error: None,
    };
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "code" => values.code = Some(value.to_string()),
            "state" => values.state = Some(value.to_string()),
            "error" => values.error = Some(value.to_string()),
            _ => {}
        }
    }
    Ok(values)
}

/// A small HTTP server that receives exactly one redirect of the authorization flow.
async fn wait_for_code(port: u16, expected_state: &str) -> anyhow::Result<String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let (mut socket, _) = tokio::time::timeout(TOKEN_POLL_TIMEOUT, listener.accept()).await??;
    let mut buffer = vec![0u8; 8192];
    let read = tokio::time::timeout(TOKEN_POLL_TIMEOUT, socket.read(&mut buffer)).await??;
    let request = String::from_utf8_lossy(&buffer[..read]).to_string();
    let values = parse_callback(&request)?;
    if let Some(error) = values.error {
        bail!("the server refused the authorization: {error}");
    }
    if values.state.as_deref() != Some(expected_state) {
        bail!("the state of the callback does not match the request");
    }
    let code = values.code.context("the callback carries no code")?;
    let body = "<!doctype html><html><head><meta charset=\"utf-8\"><title>r3v3rs3 MCP</title>\
                </head><body><p>The sign-in finished. You can close this page.</p></body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    Ok(code)
}

/// Runs the authorization code flow in the browser and returns the tokens.
pub async fn login(
    client: &reqwest::Client,
    server: &str,
    scope: &str,
) -> anyhow::Result<StoredTokens> {
    let metadata = metadata(client, server).await?;
    let (verifier, challenge) = pkce();
    let state = new_token();

    // The callback server listens before the browser opens, per RFC 8252.
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let authorize_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&state={}&code_challenge={}&code_challenge_method=S256&scope={}",
        metadata.authorization_endpoint,
        CLIENT_ID,
        urlencoding_escape(&redirect_uri),
        urlencoding_escape(&state),
        urlencoding_escape(&challenge),
        urlencoding_escape(scope),
    );
    println!("Opening the consent page:");
    println!("{authorize_url}");
    if webbrowser::open(&authorize_url).is_err() {
        println!("Open the address above in a browser and approve the client.");
    }

    // The server answers the browser while this task waits for the code.
    let code = wait_for_code(port, &state).await?;

    let value = token_request(
        client,
        &metadata.token_endpoint,
        &[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("client_id", CLIENT_ID),
            ("code_verifier", verifier.as_str()),
        ],
    )
    .await?;
    let tokens = tokens_from(&value, server)?;
    save_tokens(&tokens)?;
    Ok(tokens)
}

/// Refreshes the tokens of a server with the stored refresh token.
pub async fn refresh(
    client: &reqwest::Client,
    stored: &StoredTokens,
) -> anyhow::Result<StoredTokens> {
    let metadata = metadata(client, &stored.server).await?;
    let value = token_request(
        client,
        &metadata.token_endpoint,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", stored.refresh_token.as_str()),
        ],
    )
    .await?;
    let mut tokens = tokens_from(&value, &stored.server)?;
    // A server that keeps the refresh token answers without a new one.
    if tokens.refresh_token.is_empty() {
        tokens.refresh_token = stored.refresh_token.clone();
    }
    save_tokens(&tokens)?;
    Ok(tokens)
}

/// The tokens of a server that are valid long enough, refreshed when needed.
pub async fn valid_tokens(client: &reqwest::Client, server: &str) -> anyhow::Result<StoredTokens> {
    let Some(stored) = load_tokens(server)? else {
        bail!(
            "no sign-in for {server}. Run `r3v3rs3-mcp login {server}` first. The client id is {CLIENT_ID} and must be in the [mcp] section of config.toml."
        );
    };
    // One minute of margin, so the token outlives the following request.
    if stored.expires_at > unix_now() + 60 {
        return Ok(stored);
    }
    refresh(client, &stored).await
}

/// The percent encoding that a query value needs.
fn urlencoding_escape(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                escaped.push(byte as char)
            }
            _ => escaped.push_str(&format!("%{byte:02X}")),
        }
    }
    escaped
}
