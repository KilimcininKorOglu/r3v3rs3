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
        SessionInfo, TokenScope, validate_username,
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

/// Name of the session cookie for the admin panel and admin API.
pub const ADMIN_SESSION_COOKIE: &str = "token";

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

async fn verify_totp_preconditions(
    state: &AppState,
    token: Option<&str>,
    username: &str,
) -> Result<SessionRecord, Error> {
    let Some(token) = token else {
        return Err(Error::InvalidLoginCredentials);
    };
    let Some(record) = verify_login_session(state, token, username).await else {
        return Err(Error::InvalidLoginCredentials);
    };
    let mut data = state.data.lock().await;
    let reset = data.config.admin.login_attempts_reset;
    if data.totp_attempts.is_account_blocked(username, reset, Instant::now()) {
        return Err(Error::TooManyLoginAttempts);
    }
    Ok(record)
}

async fn verify_pre_login_conditions(
    state: &AppState,
    jar: &CookieJar,
    request: &LoginRequest,
    attempt_key: &LoginAttemptKey,
) -> Result<Option<SessionRecord>, Error> {
    let token = jar.get(ADMIN_SESSION_COOKIE).map(|c| c.value());
    let mut login_record = None;
    if let LoginMethod::Totp { .. } = &request.method {
        let rec = verify_totp_preconditions(state, token, &attempt_key.1).await?;
        login_record = Some(rec);
    }
    if let LoginMethod::Passkey {
        ref client_data_json,
        ..
    } = request.method
        && !verify_and_consume_challenge(state, client_data_json, "webauthn.get").await
    {
        return Err(Error::InvalidLoginCredentials);
    }
    Ok(login_record)
}

async fn cleanup_login_session(state: &AppState, token: &str, username: &str) {
    let backend = {
        let mut data = state.data.lock().await;
        data.totp_attempts.clear(token, username);
        data.sessions.clone()
    };
    let _ = backend.remove(SessionScope::Login, token).await;
}

async fn record_totp_failure(state: &AppState, token: &str, username: &str) {
    let (exhausted, backend) = {
        let mut data = state.data.lock().await;
        let max = data.config.admin.max_login_attempts;
        let reset = data.config.admin.login_attempts_reset;
        let exhausted =
            data.totp_attempts
                .record_failure(token, username, max, reset, Instant::now());
        (exhausted, data.sessions.clone())
    };
    if exhausted {
        let _ = backend.remove(SessionScope::Login, token).await;
    }
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

pub(crate) fn header_ip(headers: &axum::http::HeaderMap, name: &'static str) -> Option<IpAddr> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
}

async fn resolve_client_ip(
    state: &AppState,
    peer: &SocketAddr,
    headers: &axum::http::HeaderMap,
) -> IpAddr {
    let trusted = {
        let data = state.data.lock().await;
        data.config.admin.trusted_proxies.clone()
    };
    if !trusted.iter().any(|net| net.contains(&peer.ip())) {
        return peer.ip();
    }
    header_ip(headers, "x-forwarded-for")
        .or_else(|| header_ip(headers, "x-real-ip"))
        .unwrap_or_else(|| peer.ip())
}

async fn is_secure_connection(
    state: &AppState,
    peer: &SocketAddr,
    headers: &axum::http::HeaderMap,
) -> bool {
    let trusted = {
        let data = state.data.lock().await;
        data.config.admin.trusted_proxies.clone()
    };
    is_secure_request(&trusted, peer, headers)
}

fn is_secure_request(
    trusted_proxies: &[ipnet::IpNet],
    peer: &SocketAddr,
    headers: &axum::http::HeaderMap,
) -> bool {
    if trusted_proxies.iter().any(|net| net.contains(&peer.ip())) {
        if let Some(proto) = headers
            .get("x-forwarded-proto")
            .or_else(|| headers.get("x-forwarded-scheme"))
            .and_then(|v| v.to_str().ok())
        {
            return proto
                .split(',')
                .next()
                .map(|s| s.trim().eq_ignore_ascii_case("https"))
                .unwrap_or(false);
        }
    }
    !peer.ip().is_loopback()
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
    let client_ip = resolve_client_ip(&state, &peer, &headers).await;
    resolve_login_username(&state, &mut request);
    let username = request.username.clone();
    let attempt_key = (canonical_client_ip(client_ip), username.clone());
    ensure_login_allowed(&state, &attempt_key).await?;

    let is_totp = matches!(request.method, LoginMethod::Totp { .. });
    let login_cookie_token = jar.get(ADMIN_SESSION_COOKIE).map(|c| c.value().to_string());

    let login_record = match verify_pre_login_conditions(&state, &jar, &request, &attempt_key).await
    {
        Ok(rec) => rec,
        Err(err) => {
            record_login_failure(&state, attempt_key).await;
            return Err(err.into());
        }
    };

    let start_record = SessionRecord::new(&username);
    let is_secure = is_secure_connection(&state, &peer, &headers).await;
    let result = match state.call_system(VerifyAccount { request }).await {
        Ok(result) => result,
        Err(err) => {
            if is_totp && let Some(token) = &login_cookie_token {
                record_totp_failure(&state, token, &username).await;
            }
            record_login_failure(&state, attempt_key).await;
            return Err(err.into());
        }
    };

    let (scope, session_record) = match *result {
        LoginResponse::Success => {
            if let Some(token) = &login_cookie_token {
                cleanup_login_session(&state, token, &username).await;
            }
            record_successful_login(&state, &username, &client_ip, &attempt_key).await;
            (SessionScope::Admin, login_record.unwrap_or(start_record))
        }
        _ => {
            release_login_attempt(&state, &attempt_key).await;
            (SessionScope::Login, start_record)
        }
    };

    let (backend, expiry) = session_backend(&state).await;
    let token = backend
        .create(scope, session_record, expiry)
        .await?;

    // Lax instead of Strict, because the MCP authorization flow reaches /oauth/authorize as a
    // cross-site top-level navigation. State-changing requests are POST, PUT or DELETE, which
    // Lax does not send on a cross-site navigation. The path covers the whole panel, because
    // a cookie without a path would stay on /api and a page reload would sign the client out.
    let cookie = Cookie::build((ADMIN_SESSION_COOKIE, token))
        .http_only(true)
        .same_site(SameSite::Lax)
        .path("/")
        .secure(is_secure)
        .build();

    Ok((jar.add(cookie), Json(result)))
}

/// The sessions and their lifetime. The lock is released before the sessions are used, because
/// the sessions of a cluster wait for the store.
pub(crate) async fn session_backend(state: &AppState) -> (Arc<dyn SessionBackend>, Duration) {
    let data = state.data.lock().await;
    (data.sessions.clone(), sessions::expiry(&data.config.admin))
}

async fn ensure_login_allowed(state: &AppState, key: &LoginAttemptKey) -> Result<(), Error> {
    let mut data = state.data.lock().await;
    let now = Instant::now();
    data.ban_tracker.check_banned(&key.0, now)?;
    let reset = data.config.admin.login_attempts_reset;
    let max = data.config.admin.max_login_attempts;
    if !data.login_attempts.acquire(key, max, reset, now) {
        return Err(Error::TooManyLoginAttempts);
    }
    Ok(())
}

async fn release_login_attempt(state: &AppState, key: &LoginAttemptKey) {
    let mut data = state.data.lock().await;
    data.login_attempts.release(key);
}

pub(crate) fn sanitize_audit_username(state: &AppState, raw: &str) -> String {
    if validate_username(raw).is_err() {
        return "[invalid-username]".to_string();
    }
    if state.accounts.borrow().get(raw).is_some() {
        raw.to_string()
    } else {
        "[unknown-account]".to_string()
    }
}

/// Counts a failed sign-in and records it in the audit log.
async fn record_login_failure(state: &AppState, key: LoginAttemptKey) {
    let audit_username = sanitize_audit_username(state, &key.1);
    let record = AuditRecord::new(AuditAction::LoginFailed);
    state.record_audit(&audit_username, Some(key.0), record).await;
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

async fn verify_login_session(
    state: &AppState,
    token: &str,
    username: &str,
) -> Option<SessionRecord> {
    let (backend, expiry) = session_backend(state).await;
    let record = match backend.get(SessionScope::Login, token).await {
        Ok(rec) => rec?,
        Err(err) => {
            warn!(%err, "failed to read the sign-in session");
            return None;
        }
    };
    let is_active = record.is_active(expiry);
    let is_user = record.subject == username;
    let has_caller = state.accounts.borrow().caller(&record).is_some();
    if is_active && is_user && has_caller {
        Some(record)
    } else {
        None
    }
}

/// Ends the session and removes the `token` cookie.
#[utoipa::path(
    post,
    path = "/logout",
    tag = "auth",
    operation_id = "logout",
    security(()),
    responses(
        (status = 200, description = "The session is ended."),
        (status = 403, description = "Cross-origin request rejected.")
    )
)]
pub async fn logout(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    jar: CookieJar,
) -> Result<impl IntoResponse, AppError> {
    let public_url = {
        let data = state.data.lock().await;
        data.config.public_url.as_ref().map(|u| u.to_string())
    };
    verify_origin(&headers, public_url.as_deref())?;

    if let Some(token) = jar.get(ADMIN_SESSION_COOKIE) {
        let (backend, _) = session_backend(&state).await;
        record_logout(&state, backend.as_ref(), token.value(), peer).await;
        for scope in [SessionScope::Admin, SessionScope::Login] {
            if let Err(err) = backend.remove(scope, token.value()).await {
                warn!(%err, "failed to end the session");
            }
        }
    }
    // The removal cookie carries the path of the sign-in cookie, so the browser deletes it.
    Ok(jar.add(Cookie::build((ADMIN_SESSION_COOKIE, "")).path("/").build()))
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

pub(crate) fn matches_origin_host(origin: &url::Url, req_host: &str) -> bool {
    let Some(origin_host) = origin.host_str() else {
        return false;
    };
    let (req_hostname, req_port) = match req_host.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()),
        None => (req_host, None),
    };
    if !origin_host.eq_ignore_ascii_case(req_hostname) {
        return false;
    }
    let origin_port = origin.port_or_known_default();
    let expected_port = req_port.or_else(|| match origin.scheme() {
        "https" => Some(443),
        "http" => Some(80),
        _ => None,
    });
    origin_port == expected_port
}

pub(crate) fn matches_origin_public_url(origin: &url::Url, public_url: &str) -> bool {
    let Ok(pub_url) = url::Url::parse(public_url) else {
        return false;
    };
    origin.scheme() == pub_url.scheme()
        && origin.host_str() == pub_url.host_str()
        && origin.port_or_known_default() == pub_url.port_or_known_default()
}

fn has_preflight_custom_header(headers: &axum::http::HeaderMap) -> bool {
    headers.contains_key("x-requested-with")
        || headers.contains_key("x-csrf-token")
        || headers
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.starts_with("application/json"))
}

fn check_sec_fetch_site(headers: &axum::http::HeaderMap) -> Option<Result<(), Error>> {
    let site = headers.get("sec-fetch-site")?.to_str().ok()?;
    if site.eq_ignore_ascii_case("same-origin") {
        Some(Ok(()))
    } else if site.eq_ignore_ascii_case("cross-site") || site.eq_ignore_ascii_case("same-site") {
        Some(Err(Error::Unauthorized))
    } else {
        None
    }
}

fn matches_request_origin(
    origin: &url::Url,
    headers: &axum::http::HeaderMap,
    public_url: Option<&str>,
) -> bool {
    let host_matches = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|host| matches_origin_host(origin, host));
    let public_matches =
        public_url.is_some_and(|pub_url| matches_origin_public_url(origin, pub_url));
    host_matches || public_matches
}

pub(crate) fn verify_origin(
    headers: &axum::http::HeaderMap,
    public_url: Option<&str>,
) -> Result<(), Error> {
    if let Some(res) = check_sec_fetch_site(headers) {
        return res;
    }
    if has_preflight_custom_header(headers) {
        return Ok(());
    }
    let Some(origin_val) = headers
        .get(axum::http::header::ORIGIN)
        .or_else(|| headers.get(axum::http::header::REFERER))
    else {
        return Ok(());
    };
    let origin_str = origin_val.to_str().map_err(|_| Error::Unauthorized)?;
    let origin_url = url::Url::parse(origin_str).map_err(|_| Error::Unauthorized)?;
    if matches_request_origin(&origin_url, headers, public_url) {
        Ok(())
    } else {
        Err(Error::Unauthorized)
    }
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

#[derive(Clone, Debug)]
pub struct AuthSessionToken(pub String);

async fn resolve_caller(
    state: &AppState,
    bearer: Option<&str>,
    cookie: Option<&str>,
) -> Result<(Caller, Option<String>), Error> {
    if let Some(token) = bearer {
        if let Some(caller) = api_token_caller(state, token) {
            return Ok((caller, None));
        }
        if let Ok(caller) = bearer_caller(state, token).await {
            return Ok((caller, None));
        }
        if let Ok(caller) = session_caller(state, token).await {
            return Ok((caller, Some(token.to_string())));
        }
    }
    if let Some(token) = cookie
        && let Ok(caller) = session_caller(state, token).await
    {
        return Ok((caller, Some(token.to_string())));
    }
    Err(Error::Unauthorized)
}

fn is_state_changing(method: &axum::http::Method) -> bool {
    matches!(
        method,
        &axum::http::Method::POST
            | &axum::http::Method::PUT
            | &axum::http::Method::DELETE
            | &axum::http::Method::PATCH
    )
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
    let cookie = jar.get(ADMIN_SESSION_COOKIE).map(|c| c.value());
    match resolve_caller(&state, bearer, cookie).await {
        Ok((mut caller, session_token)) => {
            if session_token.is_some() && bearer.is_none() && is_state_changing(request.method()) {
                let public_url = {
                    let data = state.data.lock().await;
                    data.config.public_url.as_ref().map(|u| u.to_string())
                };
                if let Err(err) = verify_origin(request.headers(), public_url.as_deref()) {
                    return AppError::R3v3rs3(err).into_response();
                }
            }
            caller.client = request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip());
            request.extensions_mut().insert(caller);
            if let Some(token) = session_token {
                request.extensions_mut().insert(AuthSessionToken(token));
            }
            next.run(request).await
        }
        Err(err) => AppError::R3v3rs3(err).into_response(),
    }
}

fn apply_scope_to_caller(mut caller: Caller, scope: &str) -> Caller {
    if !scope.split_whitespace().any(|s| s == "edit") {
        caller.role = Role::Viewer;
    }
    caller
}

async fn bearer_caller(state: &AppState, token: &str) -> Result<Caller, Error> {
    let (username, created_at, scope) = {
        let data = state.data.lock().await;
        if !data.config.mcp.enabled {
            return Err(Error::Unauthorized);
        }
        match data.mcp.verify(token) {
            Some(grant) => (
                grant.username().to_string(),
                grant.created_at(),
                grant.scope.clone(),
            ),
            None => return Err(Error::Unauthorized),
        }
    };
    let mut record = crate::sessions::SessionRecord::new(&username);
    record.started_at = created_at;
    let caller = state
        .accounts
        .borrow()
        .caller(&record)
        .ok_or(Error::Unauthorized)?;
    Ok(apply_scope_to_caller(caller, &scope))
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

pub const MAX_LOGIN_USERNAME_LEN: usize = 128;
pub const MAX_LOGIN_ATTEMPTS_ENTRIES: usize = 10_000;

pub fn bounded_key(key: &LoginAttemptKey) -> LoginAttemptKey {
    if key.1.len() <= MAX_LOGIN_USERNAME_LEN {
        key.clone()
    } else {
        (key.0, key.1[..MAX_LOGIN_USERNAME_LEN].to_string())
    }
}

#[derive(Debug, Clone, Copy)]
struct LoginAttempt {
    failures: u32,
    in_flight: u32,
    window_start: Instant,
    blocked_until: Option<Instant>,
}

/// Counts failed logins per client IP and username.
#[derive(Default)]
pub struct LoginAttempts {
    entries: HashMap<LoginAttemptKey, LoginAttempt>,
}

impl LoginAttempts {
    /// Attempts to acquire an in-flight login attempt slot in O(1) time.
    pub fn acquire(
        &mut self,
        key: &LoginAttemptKey,
        max_attempts: u32,
        reset: Duration,
        now: Instant,
    ) -> bool {
        let key = bounded_key(key);
        let max = max_attempts.max(1);
        let attempt = self.get_or_insert_attempt(key, reset, now);

        if attempt.blocked_until.is_some_and(|until| until > now) {
            return false;
        }
        if attempt.failures.saturating_add(attempt.in_flight) >= max {
            return false;
        }
        attempt.in_flight += 1;
        true
    }

    /// Releases an in-flight attempt slot without recording a failure.
    pub fn release(&mut self, key: &LoginAttemptKey) {
        let key = bounded_key(key);
        if let Some(attempt) = self.entries.get_mut(&key) {
            attempt.in_flight = attempt.in_flight.saturating_sub(1);
        }
    }

    /// Returns true while the key is blocked. O(1) lookup without scanning the whole map.
    pub fn is_blocked(&self, key: &LoginAttemptKey, reset: Duration, now: Instant) -> bool {
        let key = bounded_key(key);
        self.entries.get(&key).is_some_and(|attempt| {
            attempt.is_active(reset, now)
                && attempt.blocked_until.is_some_and(|until| until > now)
        })
    }

    pub fn record_failure(
        &mut self,
        key: LoginAttemptKey,
        max_attempts: u32,
        reset: Duration,
        now: Instant,
    ) {
        let key = bounded_key(&key);
        let attempt = self.get_or_insert_attempt(key, reset, now);
        attempt.in_flight = attempt.in_flight.saturating_sub(1);
        attempt.failures += 1;
        if attempt.failures >= max_attempts.max(1) {
            attempt.blocked_until = Some(now + reset);
        }
    }

    pub fn clear(&mut self, key: &LoginAttemptKey) {
        let key = bounded_key(key);
        self.entries.remove(&key);
    }

    /// Prunes expired attempt entries outside the request path.
    pub fn prune_expired(&mut self, reset: Duration, now: Instant) {
        self.entries.retain(|_, attempt| attempt.is_active(reset, now));
    }

    fn prune_oldest(&mut self) {
        if let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, attempt)| attempt.window_start)
            .map(|(k, _)| k.clone())
        {
            self.entries.remove(&oldest);
        }
    }

    fn get_or_insert_attempt(
        &mut self,
        key: LoginAttemptKey,
        reset: Duration,
        now: Instant,
    ) -> &mut LoginAttempt {
        if self.entries.contains_key(&key) {
            let attempt = self.entries.get_mut(&key).unwrap();
            if !attempt.is_active(reset, now) {
                *attempt = LoginAttempt::new(now);
            }
            return self.entries.get_mut(&key).unwrap();
        }

        if self.entries.len() >= MAX_LOGIN_ATTEMPTS_ENTRIES {
            self.prune_expired(reset, now);
            if self.entries.len() >= MAX_LOGIN_ATTEMPTS_ENTRIES {
                self.prune_oldest();
            }
        }
        self.entries.entry(key).or_insert_with(|| LoginAttempt::new(now))
    }
}

impl LoginAttempt {
    fn new(now: Instant) -> Self {
        Self {
            failures: 0,
            in_flight: 0,
            window_start: now,
            blocked_until: None,
        }
    }

    fn is_active(&self, reset: Duration, now: Instant) -> bool {
        if self.in_flight > 0 {
            return true;
        }
        match self.blocked_until {
            Some(until) => until > now,
            None => now.duration_since(self.window_start) < reset,
        }
    }
}

pub const MAX_SESSION_TOTP_ATTEMPTS: u32 = 3;

#[derive(Default)]
pub struct TotpAttempts {
    sessions: HashMap<String, u32>,
    accounts: HashMap<String, LoginAttempt>,
}

impl TotpAttempts {
    pub fn record_failure(
        &mut self,
        session_token: &str,
        username: &str,
        max_account_attempts: u32,
        reset: Duration,
        now: Instant,
    ) -> bool {
        let session_fails = self.sessions.entry(session_token.to_string()).or_insert(0);
        *session_fails += 1;
        let session_exhausted = *session_fails >= MAX_SESSION_TOTP_ATTEMPTS;
        if session_exhausted {
            self.sessions.remove(session_token);
        }

        let max = max_account_attempts.max(1);
        let attempt = self.accounts.entry(username.to_string()).or_insert(LoginAttempt {
            failures: 0,
            in_flight: 0,
            window_start: now,
            blocked_until: None,
        });
        attempt.failures += 1;
        if attempt.failures >= max {
            attempt.blocked_until = Some(now + reset);
        }

        session_exhausted
    }

    pub fn is_account_blocked(&mut self, username: &str, reset: Duration, now: Instant) -> bool {
        self.accounts.retain(|_, attempt| attempt.is_active(reset, now));
        self.accounts
            .get(username)
            .and_then(|attempt| attempt.blocked_until)
            .is_some_and(|until| until > now)
    }

    pub fn clear(&mut self, session_token: &str, username: &str) {
        self.sessions.remove(session_token);
        self.accounts.remove(username);
    }
}

/// Maximum number of active WebAuthn challenges held in memory.
const MAX_PASSKEY_CHALLENGES: usize = 1024;

/// Active WebAuthn challenges with 2-minute expiration.
#[derive(Default)]
pub struct PasskeyChallenges {
    entries: HashMap<String, Instant>,
}

impl PasskeyChallenges {
    pub fn insert(&mut self, challenge: String, now: Instant) {
        self.entries
            .retain(|_, time| now.duration_since(*time) < Duration::from_secs(120));
        if self.entries.len() >= MAX_PASSKEY_CHALLENGES {
            Self::prune_oldest(&mut self.entries);
        }
        self.entries.insert(challenge, now);
    }

    fn prune_oldest(entries: &mut HashMap<String, Instant>) {
        if let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, time)| *time)
            .map(|(k, _)| k.clone())
        {
            entries.remove(&oldest);
        }
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
    fn in_flight_attempts_block_concurrent_burst_exceeding_threshold() {
        let reset = Duration::from_secs(60);
        let now = Instant::now();
        let mut attempts = LoginAttempts::default();

        assert!(attempts.acquire(&key("admin"), 2, reset, now));
        assert!(attempts.acquire(&key("admin"), 2, reset, now));
        assert!(!attempts.acquire(&key("admin"), 2, reset, now));

        attempts.release(&key("admin"));
        assert!(attempts.acquire(&key("admin"), 2, reset, now));
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

    #[test]
    fn passkey_challenges_cap_and_expiry() {
        let mut challenges = PasskeyChallenges::default();
        let now = Instant::now();
        for i in 0..1030 {
            challenges.insert(format!("ch_{i}"), now + Duration::from_millis(i as u64));
        }
        assert!(challenges.entries.len() <= MAX_PASSKEY_CHALLENGES);
        assert!(!challenges.consume("ch_0", now));
        assert!(challenges.consume("ch_1029", now + Duration::from_millis(1029)));

        let expired = now + Duration::from_secs(125);
        assert!(!challenges.consume("ch_1028", expired));
    }

    #[test]
    fn bearer_scope_downgrades_to_viewer_when_read_only() {
        let caller = Caller {
            username: "admin".to_string(),
            role: Role::Admin,
            proxies: None,
            client: None,
        };
        let read_caller = apply_scope_to_caller(caller.clone(), "read");
        assert_eq!(read_caller.role, Role::Viewer);

        let edit_caller = apply_scope_to_caller(caller.clone(), "read edit");
        assert_eq!(edit_caller.role, Role::Admin);

        let edit_only_caller = apply_scope_to_caller(caller, "edit");
        assert_eq!(edit_only_caller.role, Role::Admin);
    }

    #[test]
    fn verify_origin_enforces_port_and_sec_fetch_site() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(axum::http::header::HOST, "localhost:46492".parse().unwrap());
        headers.insert(
            axum::http::header::COOKIE,
            format!("{ADMIN_SESSION_COOKIE}=xyz").parse().unwrap(),
        );

        // Cross-origin with different port -> rejected
        headers.insert(axum::http::header::ORIGIN, "http://localhost:3000".parse().unwrap());
        assert!(verify_origin(&headers, None).is_err());

        // Same-origin with exact port -> allowed
        headers.insert(axum::http::header::ORIGIN, "http://localhost:46492".parse().unwrap());
        assert!(verify_origin(&headers, None).is_ok());

        // Sec-fetch-site same-site -> rejected
        headers.insert("sec-fetch-site", "same-site".parse().unwrap());
        assert!(verify_origin(&headers, None).is_err());

        // Sec-fetch-site same-origin -> allowed
        headers.insert("sec-fetch-site", "same-origin".parse().unwrap());
        assert!(verify_origin(&headers, None).is_ok());
    }

    #[test]
    fn totp_attempts_limit_and_account_blocking() {
        let mut totp = TotpAttempts::default();
        let reset = Duration::from_secs(60);
        let now = Instant::now();

        // 1st and 2nd failed attempt on session_1 do not exhaust it
        assert!(!totp.record_failure("token_1", "admin", 5, reset, now));
        assert!(!totp.record_failure("token_1", "admin", 5, reset, now));
        assert!(!totp.is_account_blocked("admin", reset, now));

        // 3rd failed attempt on session_1 exhausts the session
        assert!(totp.record_failure("token_1", "admin", 5, reset, now));

        // Another session for the same user fails 2 more times (total 5 for user)
        assert!(!totp.record_failure("token_2", "admin", 5, reset, now));
        assert!(!totp.record_failure("token_2", "admin", 5, reset, now));

        // Now user is blocked across all sessions and source IPs
        assert!(totp.is_account_blocked("admin", reset, now));

        // Clear resets both
        totp.clear("token_2", "admin");
        assert!(!totp.is_account_blocked("admin", reset, now));
    }

    #[test]
    fn is_secure_request_derives_security_from_network_and_headers() {
        let trusted_proxies = vec!["127.0.0.1/32".parse().unwrap()];
        let loopback: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let remote: SocketAddr = "198.51.100.1:12345".parse().unwrap();

        // 1. Loopback without forwarded header -> false
        let headers = axum::http::HeaderMap::new();
        assert!(!is_secure_request(&trusted_proxies, &loopback, &headers));

        // 2. Loopback with trusted proto http -> false
        let mut headers_http = axum::http::HeaderMap::new();
        headers_http.insert("x-forwarded-proto", "http".parse().unwrap());
        assert!(!is_secure_request(&trusted_proxies, &loopback, &headers_http));

        // 3. Loopback with trusted proto https -> true
        let mut headers_https = axum::http::HeaderMap::new();
        headers_https.insert("x-forwarded-proto", "https".parse().unwrap());
        assert!(is_secure_request(&trusted_proxies, &loopback, &headers_https));

        // 4. Remote connection -> always true
        assert!(is_secure_request(&trusted_proxies, &remote, &headers));

        // 5. Untrusted client sending x-forwarded-proto is ignored
        let empty_trusted: Vec<ipnet::IpNet> = Vec::new();
        assert!(!is_secure_request(&empty_trusted, &loopback, &headers_https));
    }

    #[tokio::test]
    async fn sanitize_audit_username_prevents_password_and_invalid_leakage() {
        let temp_dir = std::env::temp_dir().join(format!("r3-auth-test-{}", r3v3rs3_api::id::ShortId::new()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let _ = crate::log::open_database(&temp_dir.join("log.db")).await.unwrap();
        let app_info = crate::config::new_appinfo(&temp_dir, &temp_dir);
        let data = Arc::new(tokio::sync::Mutex::new(crate::admin::Data::new(app_info).await.unwrap()));
        let (sender, _) = tokio::sync::mpsc::channel(1);
        let (event, _) = tokio::sync::broadcast::channel(1);
        let mut accounts_map = HashMap::new();
        accounts_map.insert("admin".to_string(), r3v3rs3_api::auth::Account::default());
        let directory = Arc::new(crate::accounts::AccountDirectory::new(&accounts_map));
        let (_, accounts) = tokio::sync::watch::channel(directory);
        let (upgrade, _) = tokio::sync::mpsc::channel(1);
        let state = AppState {
            sender,
            event,
            event_listener_counter: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            accounts,
            upgrade,
            upgrade_stage: Arc::new(std::sync::Mutex::new(None)),
            data,
        };

        // 1. Existing account is retained
        assert_eq!(sanitize_audit_username(&state, "admin"), "admin");

        // 2. Unknown account (valid format, but not in directory) is redacted
        assert_eq!(sanitize_audit_username(&state, "secretpass123"), "[unknown-account]");

        // 3. Invalid username format (contains invalid characters or spaces) is redacted
        assert_eq!(sanitize_audit_username(&state, "secret:password"), "[invalid-username]");
        assert_eq!(sanitize_audit_username(&state, "secret password"), "[invalid-username]");
        assert_eq!(sanitize_audit_username(&state, ""), "[invalid-username]");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
