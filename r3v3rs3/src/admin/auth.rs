use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::audit::AuditRecord;
use crate::server::rpc::auth::VerifyAccount;
use crate::sessions::{self, SessionBackend, SessionRecord, SessionScope};
use axum::{
    Extension, Json,
    extract::{ConnectInfo, Path, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use r3v3rs3_api::{
    audit::AuditAction,
    auth::{
        ApiTokenCreated, ApiTokenInfo, CreateApiTokenRequest, LoginMethod, LoginRequest,
        LoginResponse, PasskeyChallenge, PasskeyCredential, PasskeyRegisterRequest, Role,
        SessionInfo, TokenScope,
    },
    client_ip::canonical_client_ip,
    error::{Error, ErrorMessage},
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::warn;

fn resolve_login_username(state: &AppState, request: &mut LoginRequest) {
    if request.username.is_empty()
        && let LoginMethod::Passkey {
            ref credential_id, ..
        } = request.method
        && let Some(user) = state.accounts.borrow().find_by_passkey(credential_id)
    {
        request.username = user;
    }
}

async fn verify_pre_login_conditions(
    state: &AppState,
    jar: &CookieJar,
    request: &LoginRequest,
    attempt_key: &LoginAttemptKey,
) -> Result<(), Error> {
    let token = jar.get("token").map(|c| c.value().to_string());
    if let LoginMethod::Totp { .. } = &request.method
        && !verify_login_session(state, &token.unwrap_or_default(), &attempt_key.1).await
    {
        return Err(Error::InvalidLoginCredentials);
    }
    if let LoginMethod::Passkey {
        ref client_data_json,
        ..
    } = request.method
        && !verify_and_consume_challenge(state, client_data_json, "webauthn.get").await
    {
        return Err(Error::InvalidLoginCredentials);
    }
    Ok(())
}

async fn record_successful_login(
    state: &AppState,
    username: &str,
    peer_ip: &IpAddr,
    attempt_key: &LoginAttemptKey,
) {
    let mut data = state.data.lock().await;
    data.login_attempts.clear(attempt_key);
    data.ban_tracker.clear(peer_ip);
    drop(data);
    let record = AuditRecord::new(AuditAction::Login);
    state.record_audit(username, Some(*peer_ip), record).await;
}

fn header_ip(headers: &axum::http::HeaderMap, name: &'static str) -> Option<IpAddr> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
}

fn resolve_client_ip(peer: &SocketAddr, headers: &axum::http::HeaderMap) -> IpAddr {
    if !peer.ip().is_loopback() {
        return peer.ip();
    }
    header_ip(headers, "x-forwarded-for")
        .or_else(|| header_ip(headers, "x-real-ip"))
        .unwrap_or_else(|| peer.ip())
}

/// Signs in and sets the `token` session cookie. An account with TOTP returns `totp_required`
/// first, and a second request with the TOTP code completes the sign-in.
#[utoipa::path(
    post,
    path = "/login",
    tag = "auth",
    operation_id = "login",
    security(()),
    request_body = LoginRequest,
    responses(
        (status = 200, description = "The sign-in step succeeded.", body = LoginResponse),
        (status = 400, description = "The credentials are not valid.", body = ErrorMessage),
        (status = 429, description = "The client made too many sign-in attempts.", body = ErrorMessage),
        (status = 500, description = "The server failed to handle the request.", body = ErrorMessage)
    )
)]
pub async fn login(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
    Json(mut request): Json<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    let client_ip = resolve_client_ip(&peer, &headers);
    resolve_login_username(&state, &mut request);
    let username = request.username.clone();
    let attempt_key = (canonical_client_ip(client_ip), username.clone());
    ensure_login_allowed(&state, &attempt_key).await?;

    if let Err(err) = verify_pre_login_conditions(&state, &jar, &request, &attempt_key).await {
        record_login_failure(&state, attempt_key).await;
        return Err(err.into());
    }

    let insecure = request.insecure;
    let result = match state.call_system(VerifyAccount { request }).await {
        Ok(result) => result,
        Err(err) => {
            record_login_failure(&state, attempt_key).await;
            return Err(err.into());
        }
    };

    let scope = match *result {
        LoginResponse::Success => {
            record_successful_login(&state, &username, &client_ip, &attempt_key).await;
            SessionScope::Admin
        }
        _ => SessionScope::Login,
    };

    let (backend, expiry) = session_backend(&state).await;
    let token = backend
        .create(scope, SessionRecord::new(&username), expiry)
        .await?;

    // Lax instead of Strict, because the MCP authorization flow reaches /oauth/authorize as a
    // cross-site top-level navigation. State-changing requests are POST, PUT or DELETE, which
    // Lax does not send on a cross-site navigation. The path covers the whole panel, because
    // a cookie without a path would stay on /api and a page reload would sign the client out.
    let cookie = Cookie::build(("token", token))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(!insecure)
        .build();

    Ok((jar.add(cookie), Json(result)))
}

/// The sessions and their lifetime. The lock is released before the sessions are used, because
/// the sessions of a cluster wait for the store.
async fn session_backend(state: &AppState) -> (Arc<dyn SessionBackend>, Duration) {
    let data = state.data.lock().await;
    (data.sessions.clone(), sessions::expiry(&data.config.admin))
}

async fn ensure_login_allowed(state: &AppState, key: &LoginAttemptKey) -> Result<(), Error> {
    let mut data = state.data.lock().await;
    let now = Instant::now();
    data.ban_tracker.check_banned(&key.0, now)?;
    let reset = data.config.admin.login_attempts_reset;
    if data.login_attempts.is_blocked(key, reset, now) {
        return Err(Error::TooManyLoginAttempts);
    }
    Ok(())
}

/// Counts a failed sign-in and records it in the audit log.
async fn record_login_failure(state: &AppState, key: LoginAttemptKey) {
    let record = AuditRecord::new(AuditAction::LoginFailed);
    state.record_audit(&key.1, Some(key.0), record).await;
    let mut data = state.data.lock().await;
    let admin = data.config.admin.clone();
    let now = Instant::now();
    data.login_attempts.record_failure(
        key.clone(),
        admin.max_login_attempts,
        admin.login_attempts_reset,
        now,
    );
    let ban_threshold = admin.max_login_attempts.saturating_mul(2).max(5);
    data.ban_tracker.record_failure(
        key.0,
        ban_threshold,
        admin.login_attempts_reset,
        admin.login_attempts_reset.max(Duration::from_secs(300)),
        now,
    );
}

async fn verify_login_session(state: &AppState, token: &str, username: &str) -> bool {
    let (backend, expiry) = session_backend(state).await;
    match backend.get(SessionScope::Login, token).await {
        Ok(record) => {
            record.is_some_and(|record| record.is_active(expiry) && record.subject == username)
        }
        Err(err) => {
            warn!(%err, "failed to read the sign-in session");
            false
        }
    }
}

/// Ends the session and removes the `token` cookie.
#[utoipa::path(
    get,
    path = "/logout",
    tag = "auth",
    operation_id = "logout",
    security(()),
    responses((status = 200, description = "The session is ended."))
)]
pub async fn logout(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    jar: CookieJar,
) -> impl IntoResponse {
    if let Some(token) = jar.get("token") {
        let (backend, _) = session_backend(&state).await;
        record_logout(&state, backend.as_ref(), token.value(), peer).await;
        for scope in [SessionScope::Admin, SessionScope::Login] {
            if let Err(err) = backend.remove(scope, token.value()).await {
                warn!(%err, "failed to end the session");
            }
        }
    }
    // The removal cookie carries the path of the sign-in cookie, so the browser deletes it.
    jar.add(Cookie::build(("token", "")).path("/").build())
}

/// Records the sign-out of the account of an admin session.
async fn record_logout(
    state: &AppState,
    backend: &dyn SessionBackend,
    token: &str,
    peer: SocketAddr,
) {
    match backend.get(SessionScope::Admin, token).await {
        Ok(Some(record)) => {
            let audit = AuditRecord::new(AuditAction::Logout);
            state
                .record_audit(&record.subject, Some(peer.ip()), audit)
                .await;
        }
        Ok(None) => {}
        Err(err) => warn!(%err, "failed to read the session"),
    }
}

/// Returns the account of the current session, so the WebUI shows only the pages of its role.
#[utoipa::path(
    get,
    path = "/",
    tag = "auth",
    operation_id = "get_session",
    responses((status = 200, description = "The account of the session.", body = SessionInfo), ErrorResponses)
)]
pub async fn session(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Json<SessionInfo> {
    let cert_expiry_warning = state
        .data
        .lock()
        .await
        .config
        .notifications
        .cert_expiry_warning;
    Json(SessionInfo {
        username: caller.username,
        role: caller.role,
        proxies: caller.proxies,
        cert_expiry_warning,
        display_timezone: state
            .data
            .lock()
            .await
            .config
            .admin
            .display_timezone
            .clone(),
    })
}

/// Returns a new WebAuthn challenge for passkey sign-in or registration.
#[utoipa::path(
    get,
    path = "/passkey/challenge",
    tag = "auth",
    operation_id = "get_passkey_challenge",
    security(()),
    responses(
        (status = 200, description = "A new WebAuthn challenge.", body = PasskeyChallenge),
        (status = 500, description = "Failed to generate challenge.", body = ErrorMessage)
    )
)]
pub async fn get_passkey_challenge(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<PasskeyChallenge>, AppError> {
    let mut challenge_bytes = [0u8; 32];
    use aws_lc_rs::rand::{SecureRandom, SystemRandom};
    SystemRandom::new()
        .fill(&mut challenge_bytes)
        .map_err(|_| Error::InvalidPasskey)?;
    let challenge = URL_SAFE_NO_PAD.encode(challenge_bytes);
    let rp_id = extract_rp_id(&state, &headers).await;

    let mut data = state.data.lock().await;
    data.passkey_challenges
        .insert(challenge.clone(), Instant::now());

    Ok(Json(PasskeyChallenge { challenge, rp_id }))
}

/// Registers a new WebAuthn passkey for the current account.
#[utoipa::path(
    post,
    path = "/passkey/register",
    tag = "auth",
    operation_id = "register_passkey",
    request_body = PasskeyRegisterRequest,
    responses(
        (status = 200, description = "The passkey is registered."),
        (status = 400, description = "The passkey details are invalid.", body = ErrorMessage),
        (status = 401, description = "Unauthorized.", body = ErrorMessage)
    )
)]
pub async fn register_passkey(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<PasskeyRegisterRequest>,
) -> Result<Json<()>, AppError> {
    if request.name.trim().is_empty() || request.credential_id.trim().is_empty() {
        return Err(Error::InvalidPasskey.into());
    }
    if crate::config::account::decode_p256_public_key(&request.public_key).is_none() {
        return Err(Error::InvalidPasskey.into());
    }
    let Some(client_data) = &request.client_data_json else {
        return Err(Error::InvalidPasskey.into());
    };
    if !verify_and_consume_challenge(&state, client_data, "webauthn.create").await {
        return Err(Error::InvalidPasskey.into());
    }

    let credential = PasskeyCredential {
        id: request.credential_id.trim().to_string(),
        name: request.name.trim().to_string(),
        public_key: request.public_key.trim().to_string(),
        counter: 0,
        created_at: crate::clock::unix_secs(),
    };

    state
        .call(
            &caller,
            crate::server::rpc::accounts::RegisterPasskey {
                username: caller.username.clone(),
                credential,
            },
        )
        .await?;

    Ok(Json(()))
}

/// Deletes a registered WebAuthn passkey of the current account.
#[utoipa::path(
    delete,
    path = "/passkey/{id}",
    tag = "auth",
    operation_id = "delete_passkey",
    params(("id" = String, Path, description = "Credential ID.")),
    responses(
        (status = 200, description = "The passkey is deleted."),
        (status = 401, description = "Unauthorized.", body = ErrorMessage),
        (status = 404, description = "Passkey not found.", body = ErrorMessage)
    )
)]
pub async fn delete_passkey(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<()>, AppError> {
    state
        .call(
            &caller,
            crate::server::rpc::accounts::DeletePasskey {
                username: caller.username.clone(),
                credential_id: id,
            },
        )
        .await?;
    Ok(Json(()))
}

/// Returns the API tokens belonging to the current account.
#[utoipa::path(
    get,
    path = "/tokens",
    tag = "auth",
    operation_id = "list_tokens",
    responses(
        (status = 200, description = "The API tokens of the account.", body = [ApiTokenInfo]),
        ErrorResponses
    )
)]
pub async fn list_tokens(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Result<Json<Vec<ApiTokenInfo>>, AppError> {
    let accounts = state.accounts.borrow();
    let account = accounts
        .get(&caller.username)
        .ok_or_else(|| Error::AccountNotFound {
            username: caller.username.clone(),
        })?;
    let tokens = account
        .tokens
        .iter()
        .map(|tok| ApiTokenInfo {
            id: tok.id.clone(),
            name: tok.name.clone(),
            scope: tok.scope,
            created_at: tok.created_at,
        })
        .collect();
    Ok(Json(tokens))
}

/// Creates a persistent API token for the current account.
#[utoipa::path(
    post,
    path = "/tokens",
    tag = "auth",
    operation_id = "create_token",
    request_body = CreateApiTokenRequest,
    responses(
        (status = 200, description = "The API token was created.", body = ApiTokenCreated),
        ErrorResponses
    )
)]
pub async fn create_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(request): Json<CreateApiTokenRequest>,
) -> Result<Json<ApiTokenCreated>, AppError> {
    if caller.role == Role::Viewer && request.scope == TokenScope::Edit {
        return Err(Error::Forbidden.into());
    }
    let result = state
        .call(
            &caller,
            crate::server::rpc::accounts::CreateApiToken {
                username: caller.username.clone(),
                name: request.name,
                scope: request.scope,
            },
        )
        .await?;
    Ok(Json(*result))
}

/// Deletes an API token belonging to the current account.
#[utoipa::path(
    delete,
    path = "/tokens/{id}",
    tag = "auth",
    operation_id = "delete_token",
    params(("id" = String, Path, description = "Token ID.")),
    responses(
        (status = 200, description = "The API token was deleted."),
        ErrorResponses
    )
)]
pub async fn delete_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> Result<Json<()>, AppError> {
    state
        .call(
            &caller,
            crate::server::rpc::accounts::DeleteApiToken {
                username: caller.username.clone(),
                token_id: id,
            },
        )
        .await?;
    Ok(Json(()))
}

fn sanitize_rp_domain(raw: &str) -> String {
    let domain = raw.split(':').next().unwrap_or("localhost");
    if domain == "127.0.0.1" || domain == "::1" {
        "localhost".to_string()
    } else {
        domain.to_string()
    }
}

fn domain_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(fwd) = headers
        .get("x-forwarded-host")
        .and_then(|h| h.to_str().ok())
    {
        let host = fwd.split(',').next().unwrap_or("").trim();
        if !host.is_empty() {
            return Some(sanitize_rp_domain(host));
        }
    }
    let host = headers.get("host").and_then(|h| h.to_str().ok())?;
    Some(sanitize_rp_domain(host))
}

async fn extract_rp_id(state: &AppState, headers: &axum::http::HeaderMap) -> String {
    if let Some(domain) = domain_from_headers(headers) {
        return domain;
    }
    let data = state.data.lock().await;
    if let Some(url) = &data.config.public_url
        && let Ok(parsed) = url::Url::parse(url.as_str())
        && let Some(host) = parsed.host_str()
    {
        return host.to_string();
    }
    "localhost".to_string()
}

fn is_valid_origin(origin_str: &str, expected_rp_id: &str) -> bool {
    let Ok(parsed) = url::Url::parse(origin_str) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let origin_clean = host.trim_start_matches('[').trim_end_matches(']');
    let rp_clean = expected_rp_id.trim_start_matches('[').trim_end_matches(']');

    if origin_clean.eq_ignore_ascii_case(rp_clean) {
        return true;
    }
    if origin_clean.ends_with(&format!(".{rp_clean}")) {
        return true;
    }
    let is_origin_local =
        origin_clean == "localhost" || origin_clean == "127.0.0.1" || origin_clean == "::1";
    let is_rp_local = rp_clean == "localhost" || rp_clean == "127.0.0.1" || rp_clean == "::1";
    is_origin_local && is_rp_local
}

pub async fn verify_and_consume_challenge(
    state: &AppState,
    client_data_b64: &str,
    expected_type: &str,
) -> bool {
    let Some(bytes) = crate::config::account::decode_base64_or_hex(client_data_b64) else {
        return false;
    };
    let Ok(client_data) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    if client_data.get("type").and_then(|v| v.as_str()) != Some(expected_type) {
        return false;
    }
    let Some(origin) = client_data.get("origin").and_then(|v| v.as_str()) else {
        return false;
    };
    let rp_id = extract_rp_id(state, &axum::http::HeaderMap::new()).await;
    if !is_valid_origin(origin, &rp_id) {
        return false;
    }
    let Some(challenge) = client_data.get("challenge").and_then(|v| v.as_str()) else {
        return false;
    };
    let mut data = state.data.lock().await;
    data.passkey_challenges.consume(challenge, Instant::now())
}

pub(crate) fn api_token_caller(state: &AppState, token: &str) -> Option<Caller> {
    let token_hash = hex::encode(Sha256::digest(token.as_bytes()));
    state.accounts.borrow().token_caller(&token_hash)
}

async fn resolve_caller(
    state: &AppState,
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> Result<Caller, Error> {
    if let Some(token) = bearer {
        if let Some(caller) = api_token_caller(state, token) {
            return Ok(caller);
        }
        if let Ok(caller) = bearer_caller(state, token).await {
            return Ok(caller);
        }
        if let Ok(caller) = session_caller(state, token).await {
            return Ok(caller);
        }
    }
    if let Some(token) = cookie
        && let Ok(caller) = session_caller(state, token).await
    {
        return Ok(caller);
    }
    Err(Error::Unauthorized)
}

/// Accepts a request with an active admin session, API token, or valid OAuth Bearer token and adds its `Caller` to the request.
pub async fn verify(
    State(state): State<AppState>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    let bearer = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|val| val.to_str().ok())
        .and_then(|val| val.strip_prefix("Bearer "));
    let cookie = jar.get("token").map(|c| c.value());
    match resolve_caller(&state, bearer, cookie).await {
        Ok(mut caller) => {
            caller.client = request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip());
            request.extensions_mut().insert(caller);
            next.run(request).await
        }
        Err(err) => AppError::R3v3rs3(err).into_response(),
    }
}

async fn bearer_caller(state: &AppState, token: &str) -> Result<Caller, Error> {
    let username = {
        let data = state.data.lock().await;
        if !data.config.mcp.enabled {
            return Err(Error::Unauthorized);
        }
        match data.mcp.verify(token) {
            Some(grant) => grant.username().to_string(),
            None => return Err(Error::Unauthorized),
        }
    };
    let record = crate::sessions::SessionRecord::new(&username);
    state
        .accounts
        .borrow()
        .caller(&record)
        .ok_or(Error::Unauthorized)
}

/// The account of an active admin session. The account must exist, and its last change must not
/// be later than the start of the session.
pub(crate) async fn session_caller(state: &AppState, token: &str) -> Result<Caller, Error> {
    let (backend, expiry) = session_backend(state).await;
    let record = backend
        .get(SessionScope::Admin, token)
        .await?
        .filter(|record| record.is_active(expiry))
        .ok_or(Error::Unauthorized)?;
    let caller = state.accounts.borrow().caller(&record);
    caller.ok_or(Error::Unauthorized)
}

pub type LoginAttemptKey = (IpAddr, String);

#[derive(Debug, Clone, Copy)]
struct LoginAttempt {
    failures: u32,
    window_start: Instant,
    blocked_until: Option<Instant>,
}

/// Counts failed logins per client IP and username.
#[derive(Default)]
pub struct LoginAttempts {
    entries: HashMap<LoginAttemptKey, LoginAttempt>,
}

impl LoginAttempts {
    /// Returns true while the key is blocked. Expired entries are dropped.
    pub fn is_blocked(&mut self, key: &LoginAttemptKey, reset: Duration, now: Instant) -> bool {
        self.entries
            .retain(|_, attempt| attempt.is_active(reset, now));
        self.entries
            .get(key)
            .and_then(|attempt| attempt.blocked_until)
            .is_some_and(|until| until > now)
    }

    pub fn record_failure(
        &mut self,
        key: LoginAttemptKey,
        max_attempts: u32,
        reset: Duration,
        now: Instant,
    ) {
        let attempt = self.entries.entry(key).or_insert(LoginAttempt {
            failures: 0,
            window_start: now,
            blocked_until: None,
        });
        attempt.failures += 1;
        if attempt.failures >= max_attempts.max(1) {
            attempt.blocked_until = Some(now + reset);
        }
    }

    pub fn clear(&mut self, key: &LoginAttemptKey) {
        self.entries.remove(key);
    }
}

impl LoginAttempt {
    fn is_active(&self, reset: Duration, now: Instant) -> bool {
        match self.blocked_until {
            Some(until) => until > now,
            None => now.duration_since(self.window_start) < reset,
        }
    }
}

/// Active WebAuthn challenges with 2-minute expiration.
#[derive(Default)]
pub struct PasskeyChallenges {
    entries: HashMap<String, Instant>,
}

impl PasskeyChallenges {
    pub fn insert(&mut self, challenge: String, now: Instant) {
        self.entries
            .retain(|_, time| now.duration_since(*time) < Duration::from_secs(120));
        self.entries.insert(challenge, now);
    }

    pub fn consume(&mut self, challenge: &str, now: Instant) -> bool {
        self.entries
            .retain(|_, time| now.duration_since(*time) < Duration::from_secs(120));
        self.entries.remove(challenge).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn key(name: &str) -> LoginAttemptKey {
        (IpAddr::V4(Ipv4Addr::LOCALHOST), name.to_string())
    }

    #[test]
    fn blocks_after_max_failures_until_reset() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        attempts.record_failure(key("admin"), 2, reset, now);
        assert!(!attempts.is_blocked(&key("admin"), reset, now));

        attempts.record_failure(key("admin"), 2, reset, now);
        assert!(attempts.is_blocked(&key("admin"), reset, now));
        assert!(!attempts.is_blocked(&key("other"), reset, now));

        let later = now + reset + Duration::from_secs(1);
        assert!(!attempts.is_blocked(&key("admin"), reset, later));
    }

    #[test]
    fn failures_outside_window_are_forgotten() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        attempts.record_failure(key("admin"), 2, reset, now);
        let later = now + reset + Duration::from_secs(1);
        assert!(!attempts.is_blocked(&key("admin"), reset, later));

        attempts.record_failure(key("admin"), 2, reset, later);
        assert!(!attempts.is_blocked(&key("admin"), reset, later));
    }

    #[test]
    fn clear_resets_failures() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        attempts.record_failure(key("admin"), 2, reset, now);
        attempts.clear(&key("admin"));
        attempts.record_failure(key("admin"), 2, reset, now);
        assert!(!attempts.is_blocked(&key("admin"), reset, now));
    }

    #[test]
    fn failures_from_same_ipv6_prefix_share_attempts() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        let ip1: IpAddr = "2001:db8::1".parse().unwrap();
        let ip2: IpAddr = "2001:db8::2".parse().unwrap();
        let key1 = (canonical_client_ip(ip1), "admin".to_string());
        let key2 = (canonical_client_ip(ip2), "admin".to_string());

        attempts.record_failure(key1.clone(), 2, reset, now);
        assert!(!attempts.is_blocked(&key1, reset, now));
        assert!(!attempts.is_blocked(&key2, reset, now));

        attempts.record_failure(key2.clone(), 2, reset, now);
        assert!(attempts.is_blocked(&key1, reset, now));
        assert!(attempts.is_blocked(&key2, reset, now));
    }

    #[test]
    fn origin_validation_rules() {
        assert!(is_valid_origin("http://localhost:46492", "localhost"));
        assert!(is_valid_origin("http://127.0.0.1:46492", "localhost"));
        assert!(is_valid_origin("https://admin.example.com", "example.com"));
        assert!(is_valid_origin("https://example.com", "example.com"));
        assert!(!is_valid_origin("https://evil.com", "example.com"));
        assert!(!is_valid_origin(
            "https://attacker-example.com",
            "example.com"
        ));
        assert!(!is_valid_origin("not-a-url", "example.com"));
    }
}
