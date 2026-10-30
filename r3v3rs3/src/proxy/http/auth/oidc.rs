use super::super::cookie::{cookie_values, remove_cookie};
use super::session::{ENDPOINT_PREFIX, safe_redirect};
use super::{AuthContext, AuthRejection};
use crate::cdn::fetch::HttpClient;
use crate::proxy::http::hyper_tls::client::HttpsConnector;
use base64::Engine;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode,
    body::Body,
    header::{CACHE_CONTROL, CONTENT_TYPE, HeaderName, HeaderValue, LOCATION, PRAGMA, SET_COOKIE},
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use r3v3rs3_api::policy::{OidcAuth, default_oidc_scopes, is_email_allowed};
use rand::distr::{Alphanumeric, SampleString};
use std::sync::Arc;
use std::time::Duration;
use tokio_rustls::rustls::ClientConfig;
use tracing::error;
use url::form_urlencoded;

use super::session::SessionService;

const COOKIE_NAME: &str = "r3v3rs3_oidc";
const STATE_COOKIE_NAME: &str = "r3v3rs3_oidc_state";
const MAX_BODY_SIZE: usize = 1024 * 1024;
const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub struct OidcAuthenticator {
    config: OidcAuth,
    client: HttpClient,
    sessions: Arc<SessionService>,
}

enum OidcAction {
    Login,
    Callback,
    Logout,
}

impl OidcAuthenticator {
    pub fn new(
        config: OidcAuth,
        tls_client_config: Arc<ClientConfig>,
        sessions: Arc<SessionService>,
    ) -> Self {
        let client =
            Client::builder(TokioExecutor::new()).build(HttpsConnector::new(tls_client_config));
        Self {
            config,
            client,
            sessions,
        }
    }

    pub async fn serve<B>(
        &self,
        req: Request<B>,
        ctx: &AuthContext<'_>,
    ) -> std::ops::ControlFlow<Response<Full<Bytes>>, Request<B>>
    where
        B: Body<Data = Bytes>,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let Some(action) = oidc_action(ctx.path_segments) else {
            return std::ops::ControlFlow::Continue(req);
        };
        let response = match action {
            OidcAction::Login => self.handle_login(&req, ctx),
            OidcAction::Callback => self.handle_callback(req, ctx).await,
            OidcAction::Logout => self.handle_logout(req.headers(), ctx).await,
        };
        std::ops::ControlFlow::Break(response)
    }

    pub async fn authorize<B>(
        &self,
        req: &mut Request<B>,
        ctx: &AuthContext<'_>,
    ) -> Result<(), AuthRejection> {
        let host = ctx.host.unwrap_or_default();
        let token = oidc_token(req.headers());
        let user = match token {
            Some(t) => self.sessions.get_proxy_session(&t, host).await,
            None => None,
        };
        let Some(user) = user else {
            return Err(AuthRejection::Response(Box::new(sign_in_required(
                req, ctx,
            ))));
        };
        if !is_email_allowed(&user, &self.config.allowed_emails) {
            return Err(AuthRejection::Response(Box::new(forbidden_page(&user))));
        }
        remove_oidc_cookie(req.headers_mut());
        inject_user_headers(req.headers_mut(), &user);
        Ok(())
    }

    fn handle_login<B>(&self, req: &Request<B>, ctx: &AuthContext<'_>) -> Response<Full<Bytes>> {
        let target = query_redirect(req);
        let state = new_random_string(16);
        let auth_url = self.build_authorization_url(ctx, &state);
        let state_val = format!("{state}:{target}");
        let secure = secure_attr(ctx);
        let cookie = format!(
            "{STATE_COOKIE_NAME}={state_val}; Path=/; Max-Age=300; HttpOnly; SameSite=Lax{secure}"
        );
        redirect_response(StatusCode::FOUND, &auth_url, Some(&cookie))
    }

    async fn handle_callback<B>(
        &self,
        req: Request<B>,
        ctx: &AuthContext<'_>,
    ) -> Response<Full<Bytes>>
    where
        B: Body<Data = Bytes>,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let query = req.uri().query().unwrap_or_default();
        let Some((code, state)) = parse_callback_params(query) else {
            return text_response(StatusCode::BAD_REQUEST, "Missing code or state parameter\n");
        };
        let Some((expected_state, target)) = read_state_cookie(req.headers()) else {
            return text_response(
                StatusCode::BAD_REQUEST,
                "Missing or expired authentication state\n",
            );
        };
        if state != expected_state {
            return text_response(StatusCode::BAD_REQUEST, "Invalid OIDC state\n");
        }
        let user = match self.exchange_code_for_user(&code, ctx).await {
            Ok(user) => user,
            Err(err) => {
                error!(%err, "OIDC token exchange failed");
                return text_response(StatusCode::BAD_GATEWAY, "OIDC authentication failed\n");
            }
        };
        if !is_email_allowed(&user, &self.config.allowed_emails) {
            return forbidden_page(&user);
        }
        self.create_session_and_redirect(ctx, &user, &target).await
    }

    async fn handle_logout(
        &self,
        headers: &HeaderMap,
        ctx: &AuthContext<'_>,
    ) -> Response<Full<Bytes>> {
        if let Some(token) = oidc_token(headers) {
            self.sessions.remove_proxy_session(&token).await;
        }
        let secure = secure_attr(ctx);
        let cookie = format!("{COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{secure}");
        redirect_response(StatusCode::FOUND, "/", Some(&cookie))
    }

    fn build_authorization_url(&self, ctx: &AuthContext<'_>, state: &str) -> String {
        let auth_endpoint = resolve_auth_endpoint(&self.config);
        let redirect_uri = callback_uri(ctx);
        let scopes = if self.config.scopes.is_empty() {
            default_oidc_scopes()
        } else {
            self.config.scopes.clone()
        };
        let scope_str = scopes.join(" ");

        let mut pairs = form_urlencoded::Serializer::new(String::new());
        pairs.append_pair("response_type", "code");
        pairs.append_pair("client_id", &self.config.client_id);
        pairs.append_pair("redirect_uri", &redirect_uri);
        pairs.append_pair("scope", &scope_str);
        pairs.append_pair("state", state);
        let query = pairs.finish();

        if auth_endpoint.contains('?') {
            format!("{auth_endpoint}&{query}")
        } else {
            format!("{auth_endpoint}?{query}")
        }
    }

    async fn exchange_code_for_user(
        &self,
        code: &str,
        ctx: &AuthContext<'_>,
    ) -> Result<String, String> {
        let token_endpoint = resolve_token_endpoint(&self.config);
        let redirect_uri = callback_uri(ctx);
        let form_body = form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "authorization_code")
            .append_pair("code", code)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("client_id", &self.config.client_id)
            .append_pair("client_secret", &self.config.client_secret)
            .finish();

        let req = Request::builder()
            .method(Method::POST)
            .uri(&token_endpoint)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(Full::new(Bytes::from(form_body)))
            .map_err(|e| e.to_string())?;

        let token_json = self.execute_request(req).await?;
        if let Some(id_token) = token_json.get("id_token").and_then(|v| v.as_str())
            && let Some(user) = decode_jwt_user(id_token)
        {
            return Ok(user);
        }
        if let Some(user) = extract_user_from_json(&token_json) {
            return Ok(user);
        }
        if let Some(access_token) = token_json.get("access_token").and_then(|v| v.as_str()) {
            return self.fetch_userinfo(access_token).await;
        }
        Err("No user identity found in OIDC token response".to_string())
    }

    async fn fetch_userinfo(&self, access_token: &str) -> Result<String, String> {
        let endpoint = resolve_userinfo_endpoint(&self.config);
        if endpoint.is_empty() {
            return Err("No userinfo endpoint available".to_string());
        }
        let req = Request::builder()
            .method(Method::GET)
            .uri(&endpoint)
            .header("authorization", format!("Bearer {access_token}"))
            .header("accept", "application/json")
            .body(Full::new(Bytes::new()))
            .map_err(|e| e.to_string())?;

        let userinfo_json = self.execute_request(req).await?;
        extract_user_from_json(&userinfo_json)
            .ok_or_else(|| "Failed to extract user from userinfo".to_string())
    }

    async fn execute_request(
        &self,
        req: Request<Full<Bytes>>,
    ) -> Result<serde_json::Value, String> {
        let uri = req.uri().clone();
        let resp = tokio::time::timeout(TOKEN_REQUEST_TIMEOUT, self.client.request(req))
            .await
            .map_err(|_| format!("Request to {uri} timed out"))?
            .map_err(|e| format!("Request to {uri} failed: {e}"))?;

        let status = resp.status();
        let body = Limited::new(resp.into_body(), MAX_BODY_SIZE)
            .collect()
            .await
            .map_err(|e| format!("Failed to read response body: {e}"))?
            .to_bytes();

        if !status.is_success() {
            return Err(format!("Server returned error status {status}"));
        }
        serde_json::from_slice(&body).map_err(|e| format!("Invalid JSON response: {e}"))
    }

    async fn create_session_and_redirect(
        &self,
        ctx: &AuthContext<'_>,
        user: &str,
        target: &str,
    ) -> Response<Full<Bytes>> {
        let host = ctx.host.unwrap_or("localhost");
        let (token, expiry) = match self.sessions.create_proxy_session(host, user).await {
            Ok(session) => session,
            Err(err) => {
                error!(%err, "failed to create proxy session");
                return text_response(StatusCode::SERVICE_UNAVAILABLE, "Service Unavailable\n");
            }
        };
        let secure = secure_attr(ctx);
        let session_cookie = format!(
            "{COOKIE_NAME}={token}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax{secure}",
            expiry.as_secs()
        );
        let clear_state =
            format!("{STATE_COOKIE_NAME}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax{secure}");
        let mut response = redirect_response(
            StatusCode::SEE_OTHER,
            safe_redirect(target),
            Some(&session_cookie),
        );
        if let Ok(val) = HeaderValue::from_str(&clear_state) {
            response.headers_mut().append(SET_COOKIE, val);
        }
        response
    }
}

fn oidc_action(segments: &[String]) -> Option<OidcAction> {
    if segments.len() < 3 || segments[0] != ENDPOINT_PREFIX[0] || segments[1] != ENDPOINT_PREFIX[1]
    {
        return None;
    }
    let target = if segments[2] == "oidc" && segments.len() >= 4 {
        segments[3].as_str()
    } else {
        segments[2].as_str()
    };
    match target {
        "login" => Some(OidcAction::Login),
        "callback" => Some(OidcAction::Callback),
        "logout" => Some(OidcAction::Logout),
        _ => None,
    }
}

fn resolve_auth_endpoint(config: &OidcAuth) -> String {
    if !config.authorization_url.is_empty() {
        return config.authorization_url.clone();
    }
    let issuer = config.issuer_url.trim_end_matches('/');
    if issuer.contains("accounts.google.com") {
        "https://accounts.google.com/o/oauth2/v2/auth".to_string()
    } else if issuer.contains("github.com") {
        "https://github.com/login/oauth/authorize".to_string()
    } else {
        format!("{issuer}/protocol/openid-connect/auth")
    }
}

fn resolve_token_endpoint(config: &OidcAuth) -> String {
    if !config.token_url.is_empty() {
        return config.token_url.clone();
    }
    let issuer = config.issuer_url.trim_end_matches('/');
    if issuer.contains("accounts.google.com") {
        "https://oauth2.googleapis.com/token".to_string()
    } else if issuer.contains("github.com") {
        "https://github.com/login/oauth/access_token".to_string()
    } else {
        format!("{issuer}/protocol/openid-connect/token")
    }
}

fn resolve_userinfo_endpoint(config: &OidcAuth) -> String {
    if !config.userinfo_url.is_empty() {
        return config.userinfo_url.clone();
    }
    let issuer = config.issuer_url.trim_end_matches('/');
    if issuer.contains("accounts.google.com") {
        "https://openidconnect.googleapis.com/v1/userinfo".to_string()
    } else if issuer.contains("github.com") {
        "https://api.github.com/user".to_string()
    } else if !issuer.is_empty() {
        format!("{issuer}/protocol/openid-connect/userinfo")
    } else {
        String::new()
    }
}

fn callback_uri(ctx: &AuthContext<'_>) -> String {
    format!(
        "{}://{}/{}/{}/oidc/callback",
        ctx.proto,
        ctx.host.unwrap_or("localhost"),
        ENDPOINT_PREFIX[0],
        ENDPOINT_PREFIX[1]
    )
}

fn new_random_string(len: usize) -> String {
    Alphanumeric.sample_string(&mut rand::rng(), len)
}

fn secure_attr(ctx: &AuthContext<'_>) -> &'static str {
    if ctx.proto == "http" { "" } else { "; Secure" }
}

fn oidc_token(headers: &HeaderMap) -> Option<String> {
    cookie_values(headers, COOKIE_NAME)
        .next()
        .map(str::to_string)
}

fn remove_oidc_cookie(headers: &mut HeaderMap) {
    remove_cookie(headers, COOKIE_NAME);
}

fn inject_user_headers(headers: &mut HeaderMap, user: &str) {
    if let Ok(val) = HeaderValue::from_str(user) {
        headers.insert(HeaderName::from_static("x-forwarded-user"), val.clone());
        headers.insert(HeaderName::from_static("x-forwarded-email"), val);
    }
}

fn parse_callback_params(query: &str) -> Option<(String, String)> {
    let mut code = None;
    let mut state = None;
    for (k, v) in form_urlencoded::parse(query.as_bytes()) {
        if k == "code" {
            code = Some(v.into_owned());
        } else if k == "state" {
            state = Some(v.into_owned());
        }
    }
    Some((code?, state?))
}

fn read_state_cookie(headers: &HeaderMap) -> Option<(String, String)> {
    let val = cookie_values(headers, STATE_COOKIE_NAME).next()?;
    let (state, target) = val.split_once(':')?;
    Some((state.to_string(), target.to_string()))
}

fn decode_jwt_user(jwt: &str) -> Option<String> {
    let mut parts = jwt.split('.');
    let _header = parts.next()?;
    let payload = parts.next()?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(payload))
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    extract_user_from_json(&json)
}

fn extract_user_from_json(json: &serde_json::Value) -> Option<String> {
    if let Some(email) = json.get("email").and_then(|v| v.as_str())
        && !email.is_empty()
    {
        return Some(email.to_string());
    }
    if let Some(username) = json.get("preferred_username").and_then(|v| v.as_str())
        && !username.is_empty()
    {
        return Some(username.to_string());
    }
    if let Some(login) = json.get("login").and_then(|v| v.as_str())
        && !login.is_empty()
    {
        return Some(login.to_string());
    }
    if let Some(sub) = json.get("sub").and_then(|v| v.as_str())
        && !sub.is_empty()
    {
        return Some(sub.to_string());
    }
    None
}

fn query_redirect<B>(req: &Request<B>) -> String {
    let query = req.uri().query().unwrap_or_default();
    form_urlencoded::parse(query.as_bytes())
        .find(|(name, _)| name == "redirect")
        .map_or_else(|| "/".to_string(), |(_, v)| safe_redirect(&v).to_string())
}

fn sign_in_required<B>(req: &Request<B>, ctx: &AuthContext<'_>) -> Response<Full<Bytes>> {
    if ![Method::GET, Method::HEAD].contains(req.method()) {
        return text_response(StatusCode::UNAUTHORIZED, "Unauthorized\n");
    }
    let target = req.uri().path_and_query().map_or("/", |t| t.as_str());
    let redirect: String = form_urlencoded::byte_serialize(target.as_bytes()).collect();
    let location = format!(
        "{}/{}/{}/oidc/login?redirect={redirect}",
        ctx.base_path, ENDPOINT_PREFIX[0], ENDPOINT_PREFIX[1]
    );
    redirect_response(StatusCode::FOUND, &location, None)
}

fn forbidden_page(user: &str) -> Response<Full<Bytes>> {
    let body = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Access Denied</title>\
         <style>body{{font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;background:#09090b;color:#f4f4f5;}}\
         .card{{background:#18181b;padding:2rem;border-radius:0.5rem;max-width:28rem;text-align:center;box-shadow:0 10px 15px -3px rgba(0,0,0,0.5);}}\
         h1{{font-size:1.5rem;margin-bottom:0.75rem;color:#ef4444;}}p{{color:#a1a1aa;margin-bottom:1.5rem;}}\
         a{{display:inline-block;padding:0.5rem 1rem;background:#27272a;color:#f4f4f5;text-decoration:none;border-radius:0.375rem;}}\
         </style></head><body><div class=\"card\"><h1>Access Denied</h1>\
         <p>Account <strong>{}</strong> is not authorized to access this service.</p>\
         <a href=\"/{}/{}/oidc/logout\">Sign out</a></div></body></html>",
        user, ENDPOINT_PREFIX[0], ENDPOINT_PREFIX[1]
    );
    let mut res = Response::new(Full::new(Bytes::from(body)));
    *res.status_mut() = StatusCode::FORBIDDEN;
    res.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    res
}

fn text_response(status: StatusCode, text: &str) -> Response<Full<Bytes>> {
    let mut res = Response::new(Full::new(Bytes::from(text.to_string())));
    *res.status_mut() = status;
    res.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    res
}

fn redirect_response(
    status: StatusCode,
    location: &str,
    cookie: Option<&str>,
) -> Response<Full<Bytes>> {
    let mut res = Response::new(Full::new(Bytes::new()));
    *res.status_mut() = status;
    res.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_static("no-cache, no-store, must-revalidate, private"),
    );
    res.headers_mut()
        .insert(PRAGMA, HeaderValue::from_static("no-cache"));
    if let Ok(val) = HeaderValue::from_str(location) {
        res.headers_mut().insert(LOCATION, val);
    }
    if let Some(c) = cookie
        && let Ok(val) = HeaderValue::from_str(c)
    {
        res.headers_mut().insert(SET_COOKIE, val);
    }
    res
}
