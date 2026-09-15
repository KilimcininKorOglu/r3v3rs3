//! The etcd store against a mock of the etcd v3 JSON gateway.

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use futures::future::ready;
use futures::stream::{self, Stream, StreamExt};
use r3v3rs3::kv::etcd::EtcdClient;
use r3v3rs3::kv::KvStore;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

mod common;
use common::kv::{
    api_client, check_conditional_commits, check_locks_and_leases, check_rekey,
    check_watch_changes, commit, next_changes, put,
};
use common::serve_http_upstream;

const USER: &str = "root";
const PASSWORD: &str = "etcd-test-password";

#[derive(Debug, Clone)]
struct Entry {
    /// Base64 text.
    value: String,
    create: i64,
    modified: i64,
    lease: i64,
}

/// A change: the revision, the key, and the new entry or `None` for a delete.
type Change = (i64, Vec<u8>, Option<Entry>);

#[derive(Default)]
struct Data {
    kv: BTreeMap<Vec<u8>, Entry>,
    history: Vec<Change>,
    compacted: i64,
    /// The TTL of each lease.
    leases: BTreeMap<i64, i64>,
    last_lease: i64,
    tokens: Vec<String>,
}

/// An etcd v3 JSON gateway with authentication, transactions, leases and watch streams. A
/// transaction with a write raises the revision once.
#[derive(Clone)]
struct MockEtcd {
    data: Arc<Mutex<Data>>,
    revision: Arc<watch::Sender<i64>>,
    authentications: Arc<AtomicUsize>,
}

impl MockEtcd {
    fn new() -> Self {
        Self {
            data: Arc::default(),
            revision: Arc::new(watch::channel(1).0),
            authentications: Arc::default(),
        }
    }

    fn router(&self) -> Router {
        Router::new()
            .route("/v3/auth/authenticate", post(authenticate))
            .route("/v3/kv/range", post(range))
            .route("/v3/kv/txn", post(txn))
            .route("/v3/lease/grant", post(grant))
            .route("/v3/lease/keepalive", post(keep_alive))
            .route("/v3/lease/revoke", post(revoke))
            .route("/v3/watch", post(watch_keys))
            .with_state(self.clone())
    }

    fn revoke_tokens(&self) {
        self.data.lock().unwrap().tokens.clear();
    }

    /// Removes the history up to the current revision.
    fn compact(&self) {
        let revision = *self.revision.borrow();
        let mut data = self.data.lock().unwrap();
        data.compacted = revision;
        data.history.retain(|(changed, _, _)| *changed > revision);
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let token = headers.get("authorization").and_then(|v| v.to_str().ok());
        let data = self.data.lock().unwrap();
        token.is_some_and(|token| data.tokens.iter().any(|t| t == token))
    }

    fn header(&self) -> Value {
        json!({"revision": self.revision.borrow().to_string()})
    }

    /// The events of the first revision from `next` that changed a key of the range.
    fn events(&self, next: i64, start: &[u8], end: Option<&[u8]>) -> Option<(i64, Value)> {
        let data = self.data.lock().unwrap();
        let matching = |change: &&Change| change.0 >= next && in_range(&change.1, start, end);
        let revision = data.history.iter().filter(matching).map(|c| c.0).min()?;
        let events = data
            .history
            .iter()
            .filter(matching)
            .filter(|change| change.0 == revision)
            .map(event_json)
            .collect::<Vec<_>>();
        let message = json!({"result": {
            "header": {"revision": revision.to_string()},
            "events": events,
        }});
        Some((revision, message))
    }
}

fn unauthorized() -> Response {
    let message = "etcdserver: invalid auth token";
    let body = json!({"error": message, "code": 16, "message": message});
    (StatusCode::UNAUTHORIZED, Json(body)).into_response()
}

fn bytes(value: &Value) -> Vec<u8> {
    BASE64_STANDARD
        .decode(value.as_str().unwrap_or_default())
        .unwrap_or_default()
}

/// An int64 field as a JSON string or a number.
fn number(value: &Value) -> i64 {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        .unwrap_or_default()
}

fn ops(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

fn in_range(key: &[u8], start: &[u8], end: Option<&[u8]>) -> bool {
    match end {
        None => key == start,
        Some([0]) => key >= start,
        Some(end) => key >= start && key < end,
    }
}

fn kv_json(key: &[u8], entry: &Entry) -> Value {
    json!({
        "key": BASE64_STANDARD.encode(key),
        "value": entry.value,
        "create_revision": entry.create.to_string(),
        "mod_revision": entry.modified.to_string(),
        "lease": entry.lease.to_string(),
    })
}

fn event_json(change: &Change) -> Value {
    let (revision, key, entry) = change;
    match entry {
        Some(entry) => json!({"kv": kv_json(key, entry)}),
        None => json!({"type": "DELETE", "kv": {
            "key": BASE64_STANDARD.encode(key),
            "mod_revision": revision.to_string(),
        }}),
    }
}

/// Whether the key of the comparison has the expected create revision, mod revision or lease.
fn holds(data: &Data, compare: &Value) -> bool {
    let entry = data.kv.get(&bytes(&compare["key"]));
    let (actual, field) = match compare["target"].as_str() {
        Some("CREATE") => (entry.map_or(0, |e| e.create), "create_revision"),
        Some("MOD") => (entry.map_or(0, |e| e.modified), "mod_revision"),
        Some("LEASE") => (entry.map_or(0, |e| e.lease), "lease"),
        _ => return false,
    };
    compare["result"] == "EQUAL" && actual == number(&compare[field])
}

/// Applies one operation of a transaction, and returns its response and whether it wrote.
fn apply(data: &mut Data, op: &Value, revision: i64) -> (Value, bool) {
    if let Some(put) = op.get("request_put") {
        let key = bytes(&put["key"]);
        let current = data.kv.get(&key);
        let create = current.map_or(revision, |entry| entry.create);
        let lease = if put["ignore_lease"] == true {
            current.map_or(0, |entry| entry.lease)
        } else {
            number(&put["lease"])
        };
        let entry = Entry {
            value: put["value"].as_str().unwrap_or_default().to_string(),
            create,
            modified: revision,
            lease,
        };
        data.history
            .push((revision, key.clone(), Some(entry.clone())));
        data.kv.insert(key, entry);
        return (json!({"response_put": {}}), true);
    }
    if let Some(delete) = op.get("request_delete_range") {
        let key = bytes(&delete["key"]);
        let deleted = data.kv.remove(&key).is_some();
        if deleted {
            data.history.push((revision, key, None));
        }
        return (json!({"response_delete_range": {}}), deleted);
    }
    let key = bytes(&op["request_range"]["key"]);
    let kvs = data
        .kv
        .get(&key)
        .map(|entry| vec![kv_json(&key, entry)])
        .unwrap_or_default();
    (json!({"response_range": {"kvs": kvs}}), false)
}

fn lines(messages: impl Stream<Item = Value> + Send + 'static) -> Response {
    let lines = messages.map(|message| Ok::<_, Infallible>(format!("{message}\n")));
    Body::from_stream(lines).into_response()
}

async fn authenticate(State(mock): State<MockEtcd>, Json(body): Json<Value>) -> Response {
    if body != json!({"name": USER, "password": PASSWORD}) {
        let message = "etcdserver: authentication failed, invalid user ID or password";
        return (StatusCode::BAD_REQUEST, Json(json!({"error": message}))).into_response();
    }
    let count = mock.authentications.fetch_add(1, Ordering::SeqCst) + 1;
    let token = format!("token-{count}");
    mock.data.lock().unwrap().tokens.push(token.clone());
    Json(json!({"header": mock.header(), "token": token})).into_response()
}

async fn range(
    State(mock): State<MockEtcd>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !mock.authorized(&headers) {
        return unauthorized();
    }
    let start = bytes(&body["key"]);
    let end = body.get("range_end").map(bytes);
    let kvs = mock
        .data
        .lock()
        .unwrap()
        .kv
        .iter()
        .filter(|(key, _)| in_range(key, &start, end.as_deref()))
        .map(|(key, entry)| kv_json(key, entry))
        .collect::<Vec<_>>();
    let mut response = json!({"header": mock.header()});
    // The JSON gateway omits an empty list.
    if !kvs.is_empty() {
        response["kvs"] = json!(kvs);
    }
    Json(response).into_response()
}

async fn txn(
    State(mock): State<MockEtcd>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !mock.authorized(&headers) {
        return unauthorized();
    }
    let next = *mock.revision.borrow() + 1;
    let mut data = mock.data.lock().unwrap();
    let succeeded = ops(&body["compare"])
        .iter()
        .all(|compare| holds(&data, compare));
    let branch = if succeeded { "success" } else { "failure" };
    let mut wrote = false;
    let mut responses = Vec::new();
    for op in ops(&body[branch]) {
        let (response, changed) = apply(&mut data, &op, next);
        responses.push(response);
        wrote |= changed;
    }
    drop(data);
    if wrote {
        mock.revision.send_replace(next);
    }
    let mut response = json!({"header": mock.header(), "responses": responses});
    // The JSON gateway omits `false`.
    if succeeded {
        response["succeeded"] = json!(true);
    }
    Json(response).into_response()
}

async fn grant(
    State(mock): State<MockEtcd>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !mock.authorized(&headers) {
        return unauthorized();
    }
    let ttl = number(&body["TTL"]);
    let id = {
        let mut data = mock.data.lock().unwrap();
        data.last_lease += 1;
        let id = 7_000 + data.last_lease;
        data.leases.insert(id, ttl);
        id
    };
    Json(json!({"header": mock.header(), "ID": id.to_string(), "TTL": ttl.to_string()}))
        .into_response()
}

/// Answers one message of the keepalive stream. A lease that ended has the TTL 0.
async fn keep_alive(
    State(mock): State<MockEtcd>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !mock.authorized(&headers) {
        return unauthorized();
    }
    let id = number(&body["ID"]);
    let ttl = mock.data.lock().unwrap().leases.get(&id).copied();
    let result = json!({"header": mock.header(), "ID": id.to_string(), "TTL": ttl.unwrap_or_default().to_string()});
    lines(stream::iter([json!({ "result": result })]))
}

/// Ends the lease and deletes its keys.
async fn revoke(
    State(mock): State<MockEtcd>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !mock.authorized(&headers) {
        return unauthorized();
    }
    let id = number(&body["ID"]);
    let next = *mock.revision.borrow() + 1;
    let mut data = mock.data.lock().unwrap();
    if data.leases.remove(&id).is_none() {
        let body = json!({"error": "etcdserver: requested lease not found", "code": 5});
        return (StatusCode::NOT_FOUND, Json(body)).into_response();
    }
    let keys = data
        .kv
        .iter()
        .filter(|(_, entry)| entry.lease == id)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    for key in &keys {
        data.kv.remove(key);
        data.history.push((next, key.clone(), None));
    }
    drop(data);
    if !keys.is_empty() {
        mock.revision.send_replace(next);
    }
    Json(json!({"header": mock.header()})).into_response()
}

/// Sends the created message, then one message for each revision from the start revision that
/// changed a key of the range. A compacted start revision cancels the watch.
async fn watch_keys(
    State(mock): State<MockEtcd>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !mock.authorized(&headers) {
        return unauthorized();
    }
    let request = &body["create_request"];
    let start = bytes(&request["key"]);
    let end = request.get("range_end").map(bytes);
    let from = number(&request["start_revision"]);
    let created = json!({"result": {"header": mock.header(), "created": true}});
    let compacted = mock.data.lock().unwrap().compacted;
    if from <= compacted {
        let canceled = json!({"result": {
            "header": mock.header(),
            "canceled": true,
            "compact_revision": compacted.to_string(),
        }});
        return lines(stream::iter([created, canceled]));
    }
    let receiver = mock.revision.subscribe();
    let events = stream::unfold((mock, receiver, from), move |(mock, mut receiver, next)| {
        let (start, end) = (start.clone(), end.clone());
        async move {
            loop {
                if let Some((revision, message)) = mock.events(next, &start, end.as_deref()) {
                    return Some((message, (mock, receiver, revision + 1)));
                }
                receiver.changed().await.ok()?;
            }
        }
    });
    lines(stream::once(ready(created)).chain(events))
}

async fn connect(mock: &MockEtcd) -> anyhow::Result<EtcdClient> {
    let url = serve_http_upstream(mock.router()).await?;
    Ok(EtcdClient::new(api_client(&url)?, USER, Some(PASSWORD)))
}

#[tokio::test]
async fn etcd_commit_applies_writes_only_when_the_conditions_hold() -> anyhow::Result<()> {
    check_conditional_commits(&connect(&MockEtcd::new()).await?).await
}

#[tokio::test]
async fn etcd_lock_belongs_to_one_lease_until_the_lease_is_revoked() -> anyhow::Result<()> {
    check_locks_and_leases(&connect(&MockEtcd::new()).await?).await
}

#[tokio::test]
async fn etcd_rekey_encrypts_old_values_with_the_first_key() -> anyhow::Result<()> {
    check_rekey(&connect(&MockEtcd::new()).await?).await
}

#[tokio::test]
async fn etcd_store_authenticates_again_after_the_token_is_revoked() -> anyhow::Result<()> {
    let mock = MockEtcd::new();
    let store = connect(&mock).await?;
    store.authenticate().await?;
    commit(&store, vec![], vec![put("t/a", "1")]).await?;

    mock.revoke_tokens();
    assert!(store.get("t/a").await?.is_some());
    assert_eq!(mock.authentications.load(Ordering::SeqCst), 2);
    Ok(())
}

#[tokio::test]
async fn etcd_watch_decodes_changes_and_resyncs_after_a_compaction() -> anyhow::Result<()> {
    let mock = MockEtcd::new();
    let store = connect(&mock).await?;
    let (from, _) = check_watch_changes(&store).await?;

    // The history after the first list is gone, so a watcher from that list reads the keys again.
    mock.compact();
    let mut stale = store.watch("w/", &from);
    assert_eq!(next_changes(stale.as_mut()).await?, ["resync w/b"]);
    commit(&store, vec![], vec![put("w/c", "4")]).await?;
    assert_eq!(next_changes(stale.as_mut()).await?, ["put w/c=4"]);
    Ok(())
}
