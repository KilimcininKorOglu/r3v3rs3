use super::super::cookie::{cookie_values, remove_cookie};
use super::super::page::PagePreferences;
use super::{AuthContext, AuthRejection};
use crate::admin::auth::{LoginAttemptKey, LoginAttempts};
use crate::config::storage::Storage;
use crate::sessions::{self, SessionBackend, SessionScope};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::Body,
    header::{
        HeaderMap, HeaderValue, ALLOW, CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
        LOCATION, REFERRER_POLICY, SET_COOKIE,
    },
    Method, Request, Response, StatusCode,
};
use r3v3rs3_api::{
    app::AdminConfig,
    auth::{LoginMethod, LoginRequest, LoginResponse},
    error::Error,
};
use sailfish::TemplateOnce;
use std::{
    fmt,
    ops::ControlFlow,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};
use tracing::{error, warn};
use url::form_urlencoded;

/// Name of the session cookie. The cookie has no `Domain` attribute, so the browser sends it
/// only to the host that set it.
const COOKIE_NAME: &str = "r3v3rs3_session";

/// Path segments below the route path that lead to the sign-in endpoints.
const ENDPOINT_PREFIX: [&str; 2] = [".r3v3rs3", "auth"];

const MAX_FORM_SIZE: usize = 8 * 1024;
const MAX_REDIRECT_LENGTH: usize = 2048;

const CONTENT_SECURITY: &str =
    "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'";
const INVALID_CREDENTIALS: &str = "login.invalid_credentials";
const TOTP_REQUIRED: &str = "login.totp_required";
const TOO_MANY_ATTEMPTS: &str = "login.too_many_attempts";

/// Signs clients in with the panel accounts and keeps their sessions. The server keeps one
/// instance for all proxies, so the sessions survive a proxy reload.
pub struct SessionService {
    storage: Arc<dyn Storage>,
    backend: Arc<dyn SessionBackend>,
    state: Mutex<SessionState>,
}

struct SessionState {
    config: AdminConfig,
    attempts: LoginAttempts,
}

enum Verification {
    Success,
    TotpRequired,
    Failed,
}

impl fmt::Debug for SessionService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionService").finish_non_exhaustive()
    }
}

impl SessionState {
    fn expiry(&self) -> Duration {
        sessions::expiry(&self.config)
    }
}

impl SessionService {
    pub fn new(
        storage: Arc<dyn Storage>,
        backend: Arc<dyn SessionBackend>,
        config: AdminConfig,
    ) -> Self {
        Self {
            storage,
            backend,
            state: Mutex::new(SessionState {
                config,
                attempts: LoginAttempts::default(),
            }),
        }
    }

    pub fn set_config(&self, config: AdminConfig) {
        self.state().config = config;
    }

    fn state(&self) -> MutexGuard<'_, SessionState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Returns true when the token belongs to an unexpired session of the host.
    async fn is_valid(&self, token: &str, host: &str) -> bool {
        let expiry = self.state().expiry();
        match self.backend.get(SessionScope::Proxy, token).await {
            Ok(record) => record.is_some_and(|record| {
                record.is_active(expiry) && record.subject.eq_ignore_ascii_case(host)
            }),
            Err(err) => {
                warn!(%err, "failed to read the session");
                false
            }
        }
    }

    /// Starts a session for the host and returns its token and lifetime.
    async fn create_session(&self, host: &str) -> Result<(String, Duration), Error> {
        let expiry = self.state().expiry();
        let token = self
            .backend
            .create(SessionScope::Proxy, host, expiry)
            .await?;
        Ok((token, expiry))
    }

    async fn remove_session(&self, token: &str) {
        if let Err(err) = self.backend.remove(SessionScope::Proxy, token).await {
            warn!(%err, "failed to end the session");
        }
    }

    fn is_blocked(&self, key: &LoginAttemptKey) -> bool {
        let mut state = self.state();
        let reset = state.config.login_attempts_reset;
        state.attempts.is_blocked(key, reset, Instant::now())
    }

    fn record_failure(&self, key: LoginAttemptKey) {
        let mut state = self.state();
        let config = state.config;
        state.attempts.record_failure(
            key,
            config.max_login_attempts,
            config.login_attempts_reset,
            Instant::now(),
        );
    }

    fn clear_failures(&self, key: &LoginAttemptKey) {
        self.state().attempts.clear(key);
    }

    /// Checks the password, and the TOTP code when the account requires one.
    async fn verify(&self, form: &LoginForm) -> Verification {
        let password = LoginMethod::Password {
            password: form.password.clone(),
        };
        match self.verify_account(&form.username, password).await {
            Ok(LoginResponse::Success) => Verification::Success,
            Ok(LoginResponse::TotpRequired) if form.totp.is_empty() => Verification::TotpRequired,
            Ok(LoginResponse::TotpRequired) => {
                let totp = LoginMethod::Totp {
                    token: form.totp.clone(),
                };
                match self.verify_account(&form.username, totp).await {
                    Ok(LoginResponse::Success) => Verification::Success,
                    _ => Verification::Failed,
                }
            }
            Err(_) => Verification::Failed,
        }
    }

    /// Verifies the credential on a blocking thread, because the password hash check takes CPU
    /// time.
    async fn verify_account(
        &self,
        username: &str,
        method: LoginMethod,
    ) -> Result<LoginResponse, Error> {
        let storage = self.storage.clone();
        let request = LoginRequest {
            username: username.to_string(),
            method,
            insecure: false,
        };
        let runtime = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || runtime.block_on(storage.verify_account(request)))
            .await
            .map_err(|err| {
                error!(%err, "failed to verify the account");
                Error::InvalidLoginCredentials
            })?
    }
}

/// Requires a session that a client gets by signing in with a panel account.
#[derive(Debug)]
pub struct SessionAuthenticator {
    service: Arc<SessionService>,
}

enum Action {
    Page,
    Login,
    Logout,
    NotAllowed(&'static str),
    NotFound,
}

impl SessionAuthenticator {
    pub fn new(service: Arc<SessionService>) -> Self {
        Self { service }
    }

    /// Serves the sign-in endpoints below the route path. Other requests continue.
    pub async fn serve<B>(
        &self,
        req: Request<B>,
        ctx: &AuthContext<'_>,
    ) -> ControlFlow<Response<Full<Bytes>>, Request<B>>
    where
        B: Body<Data = Bytes>,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let Some(endpoint) = endpoint(ctx.path_segments) else {
            return ControlFlow::Continue(req);
        };
        let requested = action(endpoint, req.method());
        let response = match requested {
            Action::Page => login_page(
                LoginTemplate {
                    username: "",
                    redirect: &query_redirect(&req),
                    error: None,
                    totp: false,
                    preferences: ctx.preferences,
                },
                StatusCode::OK,
            ),
            Action::Login => self.login(req, ctx).await,
            Action::Logout => self.logout(req.headers(), ctx).await,
            Action::NotAllowed(allow) => method_not_allowed(allow),
            Action::NotFound => text_response(StatusCode::NOT_FOUND, "Not Found"),
        };
        ControlFlow::Break(response)
    }

    /// Lets a request with a valid session through and removes the session cookie from it.
    pub async fn authorize<B>(
        &self,
        req: &mut Request<B>,
        ctx: &AuthContext<'_>,
    ) -> Result<(), AuthRejection> {
        let host = ctx.host.unwrap_or_default();
        let signed_in = match session_token(req.headers()) {
            Some(token) => self.service.is_valid(token, host).await,
            None => false,
        };
        if !signed_in {
            return Err(AuthRejection::Response(Box::new(sign_in_required(
                req, ctx,
            ))));
        }
        remove_session_cookie(req.headers_mut());
        Ok(())
    }

    async fn login<B>(&self, req: Request<B>, ctx: &AuthContext<'_>) -> Response<Full<Bytes>>
    where
        B: Body<Data = Bytes>,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let Some(form) = read_form(req.into_body()).await else {
            return text_response(StatusCode::BAD_REQUEST, "Bad Request");
        };
        let page = |error_key, totp, status| {
            let template = LoginTemplate {
                username: &form.username,
                redirect: &form.redirect,
                error: Some(ctx.preferences.locale.t(error_key)),
                totp,
                preferences: ctx.preferences,
            };
            login_page(template, status)
        };
        let key = (ctx.client, form.username.clone());
        if self.service.is_blocked(&key) {
            warn!(client = %ctx.client, username = %form.username, "sign-in blocked after too many failed attempts");
            return page(TOO_MANY_ATTEMPTS, false, StatusCode::TOO_MANY_REQUESTS);
        }
        match self.service.verify(&form).await {
            Verification::Success => {
                self.service.clear_failures(&key);
                let host = ctx.host.unwrap_or_default();
                let (token, expiry) = match self.service.create_session(host).await {
                    Ok(session) => session,
                    Err(err) => {
                        error!(%err, "failed to start the session");
                        return text_response(
                            StatusCode::SERVICE_UNAVAILABLE,
                            "Service Unavailable",
                        );
                    }
                };
                let cookie = format!(
                    "{COOKIE_NAME}={token}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax{}",
                    expiry.as_secs(),
                    secure_attribute(ctx)
                );
                redirect_response(StatusCode::SEE_OTHER, &form.redirect, Some(&cookie))
            }
            Verification::TotpRequired => page(TOTP_REQUIRED, true, StatusCode::UNAUTHORIZED),
            Verification::Failed => {
                warn!(client = %ctx.client, username = %form.username, "sign-in failed");
                self.service.record_failure(key);
                page(INVALID_CREDENTIALS, false, StatusCode::UNAUTHORIZED)
            }
        }
    }

    async fn logout(&self, headers: &HeaderMap, ctx: &AuthContext<'_>) -> Response<Full<Bytes>> {
        if let Some(token) = session_token(headers) {
            self.service.remove_session(token).await;
        }
        let cookie = format!(
            "{COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{}",
            secure_attribute(ctx)
        );
        redirect_response(StatusCode::SEE_OTHER, &login_path(ctx), Some(&cookie))
    }
}

/// Returns the name of the sign-in endpoint when the path below the route leads to one.
fn endpoint(segments: &[String]) -> Option<&str> {
    match segments {
        [first, second, name]
            if first.as_str() == ENDPOINT_PREFIX[0] && second.as_str() == ENDPOINT_PREFIX[1] =>
        {
            Some(name.as_str())
        }
        _ => None,
    }
}

fn action(endpoint: &str, method: &Method) -> Action {
    let read = *method == Method::GET || *method == Method::HEAD;
    match (endpoint, read, *method == Method::POST) {
        ("login", true, _) => Action::Page,
        ("login", _, true) => Action::Login,
        ("logout", _, true) => Action::Logout,
        ("login", _, _) => Action::NotAllowed("GET, HEAD, POST"),
        ("logout", _, _) => Action::NotAllowed("POST"),
        _ => Action::NotFound,
    }
}

/// Sends a browser to the sign-in page. Other clients receive `401 Unauthorized`.
fn sign_in_required<B>(req: &Request<B>, ctx: &AuthContext<'_>) -> Response<Full<Bytes>> {
    if ![Method::GET, Method::HEAD].contains(req.method()) {
        return text_response(StatusCode::UNAUTHORIZED, "Unauthorized");
    }
    let target = req
        .uri()
        .path_and_query()
        .map_or("/", |target| target.as_str());
    let redirect: String = form_urlencoded::byte_serialize(target.as_bytes()).collect();
    let location = format!("{}?redirect={redirect}", login_path(ctx));
    redirect_response(StatusCode::FOUND, &location, None)
}

fn login_path(ctx: &AuthContext<'_>) -> String {
    format!(
        "{}/{}/{}/login",
        ctx.base_path, ENDPOINT_PREFIX[0], ENDPOINT_PREFIX[1]
    )
}

fn secure_attribute(ctx: &AuthContext<'_>) -> &'static str {
    if ctx.proto == "http" {
        ""
    } else {
        "; Secure"
    }
}

fn query_redirect<B>(req: &Request<B>) -> String {
    let query = req.uri().query().unwrap_or_default();
    form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == "redirect")
        .map_or_else(
            || "/".to_string(),
            |(_, value)| safe_redirect(&value).to_string(),
        )
}

/// Returns the target when it is a path on the same host, and `/` otherwise. This prevents an
/// open redirect to another site.
fn safe_redirect(target: &str) -> &str {
    let valid = target.starts_with('/')
        && !target.starts_with("//")
        && !target.starts_with("/\\")
        && target.len() <= MAX_REDIRECT_LENGTH
        && target.bytes().all(|b| b.is_ascii_graphic());
    if valid {
        target
    } else {
        "/"
    }
}

#[derive(Default)]
struct LoginForm {
    username: String,
    password: String,
    totp: String,
    redirect: String,
}

impl LoginForm {
    fn parse(body: &[u8]) -> Self {
        let mut form = Self::default();
        for (name, value) in form_urlencoded::parse(body) {
            let field = match name.as_ref() {
                "username" => &mut form.username,
                "password" => &mut form.password,
                "totp" => &mut form.totp,
                "redirect" => &mut form.redirect,
                _ => continue,
            };
            *field = value.into_owned();
        }
        Self {
            username: form.username.trim().to_string(),
            totp: form.totp.trim().to_string(),
            redirect: safe_redirect(&form.redirect).to_string(),
            password: form.password,
        }
    }
}

async fn read_form<B>(body: B) -> Option<LoginForm>
where
    B: Body<Data = Bytes>,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    match Limited::new(body, MAX_FORM_SIZE).collect().await {
        Ok(body) => Some(LoginForm::parse(&body.to_bytes())),
        Err(err) => {
            warn!(%err, "failed to read the sign-in form");
            None
        }
    }
}

fn session_token(headers: &HeaderMap) -> Option<&str> {
    cookie_values(headers, COOKIE_NAME).next()
}

/// Removes the session cookie, so the upstream server never receives the session token.
fn remove_session_cookie(headers: &mut HeaderMap) {
    remove_cookie(headers, COOKIE_NAME);
}

#[derive(TemplateOnce)]
#[template(path = "login.stpl")]
struct LoginTemplate<'a> {
    username: &'a str,
    redirect: &'a str,
    error: Option<&'a str>,
    totp: bool,
    preferences: PagePreferences,
}

fn login_page(template: LoginTemplate<'_>, status: StatusCode) -> Response<Full<Bytes>> {
    let html = match template.render_once() {
        Ok(html) => html,
        Err(err) => {
            error!(%err, "failed to render the sign-in page");
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error");
        }
    };
    let mut res = response(status, Bytes::from(html), "text/html; charset=utf-8");
    let headers = res.headers_mut();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY),
    );
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    res
}

fn text_response(status: StatusCode, text: &'static str) -> Response<Full<Bytes>> {
    response(
        status,
        Bytes::from_static(text.as_bytes()),
        "text/plain; charset=utf-8",
    )
}

fn response(status: StatusCode, body: Bytes, content_type: &'static str) -> Response<Full<Bytes>> {
    let mut res = Response::new(Full::new(body));
    *res.status_mut() = status;
    let headers = res.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn method_not_allowed(allow: &'static str) -> Response<Full<Bytes>> {
    let mut res = text_response(StatusCode::METHOD_NOT_ALLOWED, "Method Not Allowed");
    res.headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    res
}

/// Builds a redirect. The location and the cookie contain only visible ASCII characters, so a
/// header error is a bug and results in `500 Internal Server Error`.
fn redirect_response(
    status: StatusCode,
    location: &str,
    cookie: Option<&str>,
) -> Response<Full<Bytes>> {
    let mut res = text_response(status, "");
    for (name, value) in [(LOCATION, Some(location)), (SET_COOKIE, cookie)] {
        let Some(value) = value else {
            continue;
        };
        match HeaderValue::from_str(value) {
            Ok(value) => {
                res.headers_mut().insert(name, value);
            }
            Err(err) => {
                error!(%err, %name, "invalid sign-in response header");
                return text_response(StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error");
            }
        }
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_redirect_keeps_same_host_paths() {
        assert_eq!(safe_redirect("/private?page=1"), "/private?page=1");
        for target in [
            "",
            "private",
            "//evil.example/",
            "/\\evil.example",
            "https://evil.example/",
            "/a b",
            "/\u{e9}",
        ] {
            assert_eq!(safe_redirect(target), "/", "{target}");
        }
    }

    #[test]
    fn login_form_is_parsed_and_trimmed() {
        let form = LoginForm::parse(
            b"username=+admin+&password=p%40ss+word&totp=+123456+&redirect=%2F%2Fevil.example&other=1",
        );
        assert_eq!(form.username, "admin");
        assert_eq!(form.password, "p@ss word");
        assert_eq!(form.totp, "123456");
        assert_eq!(form.redirect, "/");
    }

    #[test]
    fn endpoint_requires_the_prefix() {
        let segments = |path: &str| path.split('/').map(str::to_string).collect::<Vec<_>>();
        assert_eq!(endpoint(&segments(".r3v3rs3/auth/login")), Some("login"));
        assert_eq!(endpoint(&segments(".r3v3rs3/auth/login/extra")), None);
        assert_eq!(endpoint(&segments("auth/login/x")), None);
    }

    #[test]
    fn endpoint_methods() {
        assert!(matches!(action("login", &Method::HEAD), Action::Page));
        assert!(matches!(action("login", &Method::POST), Action::Login));
        assert!(matches!(action("logout", &Method::POST), Action::Logout));
        assert!(matches!(
            action("logout", &Method::GET),
            Action::NotAllowed("POST")
        ));
        assert!(matches!(action("other", &Method::GET), Action::NotFound));
    }
}
