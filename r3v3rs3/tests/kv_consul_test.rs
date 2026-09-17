//! The Consul store against a mock of the Consul HTTP API.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put as put_route};
use axum::{Json, Router};
use r3v3rs3::kv::consul::ConsulClient;
use r3v3rs3::kv::{ConsulStore, KvStore};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

mod common;
use common::kv::{
    api_client, check_conditional_commits, check_locks_and_leases, check_rekey,
    check_watch_changes, commit, next_changes, put,
};
use common::serve_http_upstream;

const TOKEN: &str = "consul-test-token";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    /// Base64 text.
    value: Option<String>,
    modify_index: u64,
    session: Option<String>,
}

type Kv = BTreeMap<String, Entry>;

#[derive(Default)]
struct Data {
    kv: Kv,
    sessions: Vec<String>,
    created_sessions: u64,
}

/// A Consul HTTP API with the key-value store, transactions, sessions, blocking queries and an ACL
/// token check. A transaction with a write raises the index once.
#[derive(Clone)]
struct MockConsul {
    data: Arc<Mutex<Data>>,
    index: Arc<watch::Sender<u64>>,
}

impl MockConsul {
    fn new() -> Self {
        Self {
            data: Arc::default(),
            index: Arc::new(watch::channel(1).0),
        }
    }

    fn router(&self) -> Router {
        Router::new()
            .route("/v1/kv/{*key}", get(kv))
            .route("/v1/txn", put_route(txn))
            .route("/v1/session/create", put_route(create_session))
            .route("/v1/session/renew/{id}", put_route(renew_session))
            .route("/v1/session/destroy/{id}", put_route(destroy_session))
            .with_state(self.clone())
    }

    /// Sets the index back, as a restore from an older snapshot does.
    fn restore_index(&self, index: u64) {
        self.index.send_replace(index);
    }
}

fn forbidden(headers: &HeaderMap) -> Option<Response> {
    let token = headers.get("x-consul-token").and_then(|v| v.to_str().ok());
    (token != Some(TOKEN)).then(|| (StatusCode::FORBIDDEN, "ACL not found").into_response())
}

fn kv_json(key: &str, entry: &Entry) -> Value {
    json!({
        "Key": key,
        "Value": entry.value,
        "ModifyIndex": entry.modify_index,
        "Session": entry.session,
    })
}

/// Checks one `check-*` operation of a transaction.
fn check(entry: Option<&Entry>, verb: &str, operation: &Value) -> Result<(), String> {
    let holds = match verb {
        "check-not-exists" => entry.is_none(),
        "check-index" => entry.map(|e| e.modify_index) == operation["Index"].as_u64(),
        "check-session" => {
            entry.and_then(|e| e.session.as_deref()) == operation["Session"].as_str()
        }
        _ => false,
    };
    holds.then_some(()).ok_or_else(|| format!("{verb} failed"))
}

/// The session of a `lock` operation. An unknown session, or a key that another session holds,
/// fails.
fn lock_session(
    sessions: &[String],
    holder: Option<String>,
    session: Option<&str>,
) -> Result<Option<String>, String> {
    let session = session
        .filter(|id| sessions.iter().any(|known| known == id))
        .ok_or("invalid session")?;
    match holder {
        Some(holder) if holder != session => Err(format!("the key is locked by {holder}")),
        _ => Ok(Some(session.to_string())),
    }
}

/// Applies one write operation of a transaction.
fn write(kv: &mut Kv, sessions: &[String], operation: &Value, index: u64) -> Result<(), String> {
    let key = operation["Key"].as_str().unwrap_or_default();
    let holder = kv.get(key).and_then(|entry| entry.session.clone());
    let session = match operation["Verb"].as_str().unwrap_or_default() {
        "delete" => {
            kv.remove(key);
            return Ok(());
        }
        "set" => holder,
        "lock" => lock_session(sessions, holder, operation["Session"].as_str())?,
        verb => return Err(format!("unknown verb {verb}")),
    };
    let entry = Entry {
        value: operation["Value"].as_str().map(str::to_string),
        modify_index: index,
        session,
    };
    kv.insert(key.to_string(), entry);
    Ok(())
}

/// Answers the keys of a path. A query with an index waits until the index changes, so it also
/// answers at once when the index went back.
async fn kv(
    State(mock): State<MockConsul>,
    Path(key): Path<String>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if let Some(response) = forbidden(&headers) {
        return response;
    }
    let mut index = mock.index.subscribe();
    if let Some(wanted) = query.get("index").and_then(|i| i.parse::<u64>().ok()) {
        let changed = index.wait_for(|current| *current != wanted);
        // At the end of the wait time, Consul answers with the same index.
        let _ = tokio::time::timeout(Duration::from_secs(30), changed).await;
    }
    let current = *index.borrow();
    let recurse = query.contains_key("recurse");
    let entries = mock
        .data
        .lock()
        .unwrap()
        .kv
        .iter()
        .filter(|(entry, _)| {
            if recurse {
                entry.starts_with(&key)
            } else {
                **entry == key
            }
        })
        .map(|(key, entry)| kv_json(key, entry))
        .collect::<Vec<_>>();
    let status = if entries.is_empty() {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::OK
    };
    let headers = [("X-Consul-Index", current.to_string())];
    (status, headers, Json(entries)).into_response()
}

/// Applies every operation, or none when an operation fails.
async fn txn(
    State(mock): State<MockConsul>,
    headers: HeaderMap,
    Json(operations): Json<Vec<Value>>,
) -> Response {
    if let Some(response) = forbidden(&headers) {
        return response;
    }
    let next = *mock.index.borrow() + 1;
    let mut data = mock.data.lock().unwrap();
    let mut kv = data.kv.clone();
    for (position, operation) in operations.iter().enumerate() {
        let operation = &operation["KV"];
        let verb = operation["Verb"].as_str().unwrap_or_default();
        let result = if verb.starts_with("check-") {
            let key = operation["Key"].as_str().unwrap_or_default();
            check(kv.get(key), verb, operation)
        } else {
            write(&mut kv, &data.sessions, operation, next)
        };
        if let Err(what) = result {
            let errors = json!({"Results": null, "Errors": [{"OpIndex": position, "What": what}]});
            return (StatusCode::CONFLICT, Json(errors)).into_response();
        }
    }
    let changed = kv != data.kv;
    data.kv = kv;
    drop(data);
    if changed {
        mock.index.send_replace(next);
    }
    Json(json!({"Results": [], "Errors": null})).into_response()
}

async fn create_session(
    State(mock): State<MockConsul>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if let Some(response) = forbidden(&headers) {
        return response;
    }
    if body["Behavior"] != "delete" {
        return (
            StatusCode::BAD_REQUEST,
            "the store needs the delete behavior",
        )
            .into_response();
    }
    let mut data = mock.data.lock().unwrap();
    data.created_sessions += 1;
    let id = format!("session-{}", data.created_sessions);
    data.sessions.push(id.clone());
    Json(json!({ "ID": id })).into_response()
}

async fn renew_session(
    State(mock): State<MockConsul>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = forbidden(&headers) {
        return response;
    }
    if !mock.data.lock().unwrap().sessions.contains(&id) {
        return (
            StatusCode::NOT_FOUND,
            format!("Session id '{id}' not found"),
        )
            .into_response();
    }
    Json(json!([{ "ID": id }])).into_response()
}

/// Ends the session and deletes the keys that it locks.
async fn destroy_session(
    State(mock): State<MockConsul>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = forbidden(&headers) {
        return response;
    }
    let next = *mock.index.borrow() + 1;
    let deleted = {
        let mut data = mock.data.lock().unwrap();
        data.sessions.retain(|known| *known != id);
        let count = data.kv.len();
        data.kv
            .retain(|_, entry| entry.session.as_deref() != Some(id.as_str()));
        count != data.kv.len()
    };
    if deleted {
        mock.index.send_replace(next);
    }
    Json(json!(true)).into_response()
}

async fn connect(mock: &MockConsul) -> anyhow::Result<ConsulStore> {
    let url = serve_http_upstream(mock.router()).await?;
    let client = ConsulClient::new(api_client(&url)?, Some(TOKEN));
    Ok(ConsulStore::new(client, ""))
}

#[tokio::test]
async fn consul_commit_applies_writes_only_when_the_conditions_hold() -> anyhow::Result<()> {
    check_conditional_commits(&connect(&MockConsul::new()).await?).await
}

#[tokio::test]
async fn consul_lock_belongs_to_one_session_until_the_session_is_destroyed() -> anyhow::Result<()> {
    let store = connect(&MockConsul::new()).await?;
    let short = store.grant_lease(Duration::from_secs(5)).await;
    assert!(short.is_err(), "{short:?}");
    check_locks_and_leases(&store).await
}

#[tokio::test]
async fn consul_rekey_encrypts_old_values_with_the_first_key() -> anyhow::Result<()> {
    check_rekey(&connect(&MockConsul::new()).await?).await
}

#[tokio::test]
async fn consul_rejects_a_transaction_with_more_than_64_operations() -> anyhow::Result<()> {
    let store = connect(&MockConsul::new()).await?;
    let writes = (0..65).map(|n| put(&format!("big/{n}"), "x")).collect();
    let Err(err) = commit(&store, vec![], writes).await else {
        anyhow::bail!("the transaction was sent");
    };
    assert!(err.to_string().contains("at most 64"), "{err}");
    assert!(store.list("big/").await?.items.is_empty());
    Ok(())
}

#[tokio::test]
async fn consul_watch_reports_changes_and_resyncs_when_the_index_goes_back() -> anyhow::Result<()> {
    let mock = MockConsul::new();
    let store = connect(&mock).await?;
    let (_, mut watcher) = check_watch_changes(&store).await?;

    mock.restore_index(2);
    assert_eq!(next_changes(watcher.as_mut()).await?, ["resync w/b"]);
    commit(&store, vec![], vec![put("w/c", "4")]).await?;
    assert_eq!(next_changes(watcher.as_mut()).await?, ["put w/c=4"]);
    Ok(())
}
