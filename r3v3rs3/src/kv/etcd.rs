//! A client of the etcd v3 HTTP API. It reads key ranges, keeps the authentication token and
//! follows changes with a watch stream.

use super::http::{ApiClient, Lines, RESPONSE_TIMEOUT, read_json};
use anyhow::{Context as _, anyhow, bail};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE};
use hyper::{Method, Response, StatusCode};
use serde::Deserialize as _;
use serde_derive::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, PoisonError};
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
#[derive(Debug, Default, Deserialize)]
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct KeyValue {
    /// Base64 text.
    #[serde(default)]
    pub key: String,
    /// Base64 text. The JSON gateway omits an empty value.
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default, deserialize_with = "int64")]
    pub mod_revision: i64,
    /// The lease of the key, or 0.
    #[serde(default, deserialize_with = "int64")]
    pub lease: i64,
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
pub(super) struct WatchResult {
    #[serde(default)]
    canceled: bool,
    #[serde(default, deserialize_with = "int64")]
    pub(super) compact_revision: i64,
    #[serde(default)]
    cancel_reason: String,
    #[serde(default)]
    pub(super) events: Vec<Event>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub(super) enum EventType {
    #[default]
    #[serde(rename = "PUT")]
    Put,
    #[serde(rename = "DELETE")]
    Delete,
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct Event {
    /// The JSON gateway omits the default type, `PUT`.
    #[serde(default, rename = "type")]
    pub(super) kind: EventType,
    #[serde(default)]
    pub(super) kv: KeyValue,
}

/// An int64 field, which the JSON gateway writes as a string.
pub(super) fn int64<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<i64, D::Error> {
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

/// The result of a message of the watch stream. A failed watch, or a watch that etcd canceled for
/// another reason than a compaction, is an error.
pub(super) fn watch_result(line: &[u8]) -> anyhow::Result<WatchResult> {
    let message: WatchMessage = serde_json::from_slice(line).context("invalid watch message")?;
    if let Some(error) = message.error {
        bail!("the watch failed: {error}");
    }
    let result = message.result.unwrap_or_default();
    if result.canceled && result.compact_revision <= 0 {
        bail!("etcd canceled the watch: {}", result.cancel_reason);
    }
    Ok(result)
}

/// Whether a message of the watch stream needs a new read: a change of a key, or a compacted
/// start revision.
fn watch_changed(line: &[u8]) -> anyhow::Result<bool> {
    let result = watch_result(line)?;
    Ok(result.compact_revision > 0 || !result.events.is_empty())
}

/// The next line of a watch stream.
pub(super) async fn next_watch_line(lines: &mut Lines) -> anyhow::Result<Vec<u8>> {
    tokio::time::timeout(WATCH_IDLE_TIMEOUT, lines.next())
        .await
        .map_err(|_| anyhow!("the watch stream is idle"))??
        .context("the watch stream ended")
}

/// A Base64 key range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRange {
    /// The prefix of the keys.
    pub prefix: String,
    key: String,
    range_end: String,
}

impl KeyRange {
    /// The keys under `<prefix>/`. The range prefix has no leading or trailing `/`.
    pub fn new(prefix: &str) -> Self {
        let prefix = prefix.trim_matches('/').to_string();
        let key = format!("{prefix}/");
        Self::starting_with(&key, prefix)
    }

    /// Every key that starts with `prefix`.
    pub(super) fn with_prefix(prefix: &str) -> Self {
        Self::starting_with(prefix, prefix.to_string())
    }

    fn starting_with(start: &str, prefix: String) -> Self {
        Self {
            key: BASE64_STANDARD.encode(start),
            range_end: BASE64_STANDARD.encode(prefix_end(start.as_bytes())),
            prefix,
        }
    }
}

/// The first key after every key that starts with `prefix`.
fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < u8::MAX {
            end.push(last + 1);
            return end;
        }
    }
    // The key `\0` as the range end means the end of the key space.
    vec![0]
}

#[derive(Clone)]
pub struct EtcdClient {
    client: ApiClient,
    /// The user name and the password of etcd authentication.
    credentials: Option<(String, String)>,
    token: Arc<Mutex<Option<String>>>,
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
            token: Arc::default(),
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
        let mut lines = self.watch(range, revision).await?;
        loop {
            let line = next_watch_line(&mut lines).await?;
            if !line.trim_ascii().is_empty() && watch_changed(&line)? {
                return Ok(());
            }
        }
    }

    /// Opens a watch stream on the keys of the range from the revision after `revision`.
    pub(super) async fn watch(&self, range: &KeyRange, revision: i64) -> anyhow::Result<Lines> {
        let body = json!({"create_request": {
            "key": range.key,
            "range_end": range.range_end,
            "start_revision": revision.saturating_add(1).to_string(),
            "progress_notify": true,
        }});
        Ok(Lines::new(self.post("/v3/watch", &body).await?))
    }

    pub(super) async fn post(
        &self,
        path: &str,
        body: &Value,
    ) -> anyhow::Result<Response<Incoming>> {
        self.post_with(path, body, &[]).await
    }

    /// Sends a request with the token. An expired or revoked token is replaced once. A status in
    /// `allowed` is not an error.
    pub(super) async fn post_with(
        &self,
        path: &str,
        body: &Value,
        allowed: &[StatusCode],
    ) -> anyhow::Result<Response<Incoming>> {
        let mut retry = allowed.to_vec();
        if self.credentials.is_some() {
            retry.push(StatusCode::UNAUTHORIZED);
        }
        let response = self
            .send(self.token().as_deref(), path, body, &retry)
            .await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        self.authenticate().await?;
        self.send(self.token().as_deref(), path, body, allowed)
            .await
    }

    fn token(&self) -> Option<String> {
        self.token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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
    fn watch_events_have_a_type_and_a_key() {
        let line = br#"{"result":{"events":[{"kv":{"key":"YQ==","mod_revision":"4"}},{"type":"DELETE","kv":{"key":"Yg==","mod_revision":"5"}}]}}"#;
        let events = watch_result(line).unwrap().events;
        let kinds = events.iter().map(|event| event.kind).collect::<Vec<_>>();
        assert_eq!(kinds, [EventType::Put, EventType::Delete]);
        assert_eq!(events[1].kv.mod_revision, 5);
    }

    #[test]
    fn the_range_covers_only_the_prefix() {
        let range = KeyRange::new("/apps/r3v3rs3/");
        assert_eq!(range.prefix, "apps/r3v3rs3");
        assert_eq!(range.key, BASE64_STANDARD.encode("apps/r3v3rs3/"));
        assert_eq!(range.range_end, BASE64_STANDARD.encode("apps/r3v3rs30"));

        let range = KeyRange::with_prefix("a\u{7f}");
        assert_eq!(range.range_end, BASE64_STANDARD.encode([b'a', 0x80]));
        assert_eq!(prefix_end(&[b'a', 0xff, 0xff]), b"b");
        assert_eq!(prefix_end(&[0xff]), [0]);

        let key = BASE64_STANDARD.encode("apps/r3v3rs3/http/app/ports");
        let response: RangeResponse = serde_json::from_value(json!({
            "header": {"revision": "12"},
            "kvs": [{"key": key, "value": BASE64_STANDARD.encode("web"), "mod_revision": "11", "lease": "7"}],
        }))
        .unwrap();
        assert_eq!(response.revision(), 12);
        assert_eq!(response.kvs[0].key, key);
        assert_eq!(
            (response.kvs[0].mod_revision, response.kvs[0].lease),
            (11, 7)
        );

        let empty: RangeResponse = serde_json::from_value(json!({"header": {}})).unwrap();
        assert!(empty.kvs.is_empty());
        assert_eq!(empty.revision(), 0);
    }
}
