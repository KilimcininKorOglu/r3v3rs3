use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use futures::future::ready;
use futures::stream::{self, StreamExt};
use r3v3rs3::certs::Cert;
use r3v3rs3::config::storage::Storage;
use r3v3rs3::server::rpc::certs::{DeleteCert, GetCertList};
use r3v3rs3::server::rpc::config::SetConfig;
use r3v3rs3::server::rpc::proxies::GetProxyList;
use r3v3rs3::server::ServerChannels;
use r3v3rs3_api::discovery::{DiscoveryProvider, DiscoveryState};
use r3v3rs3_api::{app::AppConfig, error::Error, proxy::ProxyKind, subject_name::SubjectName};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;

mod common;
use common::{
    alloc_tcp_port, call, http_port_entry, serve_http_upstream, wait_for_discovery,
    wait_for_host_body, wait_for_host_status, with_server, TestStorage,
};

const TOKEN: &str = "mock-token";
const HOST: &str = "app.example.com";
const CRD_HOST: &str = "crd.example.com";
const COLLECTIONS: [(&str, &str); 5] = [
    ("/apis/networking.k8s.io/v1/ingresses", "ingresses"),
    ("/apis/r3v3rs3.io/v1/r3v3rs3proxies", "r3v3rs3proxies"),
    ("/api/v1/services", "services"),
    ("/apis/discovery.k8s.io/v1/endpointslices", "endpointslices"),
    ("/api/v1/secrets", "secrets"),
];

/// The objects of a collection by namespace and name, and its change events by revision.
#[derive(Default)]
struct Collection {
    objects: BTreeMap<String, Value>,
    events: Vec<(u64, Value)>,
}

/// A Kubernetes API with list and watch requests. Each change raises the resource version.
#[derive(Clone)]
struct MockKube {
    data: Arc<Mutex<HashMap<&'static str, Collection>>>,
    revision: Arc<watch::Sender<u64>>,
}

impl MockKube {
    fn new() -> Self {
        Self {
            data: Arc::default(),
            revision: Arc::new(watch::channel(1).0),
        }
    }

    fn put(&self, collection: &'static str, object: Value) {
        self.change(collection, object, false);
    }

    fn delete(&self, collection: &'static str, object: Value) {
        self.change(collection, object, true);
    }

    fn change(&self, collection: &'static str, mut object: Value, delete: bool) {
        let revision = *self.revision.borrow() + 1;
        object["metadata"]["resourceVersion"] = json!(revision.to_string());
        let key = format!(
            "{}/{}",
            object["metadata"]["namespace"].as_str().unwrap_or_default(),
            object["metadata"]["name"].as_str().unwrap_or_default()
        );
        {
            let mut data = self.data.lock().unwrap();
            let entry = data.entry(collection).or_default();
            let kind = if delete {
                entry.objects.remove(&key);
                "DELETED"
            } else if entry.objects.insert(key, object.clone()).is_some() {
                "MODIFIED"
            } else {
                "ADDED"
            };
            let event = json!({"type": kind, "object": object});
            entry.events.push((revision, event));
        }
        self.revision.send_replace(revision);
    }

    fn router(&self) -> Router {
        COLLECTIONS
            .into_iter()
            .fold(Router::new(), |router, (path, collection)| {
                router.route(
                    path,
                    get(
                        move |state: State<MockKube>, headers: HeaderMap, query: Query<_>| {
                            list_or_watch(state, headers, query, collection)
                        },
                    ),
                )
            })
            .with_state(self.clone())
    }
}

async fn list_or_watch(
    State(mock): State<MockKube>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
    collection: &'static str,
) -> Response {
    let token = headers.get("authorization").and_then(|v| v.to_str().ok());
    if token != Some(&format!("Bearer {TOKEN}")) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !query.get("watch").is_some_and(|v| v == "true" || v == "1") {
        let items = mock
            .data
            .lock()
            .unwrap()
            .get(collection)
            .map(|c| c.objects.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        let revision = mock.revision.borrow().to_string();
        let list = json!({"kind": "List", "apiVersion": "v1", "metadata": {"resourceVersion": revision}, "items": items});
        return Json(list).into_response();
    }
    let start = query
        .get("resourceVersion")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or_default();
    let data = mock.data.clone();
    let lines = WatchStream::new(mock.revision.subscribe())
        .scan(start, move |sent, revision| {
            let events = data
                .lock()
                .unwrap()
                .get(collection)
                .map(|c| {
                    c.events
                        .iter()
                        .filter(|(rev, _)| *rev > *sent && *rev <= revision)
                        .map(|(_, event)| format!("{event}\n"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            *sent = (*sent).max(revision);
            ready(Some(stream::iter(events)))
        })
        .flatten()
        .map(Ok::<_, Infallible>);
    Body::from_stream(lines).into_response()
}

fn metadata(name: &str) -> Value {
    json!({"namespace": "default", "name": name})
}

fn service() -> Value {
    json!({
        "apiVersion": "v1", "kind": "Service", "metadata": metadata("app"),
        "spec": {"ports": [{"name": "http", "port": 80}]},
    })
}

/// The slice of the `app` Service with one ready endpoint on `port` and one endpoint that is not
/// ready.
fn slice(port: u16) -> Value {
    json!({
        "apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSlice",
        "metadata": {"namespace": "default", "name": "app-1", "labels": {"kubernetes.io/service-name": "app"}},
        "addressType": "FQDN",
        "ports": [{"name": "http", "port": port}],
        "endpoints": [
            {"addresses": ["localhost"], "conditions": {"ready": true}},
            {"addresses": ["not-ready.invalid"], "conditions": {"ready": false}},
        ],
    })
}

fn ingress(class: &str) -> Value {
    let backend = |name: &str, port: Value| json!({"service": {"name": name, "port": port}});
    json!({
        "apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
        "metadata": {"namespace": "default", "name": "app", "annotations": {"r3v3rs3.io/ports": "web"}},
        "spec": {
            "ingressClassName": class,
            "tls": [{"hosts": [HOST], "secretName": "app-tls"}],
            "rules": [{"host": HOST, "http": {"paths": [
                {"path": "/", "pathType": "Prefix", "backend": backend("app", json!({"name": "http"}))},
                {"path": "/idle", "pathType": "Prefix", "backend": backend("idle", json!({"number": 80}))},
            ]}}],
        },
    })
}

/// An R3v3rs3Proxy resource that routes `crd.example.com` to the `app` Service.
fn crd_proxy() -> Value {
    json!({
        "apiVersion": "r3v3rs3.io/v1", "kind": "R3v3rs3Proxy", "metadata": metadata("crd"),
        "spec": {
            "ports": ["web"], "vhosts": [CRD_HOST],
            "routes": [{"path": "/", "service": {"name": "app", "port": "http"}}],
        },
    })
}

fn tls_secret(cert: &Cert) -> anyhow::Result<Value> {
    let key = cert
        .pem_key
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("no key"))?;
    Ok(json!({
        "apiVersion": "v1", "kind": "Secret", "metadata": metadata("app-tls"),
        "type": "kubernetes.io/tls",
        "data": {"tls.crt": BASE64_STANDARD.encode(&cert.pem_chain), "tls.key": BASE64_STANDARD.encode(key)},
    }))
}

fn kubeconfig(server: &str) -> Value {
    json!({
        "apiVersion": "v1", "kind": "Config", "current-context": "mock",
        "clusters": [{"name": "mock", "cluster": {"server": server}}],
        "users": [{"name": "mock", "user": {"token": TOKEN}}],
        "contexts": [{"name": "mock", "context": {"cluster": "mock", "user": "mock"}}],
    })
}

/// The server URLs of the first route of the proxy that the resource defines.
async fn first_route_servers(
    channels: &mut ServerChannels,
    resource: &str,
) -> anyhow::Result<Vec<String>> {
    let entries = call(channels, GetProxyList).await??;
    let entry = entries
        .into_iter()
        .find(|entry| {
            entry
                .source
                .as_ref()
                .is_some_and(|s| s.resource == resource)
        })
        .ok_or_else(|| anyhow::anyhow!("no proxy of {resource}"))?;
    let ProxyKind::Http(http) = entry.proxy.kind else {
        anyhow::bail!("the discovered proxy is not an HTTP proxy");
    };
    Ok(http.routes[0]
        .servers
        .iter()
        .map(|server| server.url.to_string())
        .collect())
}

#[tokio::test]
async fn ingress_and_custom_resources_define_proxies_that_follow_changes() -> anyhow::Result<()> {
    let first = serve_http_upstream(Router::new().route("/", get(|| async { "first" }))).await?;
    let second = serve_http_upstream(Router::new().route("/", get(|| async { "second" }))).await?;
    let port_of = |url: &url::Url| url.port().ok_or_else(|| anyhow::anyhow!("no port"));
    let proxy_port = alloc_tcp_port().await?;
    let mut port = http_port_entry("test", &proxy_port);
    port.port.name = "web".into();

    let ca = Cert::new_ca()?;
    let cert = Cert::new_self_signed(&[SubjectName::from_str(HOST)?], &ca)?;
    let mock = MockKube::new();
    mock.put("services", service());
    mock.put("endpointslices", slice(port_of(&first)?));
    mock.put("secrets", tls_secret(&cert)?);
    mock.put("ingresses", ingress("r3v3rs3"));
    mock.put("ingresses", {
        let mut other = ingress("nginx");
        other["metadata"]["name"] = json!("other");
        other
    });
    mock.put("r3v3rs3proxies", crd_proxy());
    let api = serve_http_upstream(mock.router()).await?;
    let kubeconfig_path =
        std::env::temp_dir().join(format!("r3v3rs3-kubeconfig-{}.json", port_of(&api)?));
    let server = api.as_str().trim_end_matches('/');
    std::fs::write(&kubeconfig_path, kubeconfig(server).to_string())?;

    let mut config = AppConfig::default();
    config.discovery.kubernetes.enabled = true;
    config.discovery.kubernetes.kubeconfig = kubeconfig_path.to_string_lossy().into_owned();
    config.discovery.kubernetes.ingress_class = "r3v3rs3".into();
    config.discovery.kubernetes.crd = true;
    let storage = TestStorage::builder().ports(vec![port]).build();
    let saved = storage.clone();
    let url = proxy_port.http_url("/").to_string();
    let idle_url = proxy_port.http_url("/idle").to_string();
    let result = with_server(storage, |mut channels| async move {
        call(&mut channels, SetConfig { config }).await??;
        assert_eq!(wait_for_host_status(&url, Some(HOST), 200).await?, "first");
        let crd_body = wait_for_host_status(&url, Some(CRD_HOST), 200).await?;
        assert_eq!(crd_body, "first");
        let statuses = wait_for_discovery(&mut channels, |statuses| {
            statuses.iter().any(|status| {
                status.provider == DiscoveryProvider::Kubernetes
                    && status.state == DiscoveryState::Running
                    && status.proxies == 2
            })
        })
        .await?;
        let issues = statuses[0]
            .issues
            .iter()
            .map(|issue| format!("{}: {}", issue.resource, issue.message))
            .collect::<Vec<_>>();
        assert_eq!(
            issues,
            ["ingress default/app: app.example.com/idle: service not found: default/idle"]
        );

        // Only the ready endpoint is a server.
        let servers = first_route_servers(&mut channels, "ingress default/app").await?;
        assert_eq!(servers, [first.as_str()]);
        let servers = first_route_servers(&mut channels, "r3v3rs3proxy default/crd").await?;
        assert_eq!(servers, [first.as_str()]);

        // The route of a backend without endpoints stays, so its requests fail.
        let idle = reqwest::Client::new()
            .get(&idle_url)
            .header("host", HOST)
            .send()
            .await?;
        assert!(idle.status().is_server_error(), "{}", idle.status());

        // The TLS secret is a read-only certificate that is not saved.
        let certs = call(&mut channels, GetCertList).await??;
        let discovered = certs
            .iter()
            .find(|info| info.source.is_some())
            .ok_or_else(|| anyhow::anyhow!("no discovered certificate"))?;
        assert_eq!(discovered.id, cert.id);
        let source = discovered.source.as_ref().map(|s| s.resource.as_str());
        assert_eq!(source, Some("secret default/app-tls"));
        let deleted = call(&mut channels, DeleteCert { id: cert.id }).await?;
        assert!(matches!(deleted, Err(Error::CertificateReadOnly { .. })));
        assert!(saved.load_certs().await.is_empty());

        // The watch stream reports the moved endpoint to both proxies.
        mock.put("endpointslices", slice(port_of(&second)?));
        wait_for_host_body(&url, HOST, "second").await?;
        wait_for_host_body(&url, CRD_HOST, "second").await?;

        // A deleted Ingress removes its proxy and the certificate of its secret.
        mock.delete("ingresses", ingress("r3v3rs3"));
        wait_for_host_status(&url, Some(HOST), 502).await?;
        let certs = call(&mut channels, GetCertList).await??;
        assert!(certs.iter().all(|info| info.source.is_none()));
        let crd_body = wait_for_host_status(&url, Some(CRD_HOST), 200).await?;
        assert_eq!(crd_body, "second");

        // A deleted R3v3rs3Proxy removes its proxy.
        mock.delete("r3v3rs3proxies", crd_proxy());
        wait_for_host_status(&url, Some(CRD_HOST), 502).await?;
        Ok(())
    })
    .await;
    std::fs::remove_file(&kubeconfig_path)?;
    result
}
