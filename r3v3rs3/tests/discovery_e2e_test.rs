//! Service discovery against real services: Consul and etcd from
//! `tests/discovery/docker-compose.yml`, and the local Docker engine.
//!
//! `make test-discovery-e2e` starts the containers, runs the tests and removes them. The Docker
//! test needs a host that reaches the container addresses, for example Linux or OrbStack.

use axum::{Router, routing::get};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use r3v3rs3::server::ServerChannels;
use r3v3rs3::server::rpc::config::SetConfig;
use r3v3rs3::server::rpc::discovery::GetDiscoveryStatus;
use r3v3rs3_api::{
    app::AppConfig,
    discovery::{DiscoveryProvider, DiscoveryState},
    port::PortEntry,
};
use serde_json::{Value, json};
use std::net::Ipv4Addr;
use std::process::Command;
use std::time::Duration;

mod common;
use common::e2e::{CONSUL, CONSUL_TOKEN, ETCD, ETCD_PASSWORD, consul_put, etcd_post, etcd_token};
use common::{
    TestStorage, alloc_tcp_port, call, http_port_entry, wait_for_host_body, wait_for_host_status,
    with_server,
};

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
