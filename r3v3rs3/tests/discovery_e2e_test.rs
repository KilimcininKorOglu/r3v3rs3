//! Service discovery against real services: Consul, etcd and k3s from
//! `tests/discovery/docker-compose.yml`, and the local Docker engine.
//!
//! `make test-discovery-e2e` starts the containers, installs the custom resource definition and
//! the RBAC objects in k3s, writes the admin kubeconfig and a service account token to
//! `R3V3RS3_E2E_DIR`, runs the tests and removes the containers. The Docker test needs a host that
//! reaches the container addresses, for example Linux or OrbStack.

use axum::{routing::get, Router};
use base64::prelude::{Engine as _, BASE64_STANDARD};
use kube::api::{DeleteParams, PostParams};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::core::{ApiResource, DynamicObject, GroupVersionKind};
use kube::{Api, Client, Config};
use r3v3rs3::server::rpc::config::SetConfig;
use r3v3rs3::server::rpc::discovery::GetDiscoveryStatus;
use r3v3rs3::server::ServerChannels;
use r3v3rs3_api::{
    app::AppConfig,
    discovery::{DiscoveryProvider, DiscoveryState},
    port::PortEntry,
};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

mod common;
use common::{
    alloc_tcp_port, call, http_port_entry, wait_for_host_body, wait_for_host_status, with_server,
    TestStorage,
};

const CONSUL: &str = "http://127.0.0.1:8480";
const CONSUL_TOKEN: &str = "e2e-consul-token";
const ETCD: &str = "http://127.0.0.1:8481";
const ETCD_PASSWORD: &str = "e2e-etcd-password";
const K3S: &str = "https://127.0.0.1:8482";
const WHOAMI: &str = "r3v3rs3-e2e-whoami";

/// Serves `body` on the IPv4 address and returns the port.
async fn serve_ipv4(ip: Ipv4Addr, body: &'static str) -> anyhow::Result<u16> {
    let listener = tokio::net::TcpListener::bind((ip, 0)).await?;
    let port = listener.local_addr()?.port();
    let app = Router::new().route("/", get(move || async move { body }));
    tokio::spawn(std::future::IntoFuture::into_future(axum::serve(
        listener, app,
    )));
    Ok(port)
}

/// The IPv4 address of the default route of this host. The Kubernetes API server rejects a
/// loopback address in an EndpointSlice. A connected UDP socket sends no packet, and its local
/// address is the source address of the route. The first address of an interface list can be the
/// network address of a container bridge, which accepts no connection.
fn host_ipv4() -> anyhow::Result<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9))?;
    match socket.local_addr()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Ok(ip),
        ip => anyhow::bail!("the default route has no IPv4 address for an EndpointSlice: {ip}"),
    }
}

/// A proxy port with the name `web`, and its URL.
async fn web_port() -> anyhow::Result<(PortEntry, String)> {
    let proxy_port = alloc_tcp_port().await?;
    let mut port = http_port_entry("web", &proxy_port);
    port.port.name = "web".into();
    Ok((port, proxy_port.http_url("/").to_string()))
}

/// Waits until the provider runs with `proxies` proxies. A real service needs more time than a
/// mock, so the wait is longer than `wait_for_discovery`.
async fn wait_for_proxies(
    channels: &mut ServerChannels,
    provider: DiscoveryProvider,
    proxies: usize,
) -> anyhow::Result<()> {
    let mut statuses = Vec::new();
    for _ in 0..600 {
        statuses = call(channels, GetDiscoveryStatus).await??;
        let running = statuses.iter().any(|status| {
            status.provider == provider
                && status.state == DiscoveryState::Running
                && status.proxies == proxies
        });
        if running {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("{provider} did not add {proxies} proxies: {statuses:?}")
}

async fn consul_put(path: &str, body: String) -> anyhow::Result<()> {
    reqwest::Client::new()
        .put(format!("{CONSUL}{path}"))
        .header("X-Consul-Token", CONSUL_TOKEN)
        .body(body)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

#[tokio::test]
#[ignore = "needs the discovery containers, run it with make test-discovery-e2e"]
async fn consul_services_and_keys_define_proxies() -> anyhow::Result<()> {
    let catalog_port = serve_ipv4(Ipv4Addr::LOCALHOST, "catalog").await?;
    let kv_port = serve_ipv4(Ipv4Addr::LOCALHOST, "kv").await?;
    let service = json!({
        "ID": "e2e-web-1", "Name": "e2e-web", "Address": "127.0.0.1", "Port": catalog_port,
        "Tags": [
            "r3v3rs3.enable=true",
            "r3v3rs3.http.web.ports=web",
            "r3v3rs3.http.web.vhosts=catalog.e2e.test",
        ],
    });
    consul_put("/v1/agent/service/register", service.to_string()).await?;
    let keys = [
        ("ports", "web".to_string()),
        ("vhosts", "kv.e2e.test".to_string()),
        (
            "routes/0/servers/0/url",
            format!("http://127.0.0.1:{kv_port}/"),
        ),
    ];
    for (key, value) in keys {
        consul_put(&format!("/v1/kv/r3v3rs3/http/kv/{key}"), value).await?;
    }

    let (port, url) = web_port().await?;
    let mut config = AppConfig::default();
    config.discovery.consul.enabled = true;
    config.discovery.consul.address = CONSUL.into();
    config.discovery.consul.token = Some(CONSUL_TOKEN.into());
    let storage = TestStorage::builder().ports(vec![port]).build();
    with_server(storage, |mut channels| async move {
        call(&mut channels, SetConfig { config }).await??;
        wait_for_proxies(&mut channels, DiscoveryProvider::Consul, 2).await?;
        wait_for_host_body(&url, "catalog.e2e.test", "catalog").await?;
        wait_for_host_body(&url, "kv.e2e.test", "kv").await?;

        // A deregistered service removes its proxy.
        consul_put("/v1/agent/service/deregister/e2e-web-1", String::new()).await?;
        wait_for_proxies(&mut channels, DiscoveryProvider::Consul, 1).await?;
        wait_for_host_status(&url, Some("catalog.e2e.test"), 502).await?;
        Ok(())
    })
    .await
}

async fn etcd_post(path: &str, body: Value, token: Option<&str>) -> anyhow::Result<Value> {
    let mut request = reqwest::Client::new()
        .post(format!("{ETCD}{path}"))
        .json(&body);
    if let Some(token) = token {
        request = request.header("Authorization", token);
    }
    let response = request.send().await?;
    let status = response.status();
    let body = response.json::<Value>().await?;
    anyhow::ensure!(status.is_success(), "etcd {path}: {status} {body}");
    Ok(body)
}

/// Adds the `root` user, enables authentication and returns a token. A cluster whose
/// authentication is already on only returns the token.
async fn etcd_token() -> anyhow::Result<String> {
    let credentials = json!({"name": "root", "password": ETCD_PASSWORD});
    let status = etcd_post("/v3/auth/status", json!({}), None).await?;
    if status["enabled"] != json!(true) {
        etcd_post("/v3/auth/user/add", credentials.clone(), None).await?;
        let grant = json!({"user": "root", "role": "root"});
        etcd_post("/v3/auth/user/grant", grant, None).await?;
        etcd_post("/v3/auth/enable", json!({}), None).await?;
    }
    let body = etcd_post("/v3/auth/authenticate", credentials, None).await?;
    body["token"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| anyhow::anyhow!("etcd returned no token: {body}"))
}

async fn etcd_put(token: &str, key: &str, value: &str) -> anyhow::Result<()> {
    let body = json!({"key": BASE64_STANDARD.encode(key), "value": BASE64_STANDARD.encode(value)});
    etcd_post("/v3/kv/put", body, Some(token)).await?;
    Ok(())
}

#[tokio::test]
#[ignore = "needs the discovery containers, run it with make test-discovery-e2e"]
async fn etcd_keys_define_proxies_that_follow_changes() -> anyhow::Result<()> {
    const URL_KEY: &str = "r3v3rs3/http/etcd/routes/0/servers/0/url";
    let first = format!(
        "http://127.0.0.1:{}/",
        serve_ipv4(Ipv4Addr::LOCALHOST, "first").await?
    );
    let second = format!(
        "http://127.0.0.1:{}/",
        serve_ipv4(Ipv4Addr::LOCALHOST, "second").await?
    );
    let token = etcd_token().await?;
    etcd_put(&token, "r3v3rs3/http/etcd/ports", "web").await?;
    etcd_put(&token, "r3v3rs3/http/etcd/vhosts", "etcd.e2e.test").await?;
    etcd_put(&token, URL_KEY, &first).await?;

    let (port, url) = web_port().await?;
    let mut config = AppConfig::default();
    config.discovery.etcd.enabled = true;
    config.discovery.etcd.endpoints = vec![ETCD.into()];
    config.discovery.etcd.username = "root".into();
    config.discovery.etcd.password = Some(ETCD_PASSWORD.into());
    let storage = TestStorage::builder().ports(vec![port]).build();
    with_server(storage, |mut channels| async move {
        call(&mut channels, SetConfig { config }).await??;
        wait_for_proxies(&mut channels, DiscoveryProvider::Etcd, 1).await?;
        wait_for_host_body(&url, "etcd.e2e.test", "first").await?;

        // The watch stream reports the changed key.
        etcd_put(&token, URL_KEY, &second).await?;
        wait_for_host_body(&url, "etcd.e2e.test", "second").await
    })
    .await
}

fn e2e_dir() -> anyhow::Result<PathBuf> {
    std::env::var_os("R3V3RS3_E2E_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| {
            anyhow::anyhow!("R3V3RS3_E2E_DIR is not set, run the test with make test-discovery-e2e")
        })
}

/// Writes the admin kubeconfig and the kubeconfig of the `r3v3rs3` service account for the
/// published k3s port, and returns their paths.
fn kubeconfigs(dir: &Path) -> anyhow::Result<(PathBuf, PathBuf)> {
    let admin_yaml = std::fs::read_to_string(dir.join("admin.yaml"))?;
    let mut admin: Value = serde_saphyr::from_str(&admin_yaml)?;
    admin["clusters"][0]["cluster"]["server"] = json!(K3S);
    let token = std::fs::read_to_string(dir.join("token"))?;
    let mut service_account = admin.clone();
    service_account["users"][0]["user"] = json!({"token": token.trim()});

    // JSON is valid YAML, so the kubeconfig reader accepts these files.
    let admin_path = dir.join("admin.json");
    let service_account_path = dir.join("r3v3rs3.json");
    std::fs::write(&admin_path, admin.to_string())?;
    std::fs::write(&service_account_path, service_account.to_string())?;
    Ok((admin_path, service_account_path))
}

async fn kube_client(path: &Path) -> anyhow::Result<Client> {
    let kubeconfig = Kubeconfig::read_from(path)?;
    let config = Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default()).await?;
    Ok(Client::try_from(config)?)
}

/// The API of a resource in the `default` namespace.
fn namespaced(
    client: &Client,
    api_version: (&str, &str),
    kind: &str,
    plural: &str,
) -> Api<DynamicObject> {
    let (group, version) = api_version;
    let gvk = GroupVersionKind::gvk(group, version, kind);
    let resource = ApiResource::from_gvk_with_plural(&gvk, plural);
    Api::namespaced_with(client.clone(), "default", &resource)
}

async fn create(api: &Api<DynamicObject>, object: Value) -> anyhow::Result<()> {
    api.create(&PostParams::default(), &serde_json::from_value(object)?)
        .await?;
    Ok(())
}

/// Creates the `e2e-app` Service with an EndpointSlice on the upstream, an Ingress and an
/// R3v3rs3Proxy resource that route to it, and returns the Ingress API.
async fn create_resources(
    client: &Client,
    upstream_ip: Ipv4Addr,
    upstream_port: u16,
) -> anyhow::Result<Api<DynamicObject>> {
    let services = namespaced(client, ("", "v1"), "Service", "services");
    let service = json!({
        "apiVersion": "v1", "kind": "Service", "metadata": {"name": "e2e-app"},
        "spec": {"ports": [{"name": "http", "port": 80}]},
    });
    create(&services, service).await?;
    // The Service has no selector, so the test writes its EndpointSlice.
    let slices = namespaced(
        client,
        ("discovery.k8s.io", "v1"),
        "EndpointSlice",
        "endpointslices",
    );
    let slice = json!({
        "apiVersion": "discovery.k8s.io/v1", "kind": "EndpointSlice",
        "metadata": {"name": "e2e-app-1", "labels": {"kubernetes.io/service-name": "e2e-app"}},
        "addressType": "IPv4",
        "ports": [{"name": "http", "port": upstream_port}],
        "endpoints": [{"addresses": [upstream_ip.to_string()], "conditions": {"ready": true}}],
    });
    create(&slices, slice).await?;
    let ingresses = namespaced(client, ("networking.k8s.io", "v1"), "Ingress", "ingresses");
    let backend = json!({"service": {"name": "e2e-app", "port": {"name": "http"}}});
    let ingress = json!({
        "apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
        "metadata": {"name": "e2e-app", "annotations": {"r3v3rs3.io/ports": "web"}},
        "spec": {
            "ingressClassName": "r3v3rs3",
            "rules": [{"host": "ingress.e2e.test", "http": {"paths": [
                {"path": "/", "pathType": "Prefix", "backend": backend},
            ]}}],
        },
    });
    create(&ingresses, ingress).await?;
    let proxies = namespaced(
        client,
        ("r3v3rs3.io", "v1"),
        "R3v3rs3Proxy",
        "r3v3rs3proxies",
    );
    let proxy = json!({
        "apiVersion": "r3v3rs3.io/v1", "kind": "R3v3rs3Proxy", "metadata": {"name": "e2e-crd"},
        "spec": {
            "ports": ["web"], "vhosts": ["crd.e2e.test"],
            "routes": [{"path": "/", "service": {"name": "e2e-app", "port": "http"}}],
        },
    });
    create(&proxies, proxy).await?;
    Ok(ingresses)
}

#[tokio::test]
#[ignore = "needs the discovery containers, run it with make test-discovery-e2e"]
async fn kubernetes_resources_define_proxies_with_the_rbac_of_the_manifests() -> anyhow::Result<()>
{
    let dir = e2e_dir()?;
    let (admin_path, service_account_path) = kubeconfigs(&dir)?;
    let client = kube_client(&admin_path).await?;
    let upstream_ip = host_ipv4()?;
    let upstream_port = serve_ipv4(upstream_ip, "kubernetes").await?;
    let ingresses = create_resources(&client, upstream_ip, upstream_port).await?;

    let (port, url) = web_port().await?;
    let mut config = AppConfig::default();
    let kubernetes = &mut config.discovery.kubernetes;
    kubernetes.enabled = true;
    kubernetes.kubeconfig = service_account_path.display().to_string();
    kubernetes.namespaces = vec!["default".into()];
    kubernetes.crd = true;
    kubernetes.ingress_class = "r3v3rs3".into();
    let storage = TestStorage::builder().ports(vec![port]).build();
    with_server(storage, |mut channels| async move {
        call(&mut channels, SetConfig { config }).await??;
        wait_for_proxies(&mut channels, DiscoveryProvider::Kubernetes, 2).await?;
        wait_for_host_body(&url, "ingress.e2e.test", "kubernetes").await?;
        wait_for_host_body(&url, "crd.e2e.test", "kubernetes").await?;

        // A deleted Ingress removes its proxy, and the custom resource keeps its proxy.
        ingresses
            .delete("e2e-app", &DeleteParams::default())
            .await?;
        wait_for_host_status(&url, Some("ingress.e2e.test"), 502).await?;
        wait_for_proxies(&mut channels, DiscoveryProvider::Kubernetes, 1).await?;
        wait_for_host_body(&url, "crd.e2e.test", "kubernetes").await
    })
    .await
}

fn docker(args: &[&str]) -> anyhow::Result<String> {
    let output = Command::new("docker").args(args).output()?;
    anyhow::ensure!(
        output.status.success(),
        "docker {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[tokio::test]
#[ignore = "needs the discovery containers, run it with make test-discovery-e2e"]
async fn docker_labels_of_a_running_container_define_a_proxy() -> anyhow::Result<()> {
    // `rm -f` also succeeds without the container, so a failed earlier run does not break this run.
    docker(&["rm", "-f", WHOAMI])?;
    docker(&[
        "run",
        "-d",
        "--name",
        WHOAMI,
        "--label",
        "r3v3rs3.enable=true",
        "--label",
        "r3v3rs3.http.whoami.ports=web",
        "--label",
        "r3v3rs3.http.whoami.vhosts=docker.e2e.test",
        "--label",
        "r3v3rs3.http.whoami.port=80",
        "traefik/whoami:v1.11",
    ])?;

    let (port, url) = web_port().await?;
    let mut config = AppConfig::default();
    config.discovery.docker.enabled = true;
    config.discovery.docker.endpoint = "unix:///var/run/docker.sock".into();
    let storage = TestStorage::builder().ports(vec![port]).build();
    let result = with_server(storage, |mut channels| async move {
        call(&mut channels, SetConfig { config }).await??;
        wait_for_proxies(&mut channels, DiscoveryProvider::Docker, 1).await?;
        let body = wait_for_host_status(&url, Some("docker.e2e.test"), 200).await?;
        // whoami answers with the container hostname, so the request reached the container.
        assert!(body.contains("Hostname: "), "{body}");

        // The removed container removes its proxy.
        docker(&["rm", "-f", WHOAMI])?;
        wait_for_host_status(&url, Some("docker.e2e.test"), 502).await?;
        wait_for_proxies(&mut channels, DiscoveryProvider::Docker, 0).await
    })
    .await;
    docker(&["rm", "-f", WHOAMI])?;
    result
}
