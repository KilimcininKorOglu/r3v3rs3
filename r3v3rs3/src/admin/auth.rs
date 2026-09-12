use super::{AppError, AppState};
use crate::server::rpc::auth::VerifyAccount;
use axum::{
    extract::{ConnectInfo, Request, State},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use axum_extra::extract::{
    cookie::{Cookie, SameSite},
    CookieJar,
};
use r3v3rs3_api::{
    auth::{LoginMethod, LoginRequest, LoginResponse},
    error::Error,
};
use rand::distributions::{Alphanumeric, DistString};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant},
};

const MINIMUM_SESSION_EXPIRY: Duration = Duration::from_secs(60 * 5); // 5 minutes
const SESSION_TOKEN_LENGTH: usize = 32;

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
    if let LoginMethod::Totp { .. } = &request.method {
        if !verify_login_session(&state, &token.unwrap_or_default(), &username).await {
            record_login_failure(&state, attempt_key).await;
            return Err(Error::InvalidLoginCredentials.into());
        }
    }

    let insecure = request.insecure;
    let result = match state.call(VerifyAccount { request }).await {
        Ok(result) => result,
        Err(err) => {
            record_login_failure(&state, attempt_key).await;
            return Err(err.into());
        }
    };

    let session = match *result {
        LoginResponse::Success => {
            state.data.lock().await.login_attempts.clear(&attempt_key);
            SessionKind::Admin
        }
        _ => SessionKind::Login,
    };

    let token = state
        .data
        .lock()
        .await
        .sessions
        .new_token(session, &username);

    let cookie = Cookie::build(("token", token))
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(!insecure)
        .build();

    Ok((jar.add(cookie), Json(result)))
}

async fn ensure_login_allowed(state: &AppState, key: &LoginAttemptKey) -> Result<(), Error> {
    let mut data = state.data.lock().await;
    let reset = data.config.admin.login_attempts_reset;
    if data.login_attempts.is_blocked(key, reset, Instant::now()) {
        return Err(Error::TooManyLoginAttempts);
    }
    Ok(())
}

async fn record_login_failure(state: &AppState, key: LoginAttemptKey) {
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
    let mut data = state.data.lock().await;
    let expiry = data.config.admin.session_expiry;
    data.sessions
        .verify(SessionKind::Login, token, expiry)
        .is_some_and(|session| session.username == username)
}

pub async fn logout(State(state): State<AppState>, jar: CookieJar) -> impl IntoResponse {
    if let Some(token) = jar.get("token") {
        state.data.lock().await.sessions.remove(token.value());
    }
    jar.remove("token")
}

pub async fn verify(
    State(state): State<AppState>,
    jar: CookieJar,
    request: Request,
    next: Next,
) -> Response {
    if let Some(token) = jar.get("token") {
        let mut data = state.data.lock().await;
        let expiry = data.config.admin.session_expiry;
        if data
            .sessions
            .verify(SessionKind::Admin, token.value(), expiry)
            .is_some()
        {
            std::mem::drop(data);
            let response = next.run(request).await;
            return response;
        }
    }
    AppError::R3v3rs3(Error::Unauthorized).into_response()
}

#[derive(Debug, Clone)]
pub struct Session {
    pub kind: SessionKind,
    pub username: String,
    pub started_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Login,
    Admin,
}

#[derive(Default)]
pub struct SessionStore {
    tokens: HashMap<String, Session>,
}

impl SessionStore {
    pub fn new_token(&mut self, kind: SessionKind, username: &str) -> String {
        let token = Alphanumeric.sample_string(&mut rand::thread_rng(), SESSION_TOKEN_LENGTH);
        self.tokens.insert(
            token.clone(),
            Session {
                kind,
                username: username.to_string(),
                started_at: Instant::now(),
            },
        );
        token
    }

    pub fn verify(&mut self, kind: SessionKind, token: &str, expiry: Duration) -> Option<&Session> {
        let expiry = expiry.max(MINIMUM_SESSION_EXPIRY);
        self.tokens = self
            .tokens
            .drain()
            .filter(|(_, t)| t.started_at.elapsed() < expiry)
            .collect();
        self.tokens
            .get(token)
            .filter(|session| session.kind == kind)
    }

    pub fn remove(&mut self, token: &str) {
        self.tokens.remove(token);
    }
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
