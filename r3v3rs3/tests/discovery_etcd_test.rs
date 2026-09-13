use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use futures::future::ready;
use futures::stream::{self, StreamExt};
use r3v3rs3::config::storage::Storage;
use r3v3rs3::server::rpc::config::{GetConfig, SetConfig};
use r3v3rs3::server::rpc::discovery::GetDiscoveryStatus;
use r3v3rs3::server::ServerChannels;
use r3v3rs3_api::{app::AppConfig, discovery::DiscoveryState};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

mod common;
use common::{
    alloc_tcp_port, call, http_port_entry, serve_http_upstream, wait_for_discovery,
    wait_for_status, with_server, TestStorage,
};

const USER: &str = "root";
const PASSWORD: &str = "etcd-test-password";
const URL_KEY: &str = "r3v3rs3/http/app/routes/0/servers/0/url";

#[derive(Default)]
struct Data {
    kv: BTreeMap<String, String>,
    tokens: Vec<String>,
}

/// An etcd v3 JSON gateway with authentication, a range read and a watch stream. Each put raises
/// the revision.
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

    fn put(&self, key: &str, value: &str) {
        self.data
            .lock()
            .unwrap()
            .kv
            .insert(key.into(), value.into());
        self.revision.send_modify(|revision| *revision += 1);
    }

    fn revoke_tokens(&self) {
        self.data.lock().unwrap().tokens.clear();
    }

    fn authorized(&self, headers: &HeaderMap) -> bool {
        let token = headers.get("authorization").and_then(|v| v.to_str().ok());
        let data = self.data.lock().unwrap();
        token.is_some_and(|token| data.tokens.iter().any(|t| t == token))
    }

    fn header(&self) -> Value {
        json!({"cluster_id": "1", "revision": self.revision.borrow().to_string()})
    }

    fn router(&self) -> Router {
        Router::new()
            .route("/v3/auth/authenticate", post(authenticate))
            .route("/v3/kv/range", post(range))
            .route("/v3/watch", post(watch_keys))
            .with_state(self.clone())
    }
}

fn unauthorized() -> Response {
    let message = "etcdserver: invalid auth token";
    let body = json!({"error": message, "code": 16, "message": message});
    (StatusCode::UNAUTHORIZED, Json(body)).into_response()
}

fn decode(value: &Value) -> String {
    let bytes = BASE64_STANDARD
        .decode(value.as_str().unwrap_or_default())
        .unwrap_or_default();
    String::from_utf8(bytes).unwrap_or_default()
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
    let (start, end) = (decode(&body["key"]), decode(&body["range_end"]));
    let kvs = mock
        .data
        .lock()
        .unwrap()
        .kv
        .range(start..end)
        .map(|(key, value)| {
            json!({"key": BASE64_STANDARD.encode(key), "value": BASE64_STANDARD.encode(value)})
        })
        .collect::<Vec<_>>();
    let mut response = json!({"header": mock.header(), "count": kvs.len().to_string()});
    // The JSON gateway omits an empty list.
    if !kvs.is_empty() {
        response["kvs"] = json!(kvs);
    }
    Json(response).into_response()
}

/// Sends the created message, then one event message for each revision from the start revision.
async fn watch_keys(
    State(mock): State<MockEtcd>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !mock.authorized(&headers) {
        return unauthorized();
    }
    let start = body["create_request"]["start_revision"]
        .as_str()
        .and_then(|revision| revision.parse::<i64>().ok())
        .unwrap_or_default();
    let created = json!({"result": {"header": mock.header(), "created": true}});
    let events = WatchStream::new(mock.revision.subscribe())
        .filter(move |revision| ready(*revision >= start))
        .map(|revision| {
            let revision = revision.to_string();
            json!({"result": {
                "header": {"revision": revision},
                "events": [{"kv": {"mod_revision": revision}}],
            }})
        });
    let lines = stream::once(ready(created))
        .chain(events)
        .map(|message| Ok::<_, Infallible>(format!("{message}\n")));
    Body::from_stream(lines).into_response()
}

/// Waits until the proxy answers `expected`. The provider must not report an error meanwhile.
async fn wait_for_body(
    channels: &mut ServerChannels,
    url: &str,
    expected: &str,
) -> anyhow::Result<()> {
    for _ in 0..50 {
        let statuses = call(channels, GetDiscoveryStatus).await??;
        if let Some(failed) = statuses.iter().find(|s| s.state == DiscoveryState::Error) {
            anyhow::bail!("the provider reported an error: {failed:?}");
        }
        if reqwest::get(url).await?.text().await? == expected {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("the proxy did not answer {expected}")
}

#[tokio::test]
async fn etcd_keys_define_proxies_that_follow_changes() -> anyhow::Result<()> {
    let first = serve_http_upstream(Router::new().route("/", get(|| async { "first" }))).await?;
    let second = serve_http_upstream(Router::new().route("/", get(|| async { "second" }))).await?;
    let proxy_port = alloc_tcp_port().await?;
    let mut port = http_port_entry("test", &proxy_port);
    port.port.name = "web".into();

    let mock = MockEtcd::new();
    mock.put("r3v3rs3/http/app/ports", "web");
    mock.put(URL_KEY, first.as_str());
    mock.put("r3v3rs3x/http/other/ports", "web");
    let no_port = || anyhow::anyhow!("the URL has no port");
    let etcd_port = serve_http_upstream(mock.router())
        .await?
        .port()
        .ok_or_else(no_port)?;
    // The first endpoint refuses connections, so the provider uses the second endpoint.
    let closed_port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();

    let mut config = AppConfig::default();
    config.discovery.etcd.enabled = true;
    config.discovery.etcd.endpoints = vec![
        format!("http://127.0.0.1:{closed_port}"),
        format!("http://localhost:{etcd_port}"),
    ];
    config.discovery.etcd.username = USER.into();
    config.discovery.etcd.password = Some(PASSWORD.into());
    let storage = TestStorage::builder().ports(vec![port]).build();
    let saved = storage.clone();
    let url = proxy_port.http_url("/").to_string();
    with_server(storage, |mut channels| async move {
        call(&mut channels, SetConfig { config }).await??;
        let masked = call(&mut channels, GetConfig).await??;
        assert_eq!(masked.discovery.etcd.password, None);
        assert!(masked.discovery.etcd.password_set);
        let saved_password = saved.load_app_config().await.discovery.etcd.password;
        assert_eq!(saved_password.as_deref(), Some(PASSWORD));

        assert_eq!(wait_for_status(&url, 200).await?, "first");
        wait_for_discovery(&mut channels, |statuses| {
            statuses.iter().any(|status| {
                status.state == DiscoveryState::Running
                    && status.proxies == 1
                    && status.issues.is_empty()
            })
        })
        .await?;

        // The watch stream reports the changed key.
        mock.put(URL_KEY, second.as_str());
        wait_for_body(&mut channels, &url, "second").await?;

        // etcd rejects the token, so the provider authenticates again without an error.
        mock.revoke_tokens();
        mock.put(URL_KEY, first.as_str());
        wait_for_body(&mut channels, &url, "first").await?;
        assert_eq!(mock.authentications.load(Ordering::SeqCst), 2);
        Ok(())
    })
    .await
}
