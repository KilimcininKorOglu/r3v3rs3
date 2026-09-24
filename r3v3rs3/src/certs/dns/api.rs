//! A small JSON and XML HTTPS client for DNS provider APIs.

use crate::cdn::fetch::HttpClient;
use anyhow::{anyhow, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    Method, Request, StatusCode, Uri,
    header::{CONTENT_TYPE, USER_AGENT},
};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    future::Future,
    time::{Duration, Instant},
};

/// An ID as a string. Providers use strings or numbers as IDs.
pub fn id_text(value: &Value) -> Option<String> {
    match value {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

/// The string field `key` of every object in the JSON array `items`.
pub fn strings(items: &Value, key: &str) -> Vec<String> {
    items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item[key].as_str().map(str::to_string))
        .collect()
}

/// The body of a successful response, or an error with the status and the start of the body.
fn success_body(target: &str, status: StatusCode, body: Bytes) -> anyhow::Result<Bytes> {
    if !status.is_success() {
        let text = String::from_utf8_lossy(&body[..body.len().min(MAX_ERROR_BODY)]);
        bail!("{target} returned {status}: {text}");
    }
    Ok(body)
}

/// The body as JSON. An empty body is `null`.
fn parse_json(body: &[u8]) -> anyhow::Result<Value> {
    if body.is_empty() {
        return Ok(Value::Null);
    }
    Ok(serde_json::from_slice(body)?)
}

/// Seconds before the expiry of an access token when a new token is requested.
const TOKEN_MARGIN: u64 = 60;

/// An OAuth access token that is reused until shortly before it expires.
#[derive(Default)]
pub struct TokenCache(tokio::sync::Mutex<Option<(String, Instant)>>);

impl TokenCache {
    /// The cached token, or the token of the OAuth token response that `request` returns.
    /// `request` runs only when no valid token is cached.
    pub async fn get(
        &self,
        request: impl Future<Output = anyhow::Result<Value>>,
    ) -> anyhow::Result<String> {
        let mut cached = self.0.lock().await;
        if let Some((token, expires)) = cached.as_ref()
            && Instant::now() < *expires
        {
            return Ok(token.clone());
        }
        let response = request.await?;
        let token = response["access_token"]
            .as_str()
            .ok_or_else(|| anyhow!("the token endpoint returned no access token"))?
            .to_string();
        let lifetime = response["expires_in"]
            .as_u64()
            .ok_or_else(|| anyhow!("the token endpoint returned no token lifetime"))?;
        let expires = Instant::now() + Duration::from_secs(lifetime.saturating_sub(TOKEN_MARGIN));
        *cached = Some((token.clone(), expires));
        Ok(token)
    }
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BODY_SIZE: usize = 4 * 1024 * 1024;
/// Bytes of an error response that the error message keeps.
const MAX_ERROR_BODY: usize = 300;

pub struct ApiClient {
    http: HttpClient,
    base: String,
    authority: String,
}

pub struct ApiRequest {
    method: Method,
    path: String,
    headers: Vec<(&'static str, String)>,
    body: Option<(&'static str, Vec<u8>)>,
}

impl ApiRequest {
    /// `path` is relative to the API URL and can hold a query.
    pub fn new(method: Method, path: String) -> Self {
        Self {
            method,
            path,
            headers: Vec::new(),
            body: None,
        }
    }

    pub fn header(mut self, name: &'static str, value: String) -> Self {
        self.headers.push((name, value));
        self
    }

    pub fn bearer(self, token: &str) -> Self {
        self.header("authorization", format!("Bearer {token}"))
    }

    pub fn json(self, value: &serde_json::Value) -> Self {
        self.body("application/json", value.to_string().into_bytes())
    }

    /// A URL-encoded form body.
    pub fn form(self, fields: &[(&str, &str)]) -> Self {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        self.body("application/x-www-form-urlencoded", body.into_bytes())
    }

    pub fn body(mut self, content_type: &'static str, body: Vec<u8>) -> Self {
        self.body = Some((content_type, body));
        self
    }
}

impl ApiClient {
    pub fn new(http: HttpClient, api_url: Option<&str>, default_url: &str) -> anyhow::Result<Self> {
        let base = api_url.unwrap_or(default_url).trim_end_matches('/');
        let uri: Uri = base
            .parse()
            .map_err(|err| anyhow!("invalid DNS provider API URL {base}: {err}"))?;
        let authority = uri
            .authority()
            .ok_or_else(|| anyhow!("DNS provider API URL has no host: {base}"))?
            .to_string();
        Ok(Self {
            http,
            base: base.to_string(),
            authority,
        })
    }

    /// The host and the optional port of the API URL.
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// The path of the API URL, which request paths are appended to.
    pub fn base_path(&self) -> &str {
        self.base
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|index| &rest[index..]))
            .unwrap_or("")
    }

    /// The full URL of a request path.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// Sends the request and returns the body of a successful response.
    /// The error message holds the method, the path, the status and the start of the body,
    /// never the request headers.
    pub async fn send(&self, request: ApiRequest) -> anyhow::Result<Bytes> {
        let (target, status, body) = self.exchange(request).await?;
        success_body(&target, status, body)
    }

    /// Sends the request and reads the value at the JSON `pointer` of the response.
    /// A 404 response is the default value.
    pub async fn optional<T: DeserializeOwned + Default>(
        &self,
        request: ApiRequest,
        pointer: &str,
    ) -> anyhow::Result<T> {
        let (target, status, body) = self.exchange(request).await?;
        if status == StatusCode::NOT_FOUND {
            return Ok(T::default());
        }
        let response = parse_json(&success_body(&target, status, body)?)?;
        let value = response
            .pointer(pointer)
            .ok_or_else(|| anyhow!("{target} returned no {pointer}"))?;
        Ok(T::deserialize(value)?)
    }

    /// Sends the request and returns the method and path, the status and the body. A status that
    /// is not a success is not an error here.
    pub async fn exchange(
        &self,
        request: ApiRequest,
    ) -> anyhow::Result<(String, StatusCode, Bytes)> {
        let target = format!("{} {}", request.method, request.path);
        let mut builder = Request::builder()
            .method(request.method)
            .uri(self.url(&request.path))
            .header(USER_AGENT, concat!("r3v3rs3/", env!("CARGO_PKG_VERSION")));
        for (name, value) in request.headers {
            builder = builder.header(name, value);
        }
        let body = match request.body {
            Some((content_type, body)) => {
                builder = builder.header(CONTENT_TYPE, content_type);
                Full::new(Bytes::from(body))
            }
            None => Full::new(Bytes::new()),
        };
        let response =
            tokio::time::timeout(REQUEST_TIMEOUT, self.http.request(builder.body(body)?))
                .await
                .map_err(|_| anyhow!("{target} timed out"))??;
        let status = response.status();
        let body = tokio::time::timeout(
            REQUEST_TIMEOUT,
            Limited::new(response.into_body(), MAX_BODY_SIZE).collect(),
        )
        .await
        .map_err(|_| anyhow!("{target} timed out"))?
        .map_err(|err| anyhow!(err))?
        .to_bytes();
        Ok((target, status, body))
    }

    /// Sends the request and parses the body as JSON. An empty body is `null`.
    pub async fn json(&self, request: ApiRequest) -> anyhow::Result<Value> {
        parse_json(&self.send(request).await?)
    }

    /// Sends the request and returns the ID at the JSON `pointer` of the response.
    /// Providers use strings or numbers as IDs, and both become a string.
    pub async fn json_id(
        &self,
        request: ApiRequest,
        pointer: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(self.json(request).await?.pointer(pointer).and_then(id_text))
    }

    /// Looks up the candidate zones of `fqdn`, the longest first, with the requests that
    /// `request` builds. Returns the name and the ID at `pointer` of the first zone found.
    pub async fn find_zone<'a>(
        &self,
        fqdn: &'a str,
        request: impl Fn(&str) -> ApiRequest,
        pointer: &str,
    ) -> anyhow::Result<Option<(&'a str, String)>> {
        for zone in super::zone_candidates(fqdn) {
            if let Some(id) = self.json_id(request(zone), pointer).await? {
                return Ok(Some((zone, id)));
            }
        }
        Ok(None)
    }
}
