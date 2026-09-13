//! The etcd provider. It reads the keys under a prefix through the v3 HTTP API and follows their
//! changes with a watch stream.

use super::http::{read_json, ApiClient, Lines, RESPONSE_TIMEOUT};
use super::kv::{self, KvEntry};
use super::{Built, ProxyGroups, Reporter, Watch, DEBOUNCE, MIN_BACKOFF};
use anyhow::{anyhow, bail, Context as _};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE};
use hyper::{Method, Response, StatusCode};
use r3v3rs3_api::discovery::{DiscoveryProvider, EtcdDiscoveryConfig};
use serde::Deserialize as _;
use serde_derive::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::time::Duration;

const PROVIDER: DiscoveryProvider = DiscoveryProvider::Etcd;
/// etcd sends a progress notification on an idle watch every ten minutes, so a longer silence
/// means a lost connection.
const WATCH_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Default, Deserialize)]
struct ResponseHeader {
    #[serde(default, deserialize_with = "int64")]
    revision: i64,
}

#[derive(Debug, Deserialize)]
struct RangeResponse {
    #[serde(default)]
    header: ResponseHeader,
    /// The JSON gateway omits an empty list.
    #[serde(default)]
    kvs: Vec<KeyValue>,
}

#[derive(Debug, Deserialize)]
struct KeyValue {
    /// Base64 text.
    key: String,
    /// Base64 text. The JSON gateway omits an empty value.
    #[serde(default)]
    value: Option<String>,
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
struct KeyRange {
    prefix: String,
    key: String,
    range_end: String,
}

impl KeyRange {
    fn new(prefix: &str) -> Self {
        let prefix = prefix.trim_matches('/').to_string();
        // The range ends before the byte that follows `/`, so `r3v3rs3x/...` is outside it.
        Self {
            key: BASE64_STANDARD.encode(format!("{prefix}/")),
            range_end: BASE64_STANDARD.encode(format!("{prefix}0")),
            prefix,
        }
    }
}

/// The keys as text. A key that is not UTF-8 text is an issue.
fn kv_entries(built: &mut Built, prefix: &str, kvs: Vec<KeyValue>) -> Vec<KvEntry> {
    let mut entries = Vec::new();
    for kv in kvs {
        let key = BASE64_STANDARD
            .decode(&kv.key)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        match key {
            Some(key) => entries.push(KvEntry {
                key,
                value: kv.value,
            }),
            None => built.issue(prefix, format!("the key is not UTF-8 text: {}", kv.key)),
        }
    }
    entries
}

pub(super) struct Provider {
    client: ApiClient,
    /// The user name and the password of etcd authentication.
    credentials: Option<(String, String)>,
    range: KeyRange,
    reporter: Reporter,
}

#[async_trait::async_trait]
impl Watch for Provider {
    fn reporter(&self) -> &Reporter {
        &self.reporter
    }

    async fn watch(&self, backoff: &mut Duration) -> anyhow::Result<Infallible> {
        self.follow(backoff).await
    }
}

impl Provider {
    pub(super) fn new(config: &EtcdDiscoveryConfig, client: ApiClient, reporter: Reporter) -> Self {
        let username = config.username.trim();
        let credentials = (!username.is_empty()).then(|| {
            let password = config.password.clone().unwrap_or_default();
            (username.to_string(), password)
        });
        Self {
            client,
            credentials,
            range: KeyRange::new(&config.prefix),
            reporter,
        }
    }

    /// Reads the keys, then waits until a key changes and reads again.
    async fn follow(&self, backoff: &mut Duration) -> anyhow::Result<Infallible> {
        let mut token = self.authenticate().await?;
        loop {
            let revision = self.sync(&mut token).await?;
            *backoff = MIN_BACKOFF;
            self.wait_for_change(&mut token, revision).await?;
            tokio::time::sleep(DEBOUNCE).await;
        }
    }

    /// Reads the keys and sends the proxies. Returns the revision of the read.
    async fn sync(&self, token: &mut Option<String>) -> anyhow::Result<i64> {
        let body = json!({"key": self.range.key, "range_end": self.range.range_end});
        let response = self.post(token, "/v3/kv/range", &body).await?;
        let range: RangeResponse = read_json(response).await?;
        let mut built = Built::default();
        let mut groups = ProxyGroups::new(PROVIDER);
        let prefix = &self.range.prefix;
        let entries = kv_entries(&mut built, prefix, range.kvs);
        kv::add_kv(&mut built, &mut groups, &entries, prefix);
        built.proxies = groups.into_proxies();
        self.reporter.running(built).await?;
        Ok(range.header.revision)
    }

    /// Watches the keys from the revision after `revision`, and returns at the first change.
    async fn wait_for_change(
        &self,
        token: &mut Option<String>,
        revision: i64,
    ) -> anyhow::Result<()> {
        let body = json!({"create_request": {
            "key": self.range.key,
            "range_end": self.range.range_end,
            "start_revision": revision.saturating_add(1).to_string(),
            "progress_notify": true,
        }});
        let response = self.post(token, "/v3/watch", &body).await?;
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

    /// Returns a token when authentication is set.
    async fn authenticate(&self) -> anyhow::Result<Option<String>> {
        let Some((name, password)) = &self.credentials else {
            return Ok(None);
        };
        let body = json!({"name": name, "password": password});
        let response = self
            .send(None, "/v3/auth/authenticate", &body, &[])
            .await
            .context("etcd authentication failed")?;
        let auth: AuthenticateResponse = read_json(response).await?;
        Ok(Some(auth.token))
    }

    /// Sends a request with the token. An expired or revoked token is replaced once.
    async fn post(
        &self,
        token: &mut Option<String>,
        path: &str,
        body: &Value,
    ) -> anyhow::Result<Response<Incoming>> {
        let retry: &[StatusCode] = if self.credentials.is_some() {
            &[StatusCode::UNAUTHORIZED]
        } else {
            &[]
        };
        let response = self.send(token.as_deref(), path, body, retry).await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        *token = self.authenticate().await?;
        self.send(token.as_deref(), path, body, &[]).await
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
    fn the_range_covers_only_the_prefix_and_keys_are_decoded() {
        let range = KeyRange::new("/apps/r3v3rs3/");
        assert_eq!(range.prefix, "apps/r3v3rs3");
        assert_eq!(range.key, BASE64_STANDARD.encode("apps/r3v3rs3/"));
        assert_eq!(range.range_end, BASE64_STANDARD.encode("apps/r3v3rs30"));

        let response: RangeResponse = serde_json::from_value(json!({
            "header": {"revision": "12"},
            "kvs": [
                {"key": BASE64_STANDARD.encode("apps/r3v3rs3/http/app/ports"), "value": BASE64_STANDARD.encode("web")},
                {"key": "%%%", "value": BASE64_STANDARD.encode("web")},
            ],
        }))
        .unwrap();
        assert_eq!(response.header.revision, 12);
        let mut built = Built::default();
        let entries = kv_entries(&mut built, &range.prefix, response.kvs);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, "apps/r3v3rs3/http/app/ports");
        assert_eq!(built.issues[0].message, "the key is not UTF-8 text: %%%");

        let empty: RangeResponse = serde_json::from_value(json!({"header": {}})).unwrap();
        assert!(empty.kvs.is_empty());
        assert_eq!(empty.header.revision, 0);
    }
}
