use super::openapi::ErrorResponses;
use super::{AppError, AppState};
use crate::accounts::Caller;
use crate::audit::AuditRecord;
use crate::server::rpc::auth::VerifyAccount;
use crate::sessions::{self, SessionBackend, SessionRecord, SessionScope};
use axum::{
    Extension, Json,
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, SameSite},
};
use r3v3rs3_api::{
    audit::AuditAction,
    auth::{LoginMethod, LoginRequest, LoginResponse, SessionInfo},
    error::{Error, ErrorMessage},
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::warn;

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
    jar: CookieJar,
    Json(request): Json<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    let username = request.username.clone();
    let attempt_key = (peer.ip(), username.clone());
    ensure_login_allowed(&state, &attempt_key).await?;

    let token = jar.get("token").map(|c| c.value().to_string());
    if let LoginMethod::Totp { .. } = &request.method
        && !verify_login_session(&state, &token.unwrap_or_default(), &username).await
    {
        record_login_failure(&state, attempt_key).await;
        return Err(Error::InvalidLoginCredentials.into());
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
            state.data.lock().await.login_attempts.clear(&attempt_key);
            let record = AuditRecord::new(AuditAction::Login);
            state.record_audit(&username, Some(peer.ip()), record).await;
            SessionScope::Admin
        }
        _ => SessionScope::Login,
    };

    let (backend, expiry) = session_backend(&state).await;
    let token = backend
        .create(scope, SessionRecord::new(&username), expiry)
        .await?;

    let cookie = Cookie::build(("token", token))
        .http_only(true)
        .same_site(SameSite::Strict)
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
    let reset = data.config.admin.login_attempts_reset;
    if data.login_attempts.is_blocked(key, reset, Instant::now()) {
        return Err(Error::TooManyLoginAttempts);
    }
    Ok(())
}

/// Counts a failed sign-in and records it in the audit log.
async fn record_login_failure(state: &AppState, key: LoginAttemptKey) {
    let record = AuditRecord::new(AuditAction::LoginFailed);
    state.record_audit(&key.1, Some(key.0), record).await;
    let mut data = state.data.lock().await;
    let admin = data.config.admin;
    data.login_attempts.record_failure(
        key,
        admin.max_login_attempts,
        admin.login_attempts_reset,
        Instant::now(),
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
    jar.remove("token")
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
    })
}

/// Accepts a request with an active admin session and adds its `Caller` to the request.
pub async fn verify(
    State(state): State<AppState>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(token) = jar.get("token") else {
        return AppError::R3v3rs3(Error::Unauthorized).into_response();
    };
    match session_caller(&state, token.value()).await {
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

/// The account of an active admin session. The account must exist, and its last change must not
/// be later than the start of the session.
async fn session_caller(state: &AppState, token: &str) -> Result<Caller, Error> {
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
}
