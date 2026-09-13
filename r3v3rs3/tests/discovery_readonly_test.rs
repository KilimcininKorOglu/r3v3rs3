use axum::{routing::get, Router};
use r3v3rs3::{
    command::ServerCommand,
    config::storage::Storage,
    server::rpc::{
        proxies::{DeleteProxy, UpdateProxy},
        ErasedRpcMethod, RpcMethod, RpcWrapper,
    },
    server::ServerChannels,
};
use r3v3rs3_api::{
    discovery::{DiscoveryProvider, DiscoverySource},
    error::Error,
    proxy::{HttpProxy, ProxyEntry},
};
use std::time::Duration;

mod common;
use common::{
    alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, serve_http_upstream,
    with_server, TestStorage,
};

async fn call<M: RpcMethod + 'static>(
    channels: &mut ServerChannels,
    method: M,
) -> anyhow::Result<Result<(), Error>> {
    let arg = Box::new(RpcWrapper::new(method)) as Box<dyn ErasedRpcMethod>;
    channels
        .command
        .send(ServerCommand::CallMethod { id: 1, arg })
        .await?;
    let callback = channels
        .callback
        .recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
    Ok(callback.result.map(|_| ()))
}

/// Sends requests until the proxy answers with the expected status.
async fn wait_for_status(url: &str, expected: u16) -> anyhow::Result<String> {
    for _ in 0..50 {
        if let Ok(res) = reqwest::get(url).await {
            if res.status().as_u16() == expected {
                return Ok(res.text().await?);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("{url} did not answer with {expected}")
}

#[tokio::test]
async fn discovered_proxies_serve_traffic_but_are_read_only_and_not_saved() -> anyhow::Result<()> {
    let upstream =
        serve_http_upstream(Router::new().route("/", get(|| async { "discovered" }))).await?;
    let proxy_port = alloc_tcp_port().await?;

    let manual = http_proxy_entry(
        "manual",
        "test",
        HttpProxy {
            vhosts: vec!["manual.example".parse()?],
            routes: vec![http_route("/", upstream.as_str(), None)],
            ..Default::default()
        },
    );
    let mut discovered = http_proxy_entry(
        "disc",
        "test",
        HttpProxy {
            routes: vec![http_route("/", upstream.as_str(), None)],
            ..Default::default()
        },
    );
    discovered.source = Some(DiscoverySource {
        provider: DiscoveryProvider::Docker,
        resource: "web-1".into(),
    });

    let storage = TestStorage::builder()
        .ports(vec![http_port_entry("test", &proxy_port)])
        .proxies(vec![manual.clone()])
        .build();

    let url = proxy_port.http_url("/").to_string();
    let discovered_id = discovered.id;
    let mut renamed = manual.clone();
    renamed.proxy.name = "renamed".into();
    let saved = vec![renamed.clone()];
    with_server(storage.clone(), |mut channels| async move {
        channels
            .command
            .send(ServerCommand::SetDiscoveredProxies {
                provider: DiscoveryProvider::Docker,
                entries: vec![discovered.clone()],
            })
            .await?;
        assert_eq!(wait_for_status(&url, 200).await?, "discovered");

        let update = UpdateProxy {
            entry: ProxyEntry::from((discovered_id, discovered.proxy.clone())),
        };
        assert!(matches!(
            call(&mut channels, update).await?,
            Err(Error::ProxyReadOnly { .. })
        ));
        assert!(matches!(
            call(&mut channels, DeleteProxy { id: discovered_id }).await?,
            Err(Error::ProxyReadOnly { .. })
        ));

        // Changing the manual proxy writes the proxy list to the storage.
        let update = UpdateProxy {
            entry: ProxyEntry::from((renamed.id, renamed.proxy.clone())),
        };
        call(&mut channels, update).await??;

        channels
            .command
            .send(ServerCommand::SetDiscoveredProxies {
                provider: DiscoveryProvider::Docker,
                entries: vec![],
            })
            .await?;
        // A request that matches no route receives 502 Bad Gateway.
        wait_for_status(&url, 502).await?;
        Ok(())
    })
    .await?;

    assert_eq!(storage.load_proxies().await, saved);
    Ok(())
}
