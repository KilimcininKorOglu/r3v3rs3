//! The OAuth 2.1 authorization server that protects the MCP endpoint of the admin API.
//!
//! A pre-registered client starts the authorization code flow with PKCE. The account signs in
//! through the WebUI session, approves the client on a plain form, and the client trades the
//! code for an opaque access token with a refresh token at the token endpoint. The tokens and
//! dynamically registered clients are persisted in SQLite (`platform.db`), so restarts and updates
//! preserve authorized sessions. A client that is not in the configured list registers itself
//! through RFC 7591.

use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::audit::AuditRecord;
use crate::server::rpc::auth::GetPlatform;
use axum::{
    Json,
    extract::{Form, Query, RawQuery, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use axum_extra::extract::CookieJar;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use r3v3rs3_api::app::{McpClientConfig, McpConfig, McpSession};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Row as _, SqlitePool};
use std::collections::HashMap;
use std::time::Duration;
use tracing::info;

/// The lifetime of an authorization code. RFC 6749 recommends 10 minutes.
const CODE_LIFETIME: Duration = Duration::from_secs(600);
/// The lifetime of an access token. The client refreshes it afterwards.
const ACCESS_TOKEN_LIFETIME: Duration = Duration::from_secs(3600);
/// The lifetime of a refresh token.
const REFRESH_TOKEN_LIFETIME: Duration = Duration::from_secs(30 * 24 * 3600);
/// The scopes of an access token. `read` sees the state, `edit` also changes it.
pub const SCOPES: [&str; 2] = ["read", "edit"];

const TOKEN_LENGTH: usize = 48;
const MAX_GRANTS: usize = 1024;

const SCHEMA_CLIENTS: &str = "CREATE TABLE IF NOT EXISTS mcp_clients (
    id            TEXT PRIMARY KEY,
    name          TEXT NOT NULL,
    redirect_uris TEXT NOT NULL,
    created_at    INTEGER NOT NULL
)";

const SCHEMA_TOKENS: &str = "CREATE TABLE IF NOT EXISTS mcp_tokens (
    id                 TEXT PRIMARY KEY,
    access_token       TEXT NOT NULL UNIQUE,
    refresh_token      TEXT NOT NULL UNIQUE,
    client_id          TEXT NOT NULL,
    username           TEXT NOT NULL,
    scope              TEXT NOT NULL,
    expires_at         INTEGER NOT NULL,
    refresh_expires_at INTEGER NOT NULL,
    created_at         INTEGER NOT NULL
)";

/// The authorization code that a consent approved, before the client trades it.
#[derive(Debug, Clone)]
struct AuthorizationCode {
    expires_at: u64,
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    scope: String,
    username: String,
}

/// One signed-in MCP client. The access token is the key of the map.
#[derive(Debug, Clone)]
pub struct AccessToken {
    pub id: String,
    expires_at: u64,
    refresh_expires_at: u64,
    client_id: String,
    username: String,
    pub scope: String,
    refresh_token: String,
    created_at: u64,
}

impl AccessToken {
    /// The username of the account that approved the grant.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The timestamp in seconds at which the grant was created.
    pub fn created_at(&self) -> u64 {
        self.created_at
    }
}

async fn db_save_client(pool: &SqlitePool, client: &McpClientConfig) -> anyhow::Result<()> {
    let uris = serde_json::to_string(&client.redirect_uris)?;
    let now = unix_now() as i64;
    sqlx::query(
        "INSERT INTO mcp_clients (id, name, redirect_uris, created_at) \
         VALUES (?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, redirect_uris = excluded.redirect_uris",
    )
    .bind(&client.id)
    .bind(&client.name)
    .bind(uris)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

async fn db_save_token(
    pool: &SqlitePool,
    grant: &AccessToken,
    access_token: &str,
) -> anyhow::Result<()> {
    let now = unix_now() as i64;
    sqlx::query(
        "INSERT INTO mcp_tokens (id, access_token, refresh_token, client_id, username, scope, expires_at, refresh_expires_at, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&grant.id)
    .bind(access_token)
    .bind(&grant.refresh_token)
    .bind(&grant.client_id)
    .bind(&grant.username)
    .bind(&grant.scope)
    .bind(grant.expires_at as i64)
    .bind(grant.refresh_expires_at as i64)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

async fn db_delete_token(pool: &SqlitePool, access_or_refresh: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM mcp_tokens WHERE access_token = ? OR refresh_token = ?")
        .bind(access_or_refresh)
        .bind(access_or_refresh)
        .execute(pool)
        .await?;
    Ok(())
}

async fn db_delete_session(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM mcp_tokens WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

async fn db_purge_expired(pool: &SqlitePool, now: i64) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM mcp_tokens WHERE refresh_expires_at <= ?")
        .bind(now)
        .execute(pool)
        .await?;
    Ok(())
}

async fn db_load_clients(pool: &SqlitePool) -> anyhow::Result<HashMap<String, McpClientConfig>> {
    let rows = sqlx::query("SELECT id, name, redirect_uris FROM mcp_clients")
        .fetch_all(pool)
        .await?;
    let mut clients = HashMap::new();
    for row in rows {
        let id: String = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let uris_raw: String = row.try_get("redirect_uris")?;
        let redirect_uris = serde_json::from_str(&uris_raw).unwrap_or_default();
        clients.insert(
            id.clone(),
            McpClientConfig {
                id,
                name,
                redirect_uris,
            },
        );
    }
    Ok(clients)
}

type LoadedTokens = (
    HashMap<String, AccessToken>,
    HashMap<String, String>,
    HashMap<String, String>,
);

fn hash_token(token: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(token.as_bytes())))
}

fn ensure_token_hash(token: &str) -> String {
    if token.starts_with("sha256:") {
        token.to_string()
    } else {
        hash_token(token)
    }
}

fn row_to_token(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<(String, String, AccessToken), sqlx::Error> {
    let id: String = row.try_get("id")?;
    let access_token: String = row.try_get("access_token")?;
    let refresh_token: String = row.try_get("refresh_token")?;
    let expires_at: i64 = row.try_get("expires_at")?;
    let refresh_expires_at: i64 = row.try_get("refresh_expires_at")?;
    let created_at: i64 = row.try_get("created_at")?;
    let access_hash = ensure_token_hash(&access_token);
    let refresh_hash = ensure_token_hash(&refresh_token);
    let grant = AccessToken {
        id,
        expires_at: expires_at as u64,
        refresh_expires_at: refresh_expires_at as u64,
        client_id: row.try_get("client_id")?,
        username: row.try_get("username")?,
        scope: row.try_get("scope")?,
        refresh_token: refresh_hash.clone(),
        created_at: created_at as u64,
    };
    Ok((access_hash, refresh_hash, grant))
}

async fn db_load_tokens(pool: &SqlitePool, now: i64) -> anyhow::Result<LoadedTokens> {
    let rows = sqlx::query(
        "SELECT id, access_token, refresh_token, client_id, username, scope, expires_at, refresh_expires_at, created_at \
         FROM mcp_tokens WHERE refresh_expires_at > ?",
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    let mut tokens = HashMap::new();
    let mut refresh = HashMap::new();
    let mut by_id = HashMap::new();
    for row in rows {
        if let Ok((access, ref_tok, grant)) = row_to_token(&row) {
            by_id.insert(grant.id.clone(), access.clone());
            refresh.insert(ref_tok, access.clone());
            tokens.insert(access, grant);
        }
    }
    Ok((tokens, refresh, by_id))
}

const CONSENT_CSRF_LIFETIME: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
struct ConsentCsrf {
    username: String,
    session_token_hash: String,
    expires_at: u64,
}

/// The codes and tokens of the signed-in MCP clients, in memory and backed by SQLite.
#[derive(Default)]
pub struct McpAuthorizations {
    pool: Option<SqlitePool>,
    codes: HashMap<String, AuthorizationCode>,
    tokens: HashMap<String, AccessToken>,
    refresh: HashMap<String, String>,
    by_id: HashMap<String, String>,
    /// The clients that registered themselves through RFC 7591.
    clients: HashMap<String, McpClientConfig>,
    csrf_tokens: HashMap<String, ConsentCsrf>,
}

/// The limit of the dynamically registered clients, like `MAX_GRANTS` for the tokens.
const MAX_CLIENTS: usize = 256;

impl McpAuthorizations {
    /// Whether authorizations are backed by a persistent database.
    pub fn is_persistent(&self) -> bool {
        self.pool.is_some()
    }

    /// Opens the SQLite database at `path`, ensures tables exist, loads saved clients and tokens,
    /// and purges expired records.
    pub async fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool = SqlitePool::connect_with(options).await?;
        sqlx::query(SCHEMA_CLIENTS).execute(&pool).await?;
        sqlx::query(SCHEMA_TOKENS).execute(&pool).await?;
        let now = unix_now() as i64;
        let _ = db_purge_expired(&pool, now).await;
        let clients = db_load_clients(&pool).await.unwrap_or_default();
        let (tokens, refresh, by_id) = db_load_tokens(&pool, now).await.unwrap_or_default();
        Ok(Self {
            pool: Some(pool),
            codes: HashMap::new(),
            tokens,
            refresh,
            by_id,
            clients,
            csrf_tokens: HashMap::new(),
        })
    }

    /// Lists active authorized MCP client sessions for the admin WebUI.
    pub fn list_sessions(&self, config_clients: &[McpClientConfig]) -> Vec<McpSession> {
        let now = unix_now();
        let mut sessions = Vec::new();
        for grant in self.tokens.values() {
            if grant.refresh_expires_at > now {
                let client_name = McpClientConfig::find(config_clients, &grant.client_id)
                    .map(|c| c.name.clone())
                    .or_else(|| self.clients.get(&grant.client_id).map(|c| c.name.clone()))
                    .unwrap_or_else(|| grant.client_id.clone());
                sessions.push(McpSession {
                    id: grant.id.clone(),
                    client_id: grant.client_id.clone(),
                    client_name,
                    username: grant.username.clone(),
                    scope: grant.scope.clone(),
                    created_at: grant.created_at,
                    expires_at: grant.expires_at,
                    refresh_expires_at: grant.refresh_expires_at,
                });
            }
        }
        sessions.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        sessions
    }

    /// Revokes an active authorized MCP client session by id.
    pub async fn revoke_session(&mut self, session_id: &str) -> bool {
        let Some(access_token) = self.by_id.remove(session_id) else {
            return false;
        };
        if let Some(grant) = self.tokens.remove(&access_token) {
            self.refresh.remove(&grant.refresh_token);
        }
        if let Some(pool) = &self.pool {
            let _ = db_delete_session(pool, session_id).await;
        }
        true
    }

    /// Registers a client of the dynamic registration and returns its `client_id`.
    pub async fn register_client(&mut self, mut client: McpClientConfig) -> Option<String> {
        if self.clients.len() >= MAX_CLIENTS {
            return None;
        }
        let id = format!("mcp-{}", new_token().to_lowercase());
        client.id = id.clone();
        if let Some(pool) = &self.pool
            && let Err(err) = db_save_client(pool, &client).await
        {
            tracing::warn!(err = %err, "failed to persist registered MCP client");
        }
        self.clients.insert(id.clone(), client);
        Some(id)
    }

    /// The dynamically registered client with this id, if one exists.
    pub fn dynamic_client(&self, id: &str) -> Option<McpClientConfig> {
        self.clients.get(id).cloned()
    }

    /// Stores an approved authorization code.
    fn issue_code(&mut self, grant: AuthorizationCode) -> String {
        self.cleanup();
        let code = new_token();
        self.codes.insert(code.clone(), grant);
        code
    }

    /// Trades an authorization code for tokens after the PKCE check.
    async fn exchange_code(
        &mut self,
        code: &str,
        client_id: &str,
        redirect_uri: &str,
        code_verifier: &str,
    ) -> Result<(String, IssuedTokens), StatusCode> {
        let grant = self.codes.remove(code).filter(|grant| {
            grant.expires_at > unix_now()
                && grant.client_id == client_id
                && grant.redirect_uri == redirect_uri
        });
        let Some(grant) = grant else {
            return Err(StatusCode::BAD_REQUEST);
        };
        if challenge_of(code_verifier) != grant.code_challenge {
            return Err(StatusCode::BAD_REQUEST);
        }
        let username = grant.username.clone();
        let issued = self
            .issue_tokens(&grant.client_id, &grant.scope, &grant.username)
            .await;
        Ok((username, issued))
    }

    /// Issues a fresh pair of tokens for the account and persists them.
    async fn issue_tokens(&mut self, client_id: &str, scope: &str, username: &str) -> IssuedTokens {
        let access_token = new_token();
        let refresh_token = new_token();
        let access_hash = hash_token(&access_token);
        let refresh_hash = hash_token(&refresh_token);
        let now = unix_now();
        let session_id = format!(
            "mcp-{}",
            &hex::encode(Sha256::digest(access_token.as_bytes()))[..16]
        );
        let grant = AccessToken {
            id: session_id.clone(),
            expires_at: now + ACCESS_TOKEN_LIFETIME.as_secs(),
            refresh_expires_at: now + REFRESH_TOKEN_LIFETIME.as_secs(),
            client_id: client_id.to_string(),
            username: username.to_string(),
            scope: scope.to_string(),
            refresh_token: refresh_hash.clone(),
            created_at: now,
        };
        if let Some(pool) = &self.pool
            && let Err(err) = db_save_token(pool, &grant, &access_hash).await
        {
            tracing::warn!(err = %err, "failed to persist issued MCP token");
        }
        self.tokens.insert(access_hash.clone(), grant);
        self.refresh
            .insert(refresh_hash, access_hash.clone());
        self.by_id.insert(session_id, access_hash);
        IssuedTokens {
            access_token,
            token_type: "Bearer",
            refresh_token,
            expires_in: ACCESS_TOKEN_LIFETIME.as_secs(),
            scope: scope.to_string(),
        }
    }

    /// Rotates the tokens of a refresh token: both tokens get fresh values.
    async fn refresh_tokens(&mut self, refresh_token: &str) -> Option<IssuedTokens> {
        let ref_hash = hash_token(refresh_token);
        let access = self.refresh.remove(&ref_hash)?;
        let grant = self.tokens.remove(&access)?;
        self.by_id.remove(&grant.id);
        if let Some(pool) = &self.pool
            && let Err(err) = db_delete_token(pool, &ref_hash).await
        {
            tracing::warn!(err = %err, "failed to delete rotated MCP refresh token");
        }
        if grant.refresh_expires_at <= unix_now() {
            self.refresh.remove(&grant.refresh_token);
            return None;
        }
        Some(
            self.issue_tokens(&grant.client_id, &grant.scope, &grant.username)
                .await,
        )
    }

    /// Validates an access token and returns the client that it belongs to.
    pub fn verify(&self, token: &str) -> Option<&AccessToken> {
        let hash = hash_token(token);
        self.tokens
            .get(&hash)
            .filter(|grant| grant.expires_at > unix_now())
    }

    /// Issues an anti-CSRF token for the OAuth consent page, bound to the user session.
    pub fn issue_consent_csrf(&mut self, username: &str, session_token: &str) -> String {
        self.cleanup();
        let token = new_token();
        self.csrf_tokens.insert(
            token.clone(),
            ConsentCsrf {
                username: username.to_string(),
                session_token_hash: hash_token(session_token),
                expires_at: unix_now() + CONSENT_CSRF_LIFETIME.as_secs(),
            },
        );
        token
    }

    /// Verifies and consumes a single-use anti-CSRF token.
    pub fn verify_and_consume_csrf(
        &mut self,
        csrf_token: &str,
        username: &str,
        session_token: Option<&str>,
    ) -> bool {
        let Some(csrf) = self.csrf_tokens.remove(csrf_token) else {
            return false;
        };
        if csrf.expires_at <= unix_now() || csrf.username != username {
            return false;
        }
        if let Some(session) = session_token
            && !session.is_empty()
            && csrf.session_token_hash != hash_token(session)
        {
            return false;
        }
        true
    }

    /// Drops the expired codes and tokens.
    fn cleanup(&mut self) {
        let now = unix_now();
        self.csrf_tokens.retain(|_, csrf| csrf.expires_at > now);
        if self.codes.len() + self.tokens.len() < MAX_GRANTS {
            return;
        }
        self.codes.retain(|_, grant| grant.expires_at > now);
        let expired: Vec<String> = self
            .tokens
            .iter()
            .filter(|(_, grant)| grant.refresh_expires_at <= now)
            .map(|(token, _)| token.clone())
            .collect();
        for token in expired {
            if let Some(grant) = self.tokens.remove(&token) {
                self.refresh.remove(&grant.refresh_token);
                self.by_id.remove(&grant.id);
            }
        }
        if let Some(pool) = &self.pool {
            let pool = pool.clone();
            tokio::spawn(async move {
                let _ = db_purge_expired(&pool, now as i64).await;
            });
        }
    }
}

/// The body of a successful token response.
#[derive(Debug, Serialize)]
struct IssuedTokens {
    access_token: String,
    token_type: &'static str,
    expires_in: u64,
    refresh_token: String,
    scope: String,
}

/// The query of the authorization request.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AuthorizeQuery {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    /// The opaque value that the response echoes back.
    pub state: String,
    /// The PKCE challenge of the client.
    pub code_challenge: String,
    pub code_challenge_method: Option<String>,
    /// The scopes of the token, separated by spaces. The default is `read`.
    pub scope: Option<String>,
}

/// The submitted consent form.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct ConsentForm {
    pub client_id: String,
    pub redirect_uri: String,
    pub state: String,
    pub code_challenge: String,
    pub scope: String,
    /// The name of the submit button. A refusal also submits the form.
    pub decision: String,
    /// The anti-CSRF token generated on consent page render.
    #[serde(default)]
    pub csrf_token: String,
}

/// The form of the token request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TokenForm {
    pub grant_type: String,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub code_verifier: Option<String>,
    pub refresh_token: Option<String>,
}

fn unix_now() -> u64 {
    crate::clock::unix_ms() / 1000
}

fn new_token() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), TOKEN_LENGTH)
}

/// The S256 challenge of a PKCE verifier.
fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// The public origin that OAuth clients see: the public URL of the server, or the forwarded
/// headers when it is not set. The headers are a weak fallback, because the proxy of the
/// server rewrites the `Host` header of the upstream request to the upstream address.
async fn origin_of(state: &AppState, headers: &HeaderMap) -> String {
    if let Ok(platform) = state.call_system(GetPlatform).await
        && let Ok(platform) = platform.get()
        && let Some(url) = platform.public_url()
    {
        return url.to_string().trim_end_matches('/').to_string();
    }
    base_origin(headers)
}

/// The origin of the admin API, from the forwarded headers or from the host of the request.
fn base_origin(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

/// Validates the authorization request and returns the registered client. A client lives in
/// the `config` list or registered itself dynamically.
fn validate_authorize(
    config: &McpConfig,
    dynamic: Option<&McpClientConfig>,
    query: &AuthorizeQuery,
) -> Result<McpClientConfig, Error> {
    validate_authorize_shape(query)?;
    let client = find_client(config, dynamic, &query.client_id)?;
    if !client.allows_redirect(&query.redirect_uri) {
        return Err(Error::McpRedirectInvalid);
    }
    Ok(client)
}

/// The shape of the authorization request: the response type, the PKCE parameters and the
/// scopes must be the ones that the server supports.
fn validate_authorize_shape(query: &AuthorizeQuery) -> Result<(), Error> {
    if query.response_type != "code"
        || query.code_challenge_method.as_deref() != Some("S256")
        || !(43..=128).contains(&query.code_challenge.len())
        || query.state.is_empty()
    {
        return Err(Error::McpGrantInvalid);
    }
    let scopes = scopes_of(&query.scope);
    if scopes.iter().any(|scope| !SCOPES.contains(&scope.as_str())) {
        return Err(Error::McpGrantInvalid);
    }
    Ok(())
}

/// The client with this id from the configured list or from the dynamic registrations.
fn find_client(
    config: &McpConfig,
    dynamic: Option<&McpClientConfig>,
    id: &str,
) -> Result<McpClientConfig, Error> {
    if !config.enabled {
        return Err(Error::McpDisabled);
    }
    match McpClientConfig::find(&config.clients, id) {
        Some(client) => Ok(client.clone()),
        None => dynamic
            .filter(|client| client.id == id)
            .cloned()
            .ok_or_else(|| Error::McpClientUnknown { id: id.to_string() }),
    }
}

/// The normalized scope list of a request.
fn scopes_of(scope: &Option<String>) -> Vec<String> {
    let scopes: Vec<String> = scope
        .as_deref()
        .unwrap_or("read")
        .split_whitespace()
        .map(str::to_string)
        .collect();
    if scopes.is_empty() {
        vec!["read".to_string()]
    } else {
        scopes
    }
}

/// The metadata of the authorization server, per RFC 8414.
#[utoipa::path(
    get,
    path = "/.well-known/oauth-authorization-server",
    tag = "mcp",
    operation_id = "oauth_authorization_server_metadata",
    security(()),
    responses((status = 200, description = "The authorization server metadata."))
)]
pub async fn authorization_server_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let origin = origin_of(&state, &headers).await;
    Json(json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/oauth/authorize"),
        "token_endpoint": format!("{origin}/oauth/token"),
        "registration_endpoint": format!("{origin}/oauth/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "scopes_supported": SCOPES,
    }))
    .into_response()
}

/// The metadata of the protected MCP resource, per RFC 9728.
#[utoipa::path(
    get,
    path = "/.well-known/oauth-protected-resource",
    tag = "mcp",
    operation_id = "oauth_protected_resource_metadata",
    security(()),
    responses((status = 200, description = "The protected resource metadata."))
)]
pub async fn protected_resource_metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let origin = origin_of(&state, &headers).await;
    Json(json!({
        "resource": format!("{origin}/api/mcp"),
        "authorization_servers": [origin],
        "scopes_supported": SCOPES,
        "bearer_methods_supported": ["GET", "POST"],
    }))
    .into_response()
}

/// The consent form of the authorization request. The account approves a registered client.
#[utoipa::path(
    get,
    path = "/authorize",
    tag = "mcp",
    operation_id = "mcp_authorize",
    params(AuthorizeQuery),
    responses((status = 200, description = "The consent form.", content_type = "text/html"))
)]
pub async fn authorize(
    State(state): State<AppState>,
    jar: CookieJar,
    RawQuery(raw_query): RawQuery,
    Query(query): Query<AuthorizeQuery>,
) -> Result<Response, AppError> {
    let session_cookie = jar.get("token");
    let caller = match session_cookie {
        Some(cookie) => super::auth::session_caller(&state, cookie.value())
            .await
            .ok(),
        None => None,
    };
    let Some(caller) = caller else {
        return Ok(login_redirect(raw_query.as_deref()));
    };
    let session_token = session_cookie.map(|c| c.value()).unwrap_or_default();
    let mut data = state.data.lock().await;
    let config = data.config.mcp.clone();
    let dynamic = data.mcp.dynamic_client(&query.client_id);
    let client = validate_authorize(&config, dynamic.as_ref(), &query)?;
    let csrf_token = data.mcp.issue_consent_csrf(&caller.username, session_token);
    drop(data);
    let scopes = scopes_of(&query.scope);
    let body = render_consent_page(&client, &caller, &query, &scopes, &csrf_token);
    let mut response = Html(body).into_response();
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("frame-ancestors 'none'"),
    );
    response
        .headers_mut()
        .insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    Ok(response)
}

fn login_redirect(raw_query: Option<&str>) -> Response {
    let target = match raw_query {
        Some(q) => format!("/oauth/authorize?{q}"),
        None => "/oauth/authorize".to_string(),
    };
    let redirect = format!(
        "/login?redirect={}",
        utf8_percent_encode(&target, NON_ALPHANUMERIC)
    );
    Redirect::to(&redirect).into_response()
}

fn render_scopes_list(scopes: &[String]) -> String {
    let mut html = String::new();
    for scope in scopes {
        let (desc, label) = match scope.as_str() {
            "read" => (
                "View proxies, ports, certificates, system metrics, and logs.",
                "read (read-only)",
            ),
            "edit" => (
                "Create, update, or remove proxies, ports, certificates, and settings.",
                "edit (full control)",
            ),
            other => (
                "Access specific resources associated with this scope.",
                other,
            ),
        };
        html.push_str(&format!(
            "<div class=\"scope-item\">\
             <svg class=\"scope-icon\" width=\"18\" height=\"18\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z\"/></svg>\
             <div>\
             <span class=\"scope-badge\">{label}</span>\
             <div class=\"scope-desc\">{desc}</div>\
             </div>\
             </div>",
            label = html_escape(label),
            desc = html_escape(desc),
        ));
    }
    html
}

fn render_consent_css() -> &'static str {
    "<style>\
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }\
    body {\
        font-family: -apple-system, BlinkMacSystemFont, \"Segoe UI\", Roboto, \"Helvetica Neue\", Arial, sans-serif;\
        background-color: #f8fafc;\
        color: #0f172a;\
        line-height: 1.5;\
        min-height: 100vh;\
        display: flex;\
        flex-direction: column;\
        align-items: center;\
        justify-content: center;\
        padding: 1.5rem;\
    }\
    .consent-container { max-width: 480px; width: 100%; }\
    .brand-header {\
        display: flex;\
        align-items: center;\
        justify-content: center;\
        gap: 0.75rem;\
        margin-bottom: 1.5rem;\
    }\
    .brand-logo { width: 36px; height: 36px; flex-shrink: 0; }\
    .brand-name {\
        font-size: 1.35rem;\
        font-weight: 700;\
        font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;\
        letter-spacing: -0.025em;\
        color: #0f172a;\
    }\
    .oauth-badge {\
        font-size: 0.7rem;\
        font-weight: 600;\
        padding: 0.15rem 0.5rem;\
        border-radius: 9999px;\
        background: #e2e8f0;\
        color: #475569;\
        text-transform: uppercase;\
        letter-spacing: 0.05em;\
    }\
    .card {\
        background: #ffffff;\
        border: 1px solid #e2e8f0;\
        border-radius: 16px;\
        padding: 2rem;\
        box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.06), 0 8px 10px -6px rgba(0, 0, 0, 0.04);\
    }\
    .card-header { text-align: center; margin-bottom: 1.5rem; }\
    .card-title { font-size: 1.25rem; font-weight: 700; color: #0f172a; margin-bottom: 0.35rem; }\
    .card-subtitle { font-size: 0.875rem; color: #64748b; }\
    .client-info-box {\
        display: flex;\
        align-items: flex-start;\
        gap: 0.85rem;\
        padding: 1rem;\
        background: #f8fafc;\
        border: 1px solid #e2e8f0;\
        border-radius: 10px;\
        margin-bottom: 1rem;\
    }\
    .client-icon {\
        width: 38px;\
        height: 38px;\
        border-radius: 8px;\
        background: #2563eb;\
        color: #ffffff;\
        display: flex;\
        align-items: center;\
        justify-content: center;\
        flex-shrink: 0;\
    }\
    .client-details { min-width: 0; flex: 1; }\
    .client-name { font-size: 1rem; font-weight: 600; color: #0f172a; word-break: break-word; }\
    .client-meta {\
        font-size: 0.775rem;\
        font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;\
        color: #64748b;\
        word-break: break-all;\
        margin-top: 0.2rem;\
    }\
    .user-info-row {\
        display: flex;\
        align-items: center;\
        gap: 0.5rem;\
        font-size: 0.85rem;\
        color: #475569;\
        padding: 0.6rem 0.85rem;\
        background: #f1f5f9;\
        border-radius: 8px;\
        margin-bottom: 1.25rem;\
    }\
    .user-icon { width: 16px; height: 16px; color: #64748b; flex-shrink: 0; }\
    .scopes-section { margin-bottom: 1.25rem; }\
    .scopes-title {\
        font-size: 0.8rem;\
        font-weight: 600;\
        text-transform: uppercase;\
        letter-spacing: 0.05em;\
        color: #64748b;\
        margin-bottom: 0.6rem;\
    }\
    .scope-item {\
        display: flex;\
        align-items: flex-start;\
        gap: 0.65rem;\
        padding: 0.65rem 0.85rem;\
        border: 1px solid #e2e8f0;\
        border-radius: 8px;\
        margin-bottom: 0.5rem;\
        background: #ffffff;\
    }\
    .scope-icon { color: #2563eb; flex-shrink: 0; margin-top: 2px; }\
    .scope-badge {\
        font-size: 0.725rem;\
        font-family: ui-monospace, SFMono-Regular, monospace;\
        font-weight: 600;\
        padding: 0.1rem 0.4rem;\
        border-radius: 4px;\
        background: #dbeafe;\
        color: #1e40af;\
        display: inline-block;\
        margin-bottom: 0.2rem;\
    }\
    .scope-desc { font-size: 0.825rem; color: #475569; line-height: 1.35; }\
    .security-notice {\
        font-size: 0.775rem;\
        color: #64748b;\
        text-align: center;\
        margin-bottom: 1.5rem;\
        line-height: 1.4;\
        padding: 0 0.5rem;\
    }\
    .form-actions { display: grid; grid-template-columns: 1fr 1fr; gap: 0.75rem; }\
    .btn {\
        display: inline-flex;\
        align-items: center;\
        justify-content: center;\
        font-size: 0.925rem;\
        font-weight: 600;\
        padding: 0.7rem 1rem;\
        border-radius: 8px;\
        cursor: pointer;\
        transition: all 0.15s ease-in-out;\
        border: none;\
        text-decoration: none;\
    }\
    .btn-secondary {\
        background: #ffffff;\
        color: #475569;\
        border: 1px solid #cbd5e1;\
    }\
    .btn-secondary:hover {\
        background: #f1f5f9;\
        color: #0f172a;\
        border-color: #94a3b8;\
    }\
    .btn-primary {\
        background: #2563eb;\
        color: #ffffff;\
        box-shadow: 0 1px 2px 0 rgba(0, 0, 0, 0.05);\
    }\
    .btn-primary:hover { background: #1d4ed8; }\
    .btn:focus-visible { outline: 2px solid #2563eb; outline-offset: 2px; }\
    @media (prefers-color-scheme: dark) {\
        body { background-color: #09090b; color: #f4f4f5; }\
        .brand-name { color: #f4f4f5; }\
        .oauth-badge { background: #27272a; color: #a1a1aa; }\
        .card {\
            background: #18181b;\
            border-color: #27272a;\
            box-shadow: 0 10px 25px -5px rgba(0, 0, 0, 0.5), 0 8px 10px -6px rgba(0, 0, 0, 0.5);\
        }\
        .card-title { color: #f4f4f5; }\
        .card-subtitle { color: #a1a1aa; }\
        .client-info-box { background: #27272a; border-color: #3f3f46; }\
        .client-name { color: #f4f4f5; }\
        .client-meta { color: #a1a1aa; }\
        .user-info-row { background: #27272a; color: #d4d4d8; }\
        .user-icon { color: #a1a1aa; }\
        .scopes-title { color: #a1a1aa; }\
        .scope-item { background: #1f1f23; border-color: #27272a; }\
        .scope-badge { background: #1e3a8a; color: #bfdbfe; }\
        .scope-desc { color: #d4d4d8; }\
        .scope-icon { color: #60a5fa; }\
        .security-notice { color: #a1a1aa; }\
        .btn-secondary {\
            background: #27272a;\
            color: #e4e4e7;\
            border-color: #3f3f46;\
        }\
        .btn-secondary:hover { background: #3f3f46; color: #ffffff; }\
        .btn-primary { background: #3b82f6; color: #ffffff; }\
        .btn-primary:hover { background: #2563eb; }\
    }\
    </style>"
}

fn render_consent_page(
    client: &McpClientConfig,
    caller: &Caller,
    query: &AuthorizeQuery,
    scopes: &[String],
    csrf_token: &str,
) -> String {
    let scope_value = scopes.join(" ");
    let scopes_html = render_scopes_list(scopes);
    let css = render_consent_css();
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>r3v3rs3 &middot; Authorize {name}</title>\
         {css}</head>\
         <body><div class=\"consent-container\">\
         <div class=\"brand-header\">\
         <svg class=\"brand-logo\" xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 46 46\" fill=\"none\">\
         <rect width=\"32.527\" height=\"32.527\" x=\"23\" fill=\"#FFE134\" rx=\"6\" transform=\"rotate(45 23 0)\"/>\
         <g stroke=\"#202020\" stroke-linecap=\"round\" stroke-linejoin=\"round\" stroke-width=\"3\">\
         <path d=\"M13 18h18M27 14l4 4-4 4\"/><path d=\"M33 28H15M19 24l-4 4 4 4\"/>\
         </g></svg>\
         <span class=\"brand-name\">r3v3rs3</span>\
         <span class=\"oauth-badge\">OAuth 2.1</span>\
         </div>\
         <div class=\"card\">\
         <div class=\"card-header\">\
         <h1 class=\"card-title\">Authorize {name}</h1>\
         <p class=\"card-subtitle\">An external application requests access to this server.</p>\
         </div>\
         <div class=\"client-info-box\">\
         <div class=\"client-icon\">\
         <svg width=\"22\" height=\"22\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><rect width=\"18\" height=\"18\" x=\"3\" y=\"3\" rx=\"3\"/><path d=\"m8 10 3 2.5-3 2.5\"/><path d=\"M13 15h3\"/></svg>\
         </div>\
         <div class=\"client-details\">\
         <div class=\"client-name\">{name}</div>\
         <div class=\"client-meta\">Client ID: <code>{id}</code></div>\
         <div class=\"client-meta\">Redirect: <code>{redirect_uri}</code></div>\
         </div></div>\
         <div class=\"user-info-row\">\
         <svg class=\"user-icon\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"2\" stroke-linecap=\"round\" stroke-linejoin=\"round\"><path d=\"M19 21v-2a4 4 0 0 0-4-4H9a4 4 0 0 0-4 4v2\"/><circle cx=\"12\" cy=\"7\" r=\"4\"/></svg>\
         <span>Signed in as <strong>{username}</strong></span>\
         </div>\
         <div class=\"scopes-section\">\
         <div class=\"scopes-title\">Permissions Requested</div>\
         {scopes_html}\
         </div>\
         <p class=\"security-notice\">Ensure you trust <strong>{name}</strong> before approving. It will act on behalf of your account.</p>\
         <form method=\"post\" action=\"/oauth/authorize\">\
         <input type=\"hidden\" name=\"csrf_token\" value=\"{csrf_token}\">\
         <input type=\"hidden\" name=\"client_id\" value=\"{client_id}\">\
         <input type=\"hidden\" name=\"redirect_uri\" value=\"{redirect_uri}\">\
         <input type=\"hidden\" name=\"state\" value=\"{state_value}\">\
         <input type=\"hidden\" name=\"code_challenge\" value=\"{challenge}\">\
         <input type=\"hidden\" name=\"scope\" value=\"{scope}\">\
         <div class=\"form-actions\">\
         <button type=\"submit\" name=\"decision\" value=\"deny\" class=\"btn btn-secondary\">Cancel</button>\
         <button type=\"submit\" name=\"decision\" value=\"approve\" class=\"btn btn-primary\">Approve</button>\
         </div>\
         </form></div></div></body></html>",
        name = html_escape(&client.name),
        id = html_escape(&client.id),
        username = html_escape(&caller.username),
        scope = html_escape(&scope_value),
        csrf_token = html_escape(csrf_token),
        client_id = html_escape(&query.client_id),
        redirect_uri = html_escape(&query.redirect_uri),
        state_value = html_escape(&query.state),
        challenge = html_escape(&query.code_challenge),
    )
}

/// The smallest HTML escaping that the consent form needs.
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn get_platform_public_url(state: &AppState) -> Option<String> {
    if let Ok(platform) = state.call_system(GetPlatform).await
        && let Ok(platform) = platform.get()
    {
        platform.public_url().map(|u| u.to_string())
    } else {
        None
    }
}

fn consent_redirect_uri(
    redirect_uri: &str,
    state: &str,
    code: Option<&str>,
    error: Option<&str>,
) -> String {
    let separator = if redirect_uri.contains('?') { '&' } else { '?' };
    let mut query = format!("{}state={}", separator, urlencoding_escape(state));
    if let Some(code) = code {
        query.push_str(&format!("&code={}", urlencoding_escape(code)));
    }
    if let Some(error) = error {
        query.push_str(&format!("&error={}", urlencoding_escape(error)));
    }
    format!("{redirect_uri}{query}")
}

/// Issues the authorization code and sends the browser to the redirect URI.
#[utoipa::path(
    post,
    path = "/authorize",
    tag = "mcp",
    operation_id = "mcp_consent",
    request_body = ConsentForm,
    responses((status = 303, description = "The redirect to the client with the code or an error."))
)]
pub async fn consent(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    axum::Extension(caller): axum::Extension<Caller>,
    Form(form): Form<ConsentForm>,
) -> Result<Response, AppError> {
    let public_url = get_platform_public_url(&state).await;
    super::auth::verify_origin(&headers, public_url.as_deref())?;

    let session_token = jar.get("token").map(|c| c.value());
    let mut data = state.data.lock().await;
    if !data
        .mcp
        .verify_and_consume_csrf(&form.csrf_token, &caller.username, session_token)
    {
        return Err(Error::Unauthorized.into());
    }

    let config = data.config.mcp.clone();
    let dynamic = data.mcp.dynamic_client(&form.client_id);
    let client = validate_authorize(
        &config,
        dynamic.as_ref(),
        &AuthorizeQuery {
            client_id: form.client_id.clone(),
            redirect_uri: form.redirect_uri.clone(),
            response_type: "code".to_string(),
            state: form.state.clone(),
            code_challenge: form.code_challenge.clone(),
            code_challenge_method: Some("S256".to_string()),
            scope: Some(form.scope.clone()),
        },
    )?;
    drop(data);
    if form.decision != "approve" {
        return Ok(Redirect::to(&consent_redirect_uri(
            &form.redirect_uri,
            &form.state,
            None,
            Some("access_denied"),
        ))
        .into_response());
    }
    let code = state.data.lock().await.mcp.issue_code(AuthorizationCode {
        expires_at: unix_now() + CODE_LIFETIME.as_secs(),
        client_id: client.id.clone(),
        redirect_uri: form.redirect_uri.clone(),
        code_challenge: form.code_challenge.clone(),
        scope: form.scope.clone(),
        username: caller.username.clone(),
    });
    state
        .record_audit(
            &caller.username,
            caller.client,
            AuditRecord::new(AuditAction::AuthorizeMcpClient)
                .id(client.id.clone())
                .summary(format!("{} ({})", client.name, form.scope)),
        )
        .await;
    info!(client = %client.id, "an MCP client was approved");
    Ok(Redirect::to(&consent_redirect_uri(
        &form.redirect_uri,
        &form.state,
        Some(&code),
        None,
    ))
    .into_response())
}

/// The percent encoding that a query value of a redirect URI needs.
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

/// The body of an OAuth error response.
fn oauth_error(code: &'static str, description: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": code, "error_description": description })),
    )
        .into_response()
}

/// The RFC 7591 registration request.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct RegistrationBody {
    /// The name that the consent form shows.
    pub client_name: Option<String>,
    /// The redirect URIs of the client. `http` stays allowed on the loopback addresses only,
    /// enforced at the authorization request like for the configured clients.
    pub redirect_uris: Vec<String>,
    pub grant_types: Option<Vec<String>>,
    pub response_types: Option<Vec<String>>,
    pub token_endpoint_auth_method: Option<String>,
}

/// Validates a registration request and returns the client to store. Per RFC 7591 the
/// response errors carry `invalid_redirect_uri` and `invalid_client_metadata`.
fn validate_registration(
    body: &RegistrationBody,
) -> Result<McpClientConfig, (&'static str, String)> {
    let redirect_uris = validate_redirect_uris(&body.redirect_uris)?;
    validate_client_metadata(body)?;
    Ok(McpClientConfig {
        id: String::new(),
        name: body
            .client_name
            .clone()
            .unwrap_or_else(|| "MCP client".into()),
        redirect_uris,
    })
}

/// Every redirect URI of a registration is an absolute http or https URI.
fn validate_redirect_uris(uris: &[String]) -> Result<Vec<String>, (&'static str, String)> {
    if uris.is_empty() {
        return Err((
            "invalid_redirect_uri",
            "the request lists no redirect URI".into(),
        ));
    }
    for uri in uris {
        let parsed = url::Url::parse(uri)
            .ok()
            .filter(|url| matches!(url.scheme(), "https" | "http"));
        if parsed.is_none() {
            return Err((
                "invalid_redirect_uri",
                format!("{uri} is not an absolute http or https URI"),
            ));
        }
    }
    Ok(uris.to_vec())
}

/// The grant types, the response types and the auth method of a registration stay inside
/// what the server supports.
fn validate_client_metadata(body: &RegistrationBody) -> Result<(), (&'static str, String)> {
    let grants = body.grant_types.clone().unwrap_or_else(|| {
        vec![
            "authorization_code".to_string(),
            "refresh_token".to_string(),
        ]
    });
    if grants
        .iter()
        .any(|grant| grant != "authorization_code" && grant != "refresh_token")
    {
        return Err((
            "invalid_client_metadata",
            "only authorization_code and refresh_token are supported".into(),
        ));
    }
    if let Some(types) = &body.response_types
        && types.iter().any(|kind| kind != "code")
    {
        return Err((
            "invalid_client_metadata",
            "only the code response type is supported".into(),
        ));
    }
    if let Some(method) = &body.token_endpoint_auth_method
        && method != "none"
    {
        return Err((
            "invalid_client_metadata",
            "only the none auth method is supported".into(),
        ));
    }
    Ok(())
}

/// Registers an MCP client dynamically, per RFC 7591. The registration lives in memory, so a
/// restart removes it and the client signs in again through a fresh registration.
#[utoipa::path(
    post,
    path = "/register",
    tag = "mcp",
    operation_id = "oauth_register",
    security(()),
    request_body = RegistrationBody,
    responses((status = 201, description = "The registered client."), ErrorResponses)
)]
pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegistrationBody>,
) -> Response {
    let config = state.data.lock().await.config.mcp.clone();
    if !config.enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "access_denied" })),
        )
            .into_response();
    }
    let client = match validate_registration(&body) {
        Ok(client) => client,
        Err((code, description)) => return oauth_error(code, &description),
    };
    let issued_at = unix_now();
    let mut data = state.data.lock().await;
    let Some(id) = data.mcp.register_client(client.clone()).await else {
        return oauth_error("invalid_client_metadata", "the registration list is full");
    };
    drop(data);
    info!(client = %id, "an MCP client registered itself");
    (
        StatusCode::CREATED,
        Json(json!({
            "client_id": id,
            "client_id_issued_at": issued_at,
            "client_name": client.name,
            "redirect_uris": client.redirect_uris,
            "token_endpoint_auth_method": "none",
            "grant_types_supported": ["authorization_code", "refresh_token"],
            "response_types_supported": ["code"],
        })),
    )
        .into_response()
}

/// Trades an authorization code or a refresh token for tokens.
#[utoipa::path(
    post,
    path = "/token",
    tag = "mcp",
    operation_id = "oauth_token",
    security(()),
    request_body = TokenForm,
    responses((status = 200, description = "The access and refresh tokens."), ErrorResponses)
)]
pub async fn token(State(state): State<AppState>, Form(form): Form<TokenForm>) -> Response {
    let config = state.data.lock().await.config.mcp.clone();
    if !config.enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "access_denied" })),
        )
            .into_response();
    }
    match form.grant_type.as_str() {
        "authorization_code" => {
            let (Some(code), Some(redirect_uri), Some(client_id), Some(verifier)) = (
                form.code.as_deref(),
                form.redirect_uri.as_deref(),
                form.client_id.as_deref(),
                form.code_verifier.as_deref(),
            ) else {
                return oauth_error("invalid_request", "the code request misses a parameter");
            };
            let mut data = state.data.lock().await;
            let known = McpClientConfig::find(&config.clients, client_id).is_some()
                || data.mcp.dynamic_client(client_id).is_some();
            if !known {
                return oauth_error("invalid_client", "unknown client");
            }
            match data
                .mcp
                .exchange_code(code, client_id, redirect_uri, verifier)
                .await
            {
                Ok((_username, issued)) => Json(issued).into_response(),
                Err(_) => oauth_error("invalid_grant", "the code is unknown, expired or used"),
            }
        }
        "refresh_token" => {
            let Some(refresh_token) = form.refresh_token.as_deref() else {
                return oauth_error("invalid_request", "the refresh request misses the token");
            };
            let mut data = state.data.lock().await;
            match data.mcp.refresh_tokens(refresh_token).await {
                Some(issued) => Json(issued).into_response(),
                None => oauth_error("invalid_grant", "the refresh token is unknown or expired"),
            }
        }
        _ => oauth_error(
            "unsupported_grant_type",
            "only authorization_code and refresh_token",
        ),
    }
}

/// Lists active authorized MCP client sessions.
#[utoipa::path(
    get,
    path = "/sessions",
    tag = "mcp",
    operation_id = "list_mcp_sessions",
    responses((status = 200, description = "The active MCP sessions.", body = Vec<McpSession>))
)]
pub async fn list_mcp_sessions(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<Caller>,
) -> Result<Json<Vec<r3v3rs3_api::app::McpSession>>, AppError> {
    caller.authorize(crate::accounts::Permission::Admin)?;
    let data = state.data.lock().await;
    let sessions = data.mcp.list_sessions(&data.config.mcp.clients);
    Ok(Json(sessions))
}

/// Revokes an active authorized MCP client session by id.
#[utoipa::path(
    delete,
    path = "/sessions/{id}",
    tag = "mcp",
    operation_id = "revoke_mcp_session",
    params(("id" = String, Path, description = "The session id to revoke")),
    responses(
        (status = 200, description = "Session revoked successfully."),
        (status = 404, description = "Session not found.")
    )
)]
pub async fn revoke_mcp_session(
    State(state): State<AppState>,
    axum::Extension(caller): axum::Extension<Caller>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<StatusCode, AppError> {
    caller.authorize(crate::accounts::Permission::Admin)?;
    let mut data = state.data.lock().await;
    if data.mcp.revoke_session(&id).await {
        Ok(StatusCode::OK)
    } else {
        Err(AppError::NotFound)
    }
}

/// The 401 response that points the client at the resource metadata, per RFC 9728.
fn unauthorized(origin: &str) -> Response {
    let mut response = (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
        .into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_str(&format!(
            "Bearer resource_metadata=\"{origin}/.well-known/oauth-protected-resource\""
        ))
        .unwrap_or(HeaderValue::from_static("Bearer")),
    );
    response
}

/// The middleware of the MCP endpoint: the request needs an access token with a live grant of a
/// signed-in account. The token scopes travel with the caller in the request extensions.
pub async fn verify_bearer(
    State(state): State<AppState>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let config = state.data.lock().await.config.mcp.clone();
    if !config.enabled {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "access_denied" })),
        )
            .into_response();
    }
    let origin = origin_of(&state, &headers).await;
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return unauthorized(&origin);
    };
    let (username, scope, created_at) = {
        let data = state.data.lock().await;
        match data.mcp.verify(token) {
            Some(grant) => (
                grant.username.clone(),
                grant.scope.clone(),
                grant.created_at,
            ),
            None => return unauthorized(&origin),
        }
    };
    let mut record = crate::sessions::SessionRecord::new(&username);
    record.started_at = created_at;
    let Some(mut caller) = state.accounts.borrow().caller(&record) else {
        return unauthorized(&origin);
    };
    caller.client = Some(addr.ip());
    request.extensions_mut().insert(caller);
    request.extensions_mut().insert(super::McpScope(scope));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:46492"));
        headers
    }

    #[test]
    fn the_origin_falls_back_to_the_forwarded_headers() {
        assert_eq!(base_origin(&headers()), "https://127.0.0.1:46492");
    }

    #[test]
    fn a_registration_needs_a_redirect_uri() {
        let body = RegistrationBody {
            client_name: None,
            redirect_uris: Vec::new(),
            grant_types: None,
            response_types: None,
            token_endpoint_auth_method: None,
        };
        assert_eq!(
            validate_registration(&body).unwrap_err().0,
            "invalid_redirect_uri"
        );
    }

    #[test]
    fn a_registration_rejects_an_unknown_grant_type() {
        let body = RegistrationBody {
            client_name: None,
            redirect_uris: vec!["http://127.0.0.1:8765/callback".to_string()],
            grant_types: Some(vec!["client_credentials".to_string()]),
            response_types: None,
            token_endpoint_auth_method: None,
        };
        assert_eq!(
            validate_registration(&body).unwrap_err().0,
            "invalid_client_metadata"
        );
    }

    #[test]
    fn a_registration_accepts_a_public_client() {
        let body = RegistrationBody {
            client_name: Some("Claude Code".to_string()),
            redirect_uris: vec!["http://127.0.0.1:8765/callback".to_string()],
            grant_types: None,
            response_types: Some(vec!["code".to_string()]),
            token_endpoint_auth_method: Some("none".to_string()),
        };
        let client = validate_registration(&body).unwrap();
        assert_eq!(client.name, "Claude Code");
        assert_eq!(client.redirect_uris.len(), 1);
    }

    struct TempDir(std::path::PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn tokens_are_stored_as_hashes_in_database() -> anyhow::Result<()> {
        let dir = TempDir(std::env::temp_dir().join(format!("mcp-tok-{}", rand::random::<u64>())));
        std::fs::create_dir_all(&dir.0)?;
        let db_path = dir.0.join("tokens.db");
        let mut auth = McpAuthorizations::open(&db_path).await?;
        let issued = auth.issue_tokens("test-client", "read", "admin").await;

        assert!(auth.verify(&issued.access_token).is_some());

        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", db_path.display())).await?;
        let row = sqlx::query("SELECT access_token, refresh_token FROM mcp_tokens")
            .fetch_one(&pool)
            .await?;
        let db_access: String = row.try_get("access_token")?;
        let db_refresh: String = row.try_get("refresh_token")?;

        assert!(db_access.starts_with("sha256:"));
        assert!(db_refresh.starts_with("sha256:"));
        assert_ne!(db_access, issued.access_token);
        assert_ne!(db_refresh, issued.refresh_token);
        Ok(())
    }
}
