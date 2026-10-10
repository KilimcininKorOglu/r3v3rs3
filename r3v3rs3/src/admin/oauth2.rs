//! The OAuth 2.1 authorization server that protects the MCP endpoint of the admin API.
//!
//! A pre-registered client starts the authorization code flow with PKCE. The account signs in
//! through the WebUI session, approves the client on a plain form, and the client trades the
//! code for an opaque access token with a refresh token at the token endpoint. The tokens live
//! in memory only, so a restart signs every MCP client out.

use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::audit::AuditRecord;
use axum::{
    Json,
    extract::{Form, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use r3v3rs3_api::app::{McpClientConfig, McpConfig};
use r3v3rs3_api::audit::AuditAction;
use r3v3rs3_api::error::Error;
use rand::distr::{Alphanumeric, SampleString};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
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
    expires_at: u64,
    refresh_expires_at: u64,
    client_id: String,
    username: String,
    pub scope: String,
    refresh_token: String,
}

/// The codes and tokens of the signed-in MCP clients, in memory only.
#[derive(Default)]
pub struct McpAuthorizations {
    codes: HashMap<String, AuthorizationCode>,
    tokens: HashMap<String, AccessToken>,
    refresh: HashMap<String, String>,
}

impl McpAuthorizations {
    /// Stores an approved authorization code.
    fn issue_code(&mut self, grant: AuthorizationCode) -> String {
        self.cleanup();
        let code = new_token();
        self.codes.insert(code.clone(), grant);
        code
    }

    /// Trades an authorization code for tokens after the PKCE check.
    fn exchange_code(
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
        let issued = self.issue_tokens(&grant.client_id, &grant.scope, &grant.username);
        Ok((username, issued))
    }

    /// Issues a fresh pair of tokens for the account.
    fn issue_tokens(&mut self, client_id: &str, scope: &str, username: &str) -> IssuedTokens {
        let access_token = new_token();
        let refresh_token = new_token();
        let now = unix_now();
        self.tokens.insert(
            access_token.clone(),
            AccessToken {
                expires_at: now + ACCESS_TOKEN_LIFETIME.as_secs(),
                refresh_expires_at: now + REFRESH_TOKEN_LIFETIME.as_secs(),
                client_id: client_id.to_string(),
                username: username.to_string(),
                scope: scope.to_string(),
                refresh_token: refresh_token.clone(),
            },
        );
        self.refresh
            .insert(refresh_token.clone(), access_token.clone());
        IssuedTokens {
            access_token,
            token_type: "Bearer",
            refresh_token,
            expires_in: ACCESS_TOKEN_LIFETIME.as_secs(),
            scope: scope.to_string(),
        }
    }

    /// Rotates the tokens of a refresh token: both tokens get fresh values.
    fn refresh_tokens(&mut self, refresh_token: &str) -> Option<IssuedTokens> {
        let access = self.refresh.remove(refresh_token)?;
        let grant = self.tokens.remove(&access)?;
        if grant.refresh_expires_at <= unix_now() {
            self.refresh.remove(&grant.refresh_token);
            return None;
        }
        Some(self.issue_tokens(&grant.client_id, &grant.scope, &grant.username))
    }

    /// Validates an access token and returns the client that it belongs to.
    pub fn verify(&self, token: &str) -> Option<&AccessToken> {
        self.tokens
            .get(token)
            .filter(|grant| grant.expires_at > unix_now())
    }

    /// Drops the expired codes and tokens.
    fn cleanup(&mut self) {
        if self.codes.len() + self.tokens.len() < MAX_GRANTS {
            return;
        }
        let now = unix_now();
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
            }
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

/// Validates the authorization request and returns the registered client.
fn validate_authorize<'a>(
    config: &'a McpConfig,
    query: &AuthorizeQuery,
) -> Result<&'a McpClientConfig, Error> {
    if !config.enabled {
        return Err(Error::McpDisabled);
    }
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
    let client = McpClientConfig::find(&config.clients, &query.client_id).ok_or_else(|| {
        Error::McpClientUnknown {
            id: query.client_id.clone(),
        }
    })?;
    if !client.allows_redirect(&query.redirect_uri) {
        return Err(Error::McpRedirectInvalid);
    }
    Ok(client)
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
pub async fn authorization_server_metadata(headers: HeaderMap) -> Response {
    let origin = base_origin(&headers);
    Json(json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/oauth/authorize"),
        "token_endpoint": format!("{origin}/oauth/token"),
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
pub async fn protected_resource_metadata(headers: HeaderMap) -> Response {
    let origin = base_origin(&headers);
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
    axum::Extension(caller): axum::Extension<crate::accounts::Caller>,
    Query(query): Query<AuthorizeQuery>,
) -> Result<Response, AppError> {
    let config = state.data.lock().await.config.mcp.clone();
    let client = validate_authorize(&config, &query)?;
    let scopes = scopes_of(&query.scope);
    let scope = scopes.join(" ");
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>r3v3rs3</title></head>\
         <body><h1>Authorize {name}</h1>\
         <p>The MCP client <strong>{name}</strong> (client <code>{id}</code>) requests access \
         to this r3v3rs3 server as the account <strong>{username}</strong>.</p>\
         <p>Requested scopes: <code>{scope}</code> (read: view the state, edit: change proxies).</p>\
         <form method=\"post\" action=\"/oauth/authorize\">\
         <input type=\"hidden\" name=\"client_id\" value=\"{client_id}\">\
         <input type=\"hidden\" name=\"redirect_uri\" value=\"{redirect_uri}\">\
         <input type=\"hidden\" name=\"state\" value=\"{state_value}\">\
         <input type=\"hidden\" name=\"code_challenge\" value=\"{challenge}\">\
         <input type=\"hidden\" name=\"scope\" value=\"{scope}\">\
         <button type=\"submit\" name=\"decision\" value=\"approve\">Approve</button>\
         <button type=\"submit\" name=\"decision\" value=\"deny\">Deny</button>\
         </form></body></html>",
        name = html_escape(&client.name),
        id = html_escape(&client.id),
        username = html_escape(&caller.username),
        scope = html_escape(&scope),
        client_id = html_escape(&query.client_id),
        redirect_uri = html_escape(&query.redirect_uri),
        state_value = html_escape(&query.state),
        challenge = html_escape(&query.code_challenge),
    );
    let mut response = Html(body).into_response();
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    Ok(response)
}

/// The smallest HTML escaping that the consent form needs.
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
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
    axum::Extension(caller): axum::Extension<Caller>,
    Form(form): Form<ConsentForm>,
) -> Result<Response, AppError> {
    let config = state.data.lock().await.config.mcp.clone();
    let client = validate_authorize(
        &config,
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
    if form.decision != "approve" {
        let separator = if form.redirect_uri.contains('?') {
            '&'
        } else {
            '?'
        };
        return Ok(Redirect::to(&format!(
            "{}{separator}state={}&error=access_denied",
            form.redirect_uri,
            urlencoding_escape(&form.state)
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
    let separator = if form.redirect_uri.contains('?') {
        '&'
    } else {
        '?'
    };
    Ok(Redirect::to(&format!(
        "{}{separator}code={}&state={}",
        form.redirect_uri,
        urlencoding_escape(&code),
        urlencoding_escape(&form.state)
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
            if McpClientConfig::find(&config.clients, client_id).is_none() {
                return oauth_error("invalid_client", "unknown client");
            }
            let mut data = state.data.lock().await;
            match data
                .mcp
                .exchange_code(code, client_id, redirect_uri, verifier)
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
            match data.mcp.refresh_tokens(refresh_token) {
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

/// The 401 response that points the client at the resource metadata, per RFC 9728.
fn unauthorized(headers: &HeaderMap) -> Response {
    let origin = base_origin(headers);
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
    let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return unauthorized(&headers);
    };
    let (username, scope) = {
        let data = state.data.lock().await;
        match data.mcp.verify(token) {
            Some(grant) => (grant.username.clone(), grant.scope.clone()),
            None => return unauthorized(&headers),
        }
    };
    let record = crate::sessions::SessionRecord::new(&username);
    let Some(mut caller) = state.accounts.borrow().caller(&record) else {
        return unauthorized(&headers);
    };
    caller.client = Some(addr.ip());
    request.extensions_mut().insert(caller);
    request.extensions_mut().insert(super::McpScope(scope));
    next.run(request).await
}
