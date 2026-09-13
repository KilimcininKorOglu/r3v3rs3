use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{routing::get, Json, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use r3v3rs3::config::storage::Storage;
use r3v3rs3::server::rpc::config::{GetConfig, SetConfig};
use r3v3rs3_api::{
    app::AppConfig,
    discovery::{DiscoveryIssue, DiscoveryState},
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

mod common;
use common::{
    alloc_tcp_port, call, http_port_entry, serve_http_upstream, wait_for_discovery,
    wait_for_host_status, wait_for_status, with_server, TestStorage,
};

const TOKEN: &str = "consul-test-token";

type Params = Query<HashMap<String, String>>;

#[derive(Default)]
struct Data {
    /// Tags by service name.
    services: BTreeMap<String, Vec<String>>,
    /// Passing instances by service name.
    passing: BTreeMap<String, Vec<Value>>,
    kv: Vec<(String, String)>,
}

/// A Consul HTTP API with blocking queries and an ACL token check. Each change raises the index.
#[derive(Clone)]
struct MockConsul {
    data: Arc<Mutex<Data>>,
    index: Arc<watch::Sender<u64>>,
    health_requests: Arc<AtomicUsize>,
}

impl MockConsul {
    fn new() -> Self {
        Self {
            data: Arc::default(),
            index: Arc::new(watch::channel(1).0),
            health_requests: Arc::default(),
        }
    }

    fn update(&self, change: impl FnOnce(&mut Data)) {
        change(&mut self.data.lock().unwrap());
        self.index.send_modify(|index| *index += 1);
    }

    fn router(&self) -> Router {
        Router::new()
            .route("/v1/catalog/services", get(services))
            .route("/v1/health/state/any", get(health))
            .route("/v1/health/service/{name}", get(service))
            .route("/v1/kv/{*key}", get(kv))
            .with_state(self.clone())
    }

    /// Holds a query with an index until the index passes it. `None` answers 404.
    async fn respond(
        &self,
        headers: &HeaderMap,
        query: &HashMap<String, String>,
        body: impl FnOnce(&Data) -> Option<Value>,
    ) -> Response {
        let token = headers.get("x-consul-token").and_then(|v| v.to_str().ok());
        if token != Some(TOKEN) {
            return (StatusCode::FORBIDDEN, "ACL not found").into_response();
        }
        let mut index = self.index.subscribe();
        if let Some(wanted) = query.get("index").and_then(|i| i.parse::<u64>().ok()) {
            let changed = index.wait_for(|current| *current > wanted);
            let _ = tokio::time::timeout(Duration::from_secs(30), changed).await;
        }
        let current = *index.borrow();
        let body = body(&self.data.lock().unwrap());
        let status = match body {
            Some(_) => StatusCode::OK,
            None => StatusCode::NOT_FOUND,
        };
        let headers = [("X-Consul-Index", current.to_string())];
        (status, headers, Json(body.unwrap_or(Value::Null))).into_response()
    }
}

async fn services(State(mock): State<MockConsul>, headers: HeaderMap, query: Params) -> Response {
    mock.respond(&headers, &query, |data| Some(json!(data.services)))
        .await
}

async fn health(State(mock): State<MockConsul>, headers: HeaderMap, query: Params) -> Response {
    mock.health_requests.fetch_add(1, Ordering::SeqCst);
    mock.respond(&headers, &query, |_| Some(json!([]))).await
}

async fn service(
    State(mock): State<MockConsul>,
    Path(name): Path<String>,
    headers: HeaderMap,
    query: Params,
) -> Response {
    mock.respond(&headers, &query, move |data| {
        Some(json!(data.passing.get(&name).cloned().unwrap_or_default()))
    })
    .await
}

async fn kv(
    State(mock): State<MockConsul>,
    Path(key): Path<String>,
    headers: HeaderMap,
    query: Params,
) -> Response {
    mock.respond(&headers, &query, move |data| {
        let entries = data
            .kv
            .iter()
            .filter(|(entry, _)| entry.starts_with(&key))
            .map(|(entry, value)| json!({"Key": entry, "Value": BASE64_STANDARD.encode(value)}))
            .collect::<Vec<_>>();
        (!entries.is_empty()).then(|| json!(entries))
    })
    .await
}

fn instance(tags: &[&str], port: u16) -> Value {
    json!({
        "Node": {"Address": "127.0.0.1"},
        "Service": {"Tags": tags, "Address": "localhost", "Port": port},
    })
}

#[tokio::test]
async fn consul_services_and_keys_define_proxies() -> anyhow::Result<()> {
    let catalog_upstream =
        serve_http_upstream(Router::new().route("/", get(|| async { "catalog" }))).await?;
    let kv_upstream = serve_http_upstream(Router::new().route("/", get(|| async { "kv" }))).await?;
    let no_port = || anyhow::anyhow!("the URL has no port");
    let catalog_port = catalog_upstream.port().ok_or_else(no_port)?;
    let proxy_port = alloc_tcp_port().await?;
    let mut port = http_port_entry("test", &proxy_port);
    port.port.name = "web".into();

    let mock = MockConsul::new();
    mock.update(|data| {
        let tags = ["r3v3rs3.enable=true", "r3v3rs3.http.app.ports=web"];
        data.services
            .insert("web".into(), tags.map(String::from).to_vec());
        data.passing
            .insert("web".into(), vec![instance(&tags, catalog_port)]);
        data.kv = vec![
            ("r3v3rs3/http/kv/ports".into(), "web".into()),
            ("r3v3rs3/http/kv/vhosts".into(), "kv.test".into()),
            (
                "r3v3rs3/http/kv/routes/0/servers/0/url".into(),
                kv_upstream.to_string(),
            ),
        ];
    });
    let consul_port = serve_http_upstream(mock.router())
        .await?
        .port()
        .ok_or_else(no_port)?;

    let mut config = AppConfig::default();
    config.discovery.consul.enabled = true;
    config.discovery.consul.address = format!("http://localhost:{consul_port}");
    config.discovery.consul.token = Some(TOKEN.into());
    let storage = TestStorage::builder().ports(vec![port]).build();
    let saved = storage.clone();
    let url = proxy_port.http_url("/").to_string();
    with_server(storage, |mut channels| async move {
        call(&mut channels, SetConfig { config }).await??;
        let mut update = call(&mut channels, GetConfig).await??;
        assert_eq!(update.discovery.consul.token, None);
        assert!(update.discovery.consul.token_set);
        let saved_token = || async { saved.load_app_config().await.discovery.consul.token };
        assert_eq!(saved_token().await.as_deref(), Some(TOKEN));

        assert_eq!(wait_for_status(&url, 200).await?, "catalog");
        let kv_body = wait_for_host_status(&url, Some("kv.test"), 200).await?;
        assert_eq!(kv_body, "kv");
        wait_for_discovery(&mut channels, |statuses| {
            statuses
                .iter()
                .any(|status| status.state == DiscoveryState::Running && status.proxies == 2)
        })
        .await?;

        // Blocking queries wait for a change instead of asking again.
        let requests = mock.health_requests.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(mock.health_requests.load(Ordering::SeqCst) - requests <= 1);

        // The instance fails its health check, so its proxy is removed.
        mock.update(|data| {
            data.passing.insert("web".into(), Vec::new());
        });
        let statuses = wait_for_discovery(&mut channels, |statuses| {
            statuses
                .iter()
                .any(|status| status.proxies == 1 && !status.issues.is_empty())
        })
        .await?;
        assert_eq!(
            statuses[0].issues,
            [DiscoveryIssue {
                resource: "web".into(),
                message: "the service has no passing instance".into(),
            }]
        );
        wait_for_status(&url, 502).await?;

        // An update without a token keeps the saved token.
        update.discovery.consul.exposed_by_default = true;
        call(
            &mut channels,
            SetConfig {
                config: update.clone(),
            },
        )
        .await??;
        assert_eq!(saved_token().await.as_deref(), Some(TOKEN));
        wait_for_discovery(&mut channels, |statuses| {
            statuses
                .iter()
                .any(|status| status.state == DiscoveryState::Running && status.proxies == 1)
        })
        .await?;

        // An empty token removes the saved token, so Consul rejects the requests.
        update.discovery.consul.token = Some(String::new());
        call(&mut channels, SetConfig { config: update }).await??;
        assert_eq!(saved_token().await, None);
        let statuses = wait_for_discovery(&mut channels, |statuses| {
            statuses
                .iter()
                .any(|status| status.state == DiscoveryState::Error)
        })
        .await?;
        let error = statuses[0].error.clone().unwrap_or_default();
        assert!(error.contains("403"), "{error}");
        Ok(())
    })
    .await
}
