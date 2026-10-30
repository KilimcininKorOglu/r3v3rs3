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
use aws_lc_rs::signature::{RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use hmac::{Hmac, KeyInit, Mac};
use r3v3rs3_api::policy::{OidcAuth, default_oidc_scopes, is_email_allowed};
use rand::distr::{Alphanumeric, SampleString};
use sha2::Sha256;
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
        if let Some(id_token) = token_json.get("id_token").and_then(|v| v.as_str()) {
            return self.verify_and_decode_id_token(id_token).await;
        }
        if let Some(access_token) = token_json.get("access_token").and_then(|v| v.as_str()) {
            return self.fetch_userinfo(access_token).await;
        }
        if let Some(user) = extract_user_from_json(&token_json) {
            return Ok(user);
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

    async fn fetch_jwks(&self) -> Result<serde_json::Value, String> {
        let endpoint = resolve_jwks_endpoint(&self.config);
        if endpoint.is_empty() {
            return Err("no JWKS endpoint configured".to_string());
        }
        let req = Request::builder()
            .method(Method::GET)
            .uri(&endpoint)
            .header("accept", "application/json")
            .body(Full::new(Bytes::new()))
            .map_err(|e| e.to_string())?;
        self.execute_request(req).await
    }

    async fn verify_jwt_signature(
        &self,
        alg: &str,
        kid: Option<&str>,
        message: &[u8],
        sig_bytes: &[u8],
    ) -> Result<(), String> {
        match alg {
            "none" => Err("algorithm 'none' is forbidden for ID tokens".to_string()),
            "HS256" => self.verify_hs256_signature(message, sig_bytes),
            "RS256" => self.verify_rs256_signature(kid, message, sig_bytes).await,
            other => Err(format!("unsupported ID token algorithm: {other}")),
        }
    }

    fn verify_hs256_signature(&self, message: &[u8], sig_bytes: &[u8]) -> Result<(), String> {
        if self.config.client_secret.is_empty() {
            return Err("client_secret required for HS256 verification".to_string());
        }
        if verify_hs256(message, sig_bytes, self.config.client_secret.as_bytes()) {
            Ok(())
        } else {
            Err("invalid HS256 signature".to_string())
        }
    }

    async fn verify_rs256_signature(
        &self,
        kid: Option<&str>,
        message: &[u8],
        sig_bytes: &[u8],
    ) -> Result<(), String> {
        let jwks = self.fetch_jwks().await?;
        let keys = jwks
            .get("keys")
            .and_then(|v| v.as_array())
            .ok_or_else(|| "invalid JWKS keys array".to_string())?;
        let verified = keys.iter().any(|k| {
            if let Some(expected_kid) = kid
                && k.get("kid").and_then(|v| v.as_str()) != Some(expected_kid)
            {
                return false;
            }
            verify_rs256_jwk(message, sig_bytes, k)
        });
        if verified {
            Ok(())
        } else {
            Err("invalid RS256 signature".to_string())
        }
    }

    async fn verify_and_decode_id_token(&self, jwt: &str) -> Result<String, String> {
        let (header_b64, payload_b64, sig_b64) = parse_jwt_parts(jwt)?;
        let header = parse_jwt_header(header_b64)?;
        let alg = header
            .get("alg")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing alg in header".to_string())?;
        let kid = header.get("kid").and_then(|v| v.as_str());

        let sig_bytes =
            b64_decode(sig_b64).ok_or_else(|| "invalid base64 signature".to_string())?;
        let message = format!("{header_b64}.{payload_b64}");

        self.verify_jwt_signature(alg, kid, message.as_bytes(), &sig_bytes)
            .await?;

        let payload = parse_jwt_payload(payload_b64)?;
        validate_jwt_claims(&payload, &self.config)
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

fn resolve_jwks_endpoint(config: &OidcAuth) -> String {
    let issuer = config.issuer_url.trim_end_matches('/');
    if issuer.contains("accounts.google.com") {
        "https://www.googleapis.com/oauth2/v3/certs".to_string()
    } else if !issuer.is_empty() {
        format!("{issuer}/.well-known/jwks.json")
    } else {
        String::new()
    }
}

fn b64_decode(input: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(input))
        .ok()
}

fn parse_jwt_parts(jwt: &str) -> Result<(&str, &str, &str), String> {
    let mut parts = jwt.split('.');
    let header = parts.next().ok_or("missing header")?;
    let payload = parts.next().ok_or("missing payload")?;
    let signature = parts.next().ok_or("missing signature")?;
    if parts.next().is_some() {
        return Err("too many JWT segments".to_string());
    }
    Ok((header, payload, signature))
}

fn parse_jwt_header(header_b64: &str) -> Result<serde_json::Value, String> {
    let bytes = b64_decode(header_b64).ok_or("invalid base64 header")?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid header json: {e}"))
}

fn parse_jwt_payload(payload_b64: &str) -> Result<serde_json::Value, String> {
    let bytes = b64_decode(payload_b64).ok_or("invalid base64 payload")?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid payload json: {e}"))
}

fn validate_jwt_claims(payload: &serde_json::Value, config: &OidcAuth) -> Result<String, String> {
    validate_exp(payload)?;
    validate_iss(payload, config)?;
    validate_aud(payload, config)?;
    extract_user_from_json(payload).ok_or_else(|| "no user identity in ID token".to_string())
}

fn validate_exp(payload: &serde_json::Value) -> Result<(), String> {
    let now = crate::clock::unix_ms() / 1000;
    if let Some(exp) = payload.get("exp").and_then(|v| v.as_i64())
        && exp + 60 < now as i64
    {
        return Err("ID token has expired".to_string());
    }
    Ok(())
}

fn validate_iss(payload: &serde_json::Value, config: &OidcAuth) -> Result<(), String> {
    if config.issuer_url.is_empty() {
        return Ok(());
    }
    let Some(iss) = payload.get("iss").and_then(|v| v.as_str()) else {
        return Err("missing iss claim in ID token".to_string());
    };
    if iss.trim_end_matches('/') != config.issuer_url.trim_end_matches('/') {
        return Err(format!(
            "ID token issuer mismatch: expected {}, got {iss}",
            config.issuer_url
        ));
    }
    Ok(())
}

fn validate_aud(payload: &serde_json::Value, config: &OidcAuth) -> Result<(), String> {
    if config.client_id.is_empty() {
        return Ok(());
    }
    let Some(aud) = payload.get("aud") else {
        return Err("missing aud claim in ID token".to_string());
    };
    let matches = match aud {
        serde_json::Value::String(s) => s == &config.client_id,
        serde_json::Value::Array(arr) => arr.iter().any(|v| v.as_str() == Some(&config.client_id)),
        _ => false,
    };
    if !matches {
        return Err(format!(
            "ID token audience mismatch: expected {}",
            config.client_id
        ));
    }
    Ok(())
}

fn verify_hs256(message: &[u8], sig_bytes: &[u8], secret: &[u8]) -> bool {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(message);
    mac.verify_slice(sig_bytes).is_ok()
}

fn verify_rs256_jwk(message: &[u8], sig_bytes: &[u8], jwk: &serde_json::Value) -> bool {
    let Some(n_b64) = jwk.get("n").and_then(|v| v.as_str()) else {
        return false;
    };
    let Some(e_b64) = jwk.get("e").and_then(|v| v.as_str()) else {
        return false;
    };
    let Some(n) = b64_decode(n_b64) else {
        return false;
    };
    let Some(e) = b64_decode(e_b64) else {
        return false;
    };
    let der = rsa_pkcs1_der_from_jwk_n_e(&n, &e);
    let verifier = UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, &der);
    verifier.verify(message, sig_bytes).is_ok()
}

fn encode_asn1_int(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(0x02);
    let pad = usize::from(!bytes.is_empty() && (bytes[0] & 0x80) != 0);
    let len = bytes.len() + pad;
    if len < 128 {
        out.push(len as u8);
    } else if len < 256 {
        out.push(0x81);
        out.push(len as u8);
    } else {
        out.push(0x82);
        out.push((len >> 8) as u8);
        out.push((len & 0xff) as u8);
    }
    if pad == 1 {
        out.push(0x00);
    }
    out.extend_from_slice(bytes);
    out
}

fn rsa_pkcs1_der_from_jwk_n_e(n: &[u8], e: &[u8]) -> Vec<u8> {
    let mut seq = Vec::new();
    seq.extend_from_slice(&encode_asn1_int(n));
    seq.extend_from_slice(&encode_asn1_int(e));
    let mut der = Vec::new();
    der.push(0x30);
    let len = seq.len();
    if len < 128 {
        der.push(len as u8);
    } else if len < 256 {
        der.push(0x81);
        der.push(len as u8);
    } else {
        der.push(0x82);
        der.push((len >> 8) as u8);
        der.push((len & 0xff) as u8);
    }
    der.extend_from_slice(&seq);
    der
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

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_jwt(header: &serde_json::Value, payload: &serde_json::Value, secret: &str) -> String {
        let b64 = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let h = b64.encode(header.to_string());
        let p = b64.encode(payload.to_string());
        let message = format!("{h}.{p}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(message.as_bytes());
        let sig = b64.encode(mac.finalize().into_bytes());
        format!("{message}.{sig}")
    }

    #[test]
    fn validate_jwt_claims_checks_exp_iss_and_aud() {
        let config = OidcAuth {
            issuer_url: "https://auth.example.com".to_string(),
            client_id: "my-app".to_string(),
            ..Default::default()
        };

        let valid = serde_json::json!({
            "sub": "user1",
            "iss": "https://auth.example.com",
            "aud": "my-app",
            "exp": (crate::clock::unix_ms() / 1000) + 3600
        });
        assert_eq!(validate_jwt_claims(&valid, &config).unwrap(), "user1");

        let expired = serde_json::json!({
            "sub": "user1",
            "iss": "https://auth.example.com",
            "aud": "my-app",
            "exp": (crate::clock::unix_ms() / 1000) - 100
        });
        assert!(validate_jwt_claims(&expired, &config).is_err());

        let wrong_iss = serde_json::json!({
            "sub": "user1",
            "iss": "https://evil.com",
            "aud": "my-app",
            "exp": (crate::clock::unix_ms() / 1000) + 3600
        });
        assert!(validate_jwt_claims(&wrong_iss, &config).is_err());

        let wrong_aud = serde_json::json!({
            "sub": "user1",
            "iss": "https://auth.example.com",
            "aud": "other-app",
            "exp": (crate::clock::unix_ms() / 1000) + 3600
        });
        assert!(validate_jwt_claims(&wrong_aud, &config).is_err());
    }

    #[test]
    fn hs256_signature_verification_succeeds_for_valid_key() {
        let header = serde_json::json!({ "alg": "HS256", "typ": "JWT" });
        let payload = serde_json::json!({ "sub": "alice" });
        let jwt = build_test_jwt(&header, &payload, "secret-key");
        let (h, p, s) = parse_jwt_parts(&jwt).unwrap();
        let message = format!("{h}.{p}");
        let sig = b64_decode(s).unwrap();
        assert!(verify_hs256(message.as_bytes(), &sig, b"secret-key"));
        assert!(!verify_hs256(message.as_bytes(), &sig, b"wrong-secret"));
    }
}
