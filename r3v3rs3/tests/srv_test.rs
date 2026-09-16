//! HTTP routes whose servers come from DNS SRV records.

use r3v3rs3::{
    accounts::Caller,
    command::ServerCommand,
    server::rpc::{config::SetConfig, ErasedRpcMethod, RpcWrapper},
};
use r3v3rs3_api::{
    app::AppConfig,
    proxy::{HttpProxy, Route, Server},
};
use std::{net::SocketAddr, time::Duration};

mod common;
use common::{
    alloc_tcp_port,
    dns::{MockDns, SrvRecord},
    http_port_entry, http_proxy_entry, wait_for_status, with_server, TestStorage,
};

// Not `.invalid`: hickory answers that zone with NXDOMAIN without a query (RFC 6761).
const NAME: &str = "_http._tcp.app.srv-test.test";

/// A storage with one HTTP port and one proxy whose route resolves `NAME`.
fn storage(port: &common::TestPort, resolver: Option<SocketAddr>) -> TestStorage {
    let http = HttpProxy {
        routes: vec![Route {
            servers: vec![Server::new(format!("http+srv://{NAME}/").parse().unwrap())],
            ..Default::default()
        }],
        ..Default::default()
    };
    let config = AppConfig {
        upstream_dns_resolver: resolver,
        ..Default::default()
    };
    TestStorage::builder()
        .config(config)
        .ports(vec![http_port_entry("p", port)])
        .proxies(vec![http_proxy_entry("s", "p", http)])
        .build()
}

/// A mock upstream that answers every request with `body`, and its SRV record.
async fn upstream(body: &str) -> anyhow::Result<(mockito::ServerGuard, SrvRecord)> {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/")
        .with_body(body)
        .expect_at_least(0)
        .create_async()
        .await;
    let addr = server.socket_address();
    Ok((server, SrvRecord::new(addr.port(), &addr.ip().to_string())))
}

/// Sends requests until the body is `expected`, for at most `wait`.
async fn wait_for_body(url: &str, expected: &str, wait: Duration) -> anyhow::Result<()> {
    let deadline = tokio::time::Instant::now() + wait;
    let client = reqwest::Client::new();
    let mut last = String::new();
    while tokio::time::Instant::now() < deadline {
        if let Ok(res) = client.get(url).send().await {
            last = res.text().await?;
            if last == expected {
                return Ok(());
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    anyhow::bail!("{url} answered {last:?} instead of {expected:?}")
}

#[tokio::test]
async fn srv_route_sends_requests_to_the_resolved_target() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    let (_upstream, record) = upstream("from-srv").await?;
    dns.set(NAME, 60, vec![record]);
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |_| async move {
        let body = wait_for_status(port.http_url("/").as_ref(), 200).await?;
        assert_eq!(body, "from-srv");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn srv_targets_change_when_the_ttl_expires() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    let (_first, first) = upstream("first").await?;
    let (_second, second) = upstream("second").await?;
    dns.set(NAME, 1, vec![first]);
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |_| async move {
        let url = port.http_url("/").to_string();
        wait_for_body(&url, "first", Duration::from_secs(5)).await?;
        dns.set(NAME, 1, vec![second]);
        // The TTL of 1 second is raised to the minimum of 5 seconds.
        wait_for_body(&url, "second", Duration::from_secs(15)).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn srv_without_an_answer_answers_502() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |_| async move {
        wait_for_status(port.http_url("/").as_ref(), 502).await?;
        Ok(())
    })
    .await
}

#[tokio::test]
async fn changing_the_upstream_dns_resolver_resolves_at_once() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    let (_upstream, record) = upstream("from-srv").await?;
    dns.set(NAME, 60, vec![record]);
    let port = alloc_tcp_port().await?;

    // The system resolver does not know the `.test` name.
    with_server(storage(&port, None), |mut channels| async move {
        let url = port.http_url("/").to_string();
        wait_for_status(&url, 502).await?;

        let config = AppConfig {
            upstream_dns_resolver: Some(dns.addr),
            ..Default::default()
        };
        let arg = Box::new(RpcWrapper::new(SetConfig { config })) as Box<dyn ErasedRpcMethod>;
        channels
            .command
            .send(ServerCommand::CallMethod {
                id: 1,
                arg,
                caller: Caller::system(),
            })
            .await?;
        let callback = channels
            .callback
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("callback channel closed"))?;
        assert!(callback.result.is_ok());

        wait_for_body(&url, "from-srv", Duration::from_secs(3)).await?;
        Ok(())
    })
    .await
}
