//! A small JSON and XML HTTPS client for DNS provider APIs.

use crate::cdn::fetch::HttpClient;
use anyhow::{anyhow, bail};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    header::{CONTENT_TYPE, USER_AGENT},
    Method, Request, Uri,
};
use serde_json::Value;
use std::time::Duration;

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

    /// Sends the request and returns the body of a successful response.
    /// The error message holds the method, the path, the status and the start of the body,
    /// never the request headers.
    pub async fn send(&self, request: ApiRequest) -> anyhow::Result<Bytes> {
        let target = format!("{} {}", request.method, request.path);
        let mut builder = Request::builder()
            .method(request.method)
            .uri(format!("{}{}", self.base, request.path))
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
        if !status.is_success() {
            let text = String::from_utf8_lossy(&body[..body.len().min(MAX_ERROR_BODY)]);
            bail!("{target} returned {status}: {text}");
        }
        Ok(body)
    }

    /// Sends the request and parses the body as JSON. An empty body is `null`.
    pub async fn json(&self, request: ApiRequest) -> anyhow::Result<Value> {
        let body = self.send(request).await?;
        if body.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&body)?)
    }

    /// Sends the request and returns the ID at the JSON `pointer` of the response.
    /// Providers use strings or numbers as IDs, and both become a string.
    pub async fn json_id(
        &self,
        request: ApiRequest,
        pointer: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(match self.json(request).await?.pointer(pointer) {
            Some(Value::String(id)) => Some(id.clone()),
            Some(Value::Number(id)) => Some(id.to_string()),
            _ => None,
        })
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
