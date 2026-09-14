//! A client of the etcd v3 HTTP API. It reads key ranges, keeps the authentication token and
//! follows changes with a watch stream.

use super::http::{read_json, ApiClient, Lines, RESPONSE_TIMEOUT};
use anyhow::{anyhow, bail, Context as _};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE};
use hyper::{Method, Response, StatusCode};
use serde::Deserialize as _;
use serde_derive::Deserialize;
use serde_json::{json, Value};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

/// etcd sends a progress notification on an idle watch every ten minutes, so a longer silence
/// means a lost connection.
const WATCH_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Default, Deserialize)]
struct ResponseHeader {
    #[serde(default, deserialize_with = "int64")]
    revision: i64,
}

/// The keys of a range and the revision of the read.
#[derive(Debug, Deserialize)]
pub struct RangeResponse {
    #[serde(default)]
    header: ResponseHeader,
    /// The JSON gateway omits an empty list.
    #[serde(default)]
    pub kvs: Vec<KeyValue>,
}

impl RangeResponse {
    pub fn revision(&self) -> i64 {
        self.header.revision
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct KeyValue {
    /// Base64 text.
    pub key: String,
    /// Base64 text. The JSON gateway omits an empty value.
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthenticateResponse {
    token: String,
}

#[derive(Debug, Deserialize)]
struct WatchMessage {
    #[serde(default)]
    result: Option<WatchResult>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Default, Deserialize)]
struct WatchResult {
    #[serde(default)]
    canceled: bool,
    #[serde(default, deserialize_with = "int64")]
    compact_revision: i64,
    #[serde(default)]
    cancel_reason: String,
    #[serde(default)]
    events: Vec<Value>,
}

/// An int64 field, which the JSON gateway writes as a string.
fn int64<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Int64 {
        Text(String),
        Number(i64),
    }
    match Int64::deserialize(deserializer)? {
        Int64::Number(number) => Ok(number),
        Int64::Text(text) => text.parse().map_err(serde::de::Error::custom),
    }
}

/// Whether a message of the watch stream needs a new read: a change of a key, or a compacted
/// start revision.
fn watch_changed(line: &[u8]) -> anyhow::Result<bool> {
    let message: WatchMessage = serde_json::from_slice(line).context("invalid watch message")?;
    if let Some(error) = message.error {
        bail!("the watch failed: {error}");
    }
    let result = message.result.unwrap_or_default();
    if result.compact_revision > 0 {
        return Ok(true);
    }
    if result.canceled {
        bail!("etcd canceled the watch: {}", result.cancel_reason);
    }
    Ok(!result.events.is_empty())
}

/// The keys under a prefix as a Base64 key range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRange {
    /// The prefix without a leading or a trailing `/`.
    pub prefix: String,
    key: String,
    range_end: String,
}

impl KeyRange {
    pub fn new(prefix: &str) -> Self {
        let prefix = prefix.trim_matches('/').to_string();
        // The range ends before the byte that follows `/`, so `r3v3rs3x/...` is outside it.
        Self {
            key: BASE64_STANDARD.encode(format!("{prefix}/")),
            range_end: BASE64_STANDARD.encode(format!("{prefix}0")),
            prefix,
        }
    }
}

pub struct EtcdClient {
    client: ApiClient,
    /// The user name and the password of etcd authentication.
    credentials: Option<(String, String)>,
    token: Mutex<Option<String>>,
}

impl EtcdClient {
    /// An empty `username` turns authentication off.
    pub fn new(client: ApiClient, username: &str, password: Option<&str>) -> Self {
        let username = username.trim();
        let credentials = (!username.is_empty()).then(|| {
            let password = password.unwrap_or_default().to_string();
            (username.to_string(), password)
        });
        Self {
            client,
            credentials,
            token: Mutex::default(),
        }
    }

    /// Gets a new token when authentication is set.
    pub async fn authenticate(&self) -> anyhow::Result<()> {
        let Some((name, password)) = &self.credentials else {
            return Ok(());
        };
        let body = json!({"name": name, "password": password});
        let response = self
            .send(None, "/v3/auth/authenticate", &body, &[])
            .await
            .context("etcd authentication failed")?;
        let auth: AuthenticateResponse = read_json(response).await?;
        *self.token.lock().unwrap_or_else(PoisonError::into_inner) = Some(auth.token);
        Ok(())
    }

    pub async fn range(&self, range: &KeyRange) -> anyhow::Result<RangeResponse> {
        let body = json!({"key": range.key, "range_end": range.range_end});
        let response = self.post("/v3/kv/range", &body).await?;
        read_json(response).await
    }

    /// Watches the keys of the range from the revision after `revision`, and returns at the first
    /// change.
    pub async fn wait_for_change(&self, range: &KeyRange, revision: i64) -> anyhow::Result<()> {
        let body = json!({"create_request": {
            "key": range.key,
            "range_end": range.range_end,
            "start_revision": revision.saturating_add(1).to_string(),
            "progress_notify": true,
        }});
        let response = self.post("/v3/watch", &body).await?;
        let mut lines = Lines::new(response);
        loop {
            let line = tokio::time::timeout(WATCH_IDLE_TIMEOUT, lines.next())
                .await
                .map_err(|_| anyhow!("the watch stream is idle"))??
                .context("the watch stream ended")?;
            if !line.trim_ascii().is_empty() && watch_changed(&line)? {
                return Ok(());
            }
        }
    }

    fn token(&self) -> Option<String> {
        self.token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Sends a request with the token. An expired or revoked token is replaced once.
    async fn post(&self, path: &str, body: &Value) -> anyhow::Result<Response<Incoming>> {
        let retry: &[StatusCode] = if self.credentials.is_some() {
            &[StatusCode::UNAUTHORIZED]
        } else {
            &[]
        };
        let response = self
            .send(self.token().as_deref(), path, body, retry)
            .await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        self.authenticate().await?;
        self.send(self.token().as_deref(), path, body, &[]).await
    }

    async fn send(
        &self,
        token: Option<&str>,
        path: &str,
        body: &Value,
        allowed: &[StatusCode],
    ) -> anyhow::Result<Response<Incoming>> {
        let mut builder = self
            .client
            .builder(Method::POST, path)
            .header(CONTENT_TYPE, "application/json");
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, token);
        }
        let request = builder.body(Full::new(Bytes::from(body.to_string())))?;
        self.client
            .send_with(request, RESPONSE_TIMEOUT, allowed)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_messages_report_changes_and_compaction() {
        let changed = |line: &str| watch_changed(line.as_bytes());
        let created = r#"{"result":{"header":{"revision":"7"},"created":true}}"#;
        assert!(!changed(created).unwrap());
        let progress = r#"{"result":{"header":{"revision":"9"}}}"#;
        assert!(!changed(progress).unwrap());
        let event = r#"{"result":{"header":{"revision":"8"},"events":[{"kv":{"key":"YQ=="}}]}}"#;
        assert!(changed(event).unwrap());
        let compacted = r#"{"result":{"canceled":true,"compact_revision":"5"}}"#;
        assert!(changed(compacted).unwrap());

        let canceled = r#"{"result":{"canceled":true,"cancel_reason":"permission denied"}}"#;
        let error = changed(canceled).unwrap_err().to_string();
        assert!(error.contains("permission denied"), "{error}");
        let failed = r#"{"error":{"grpc_code":16,"message":"etcdserver: invalid auth token"}}"#;
        assert!(changed(failed).is_err());
        assert!(changed("not json").is_err());
    }

    #[test]
    fn the_range_covers_only_the_prefix() {
        let range = KeyRange::new("/apps/r3v3rs3/");
        assert_eq!(range.prefix, "apps/r3v3rs3");
        assert_eq!(range.key, BASE64_STANDARD.encode("apps/r3v3rs3/"));
        assert_eq!(range.range_end, BASE64_STANDARD.encode("apps/r3v3rs30"));

        let key = BASE64_STANDARD.encode("apps/r3v3rs3/http/app/ports");
        let response: RangeResponse = serde_json::from_value(json!({
            "header": {"revision": "12"},
            "kvs": [{"key": key, "value": BASE64_STANDARD.encode("web")}],
        }))
        .unwrap();
        assert_eq!(response.revision(), 12);
        assert_eq!(response.kvs[0].key, key);

        let empty: RangeResponse = serde_json::from_value(json!({"header": {}})).unwrap();
        assert!(empty.kvs.is_empty());
        assert_eq!(empty.revision(), 0);
    }
}
