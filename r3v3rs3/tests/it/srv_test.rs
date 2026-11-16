//! HTTP routes whose servers come from DNS SRV records.

use r3v3rs3::server::{
    ServerChannels,
    rpc::{config::SetConfig, proxies::GetProxyStatus},
};
use r3v3rs3_api::{
    app::AppConfig,
    proxy::{HttpProxy, ProxyStatus, Route, Server},
    upstream::SrvStatus,
};
use std::{net::SocketAddr, time::Duration};

use crate::common;
use common::{
    TestStorage, alloc_tcp_port, call,
    dns::{MockDns, SrvRecord},
    http_port_entry, http_proxy_entry, wait_for_status, with_server,
};

/// The SRV lookup states of the proxy `s`.
async fn srv_status(channels: &mut ServerChannels) -> anyhow::Result<Vec<SrvStatus>> {
    let id = "s".parse()?;
    let status: ProxyStatus = call(channels, GetProxyStatus { id }).await??;
    Ok(status.srv)
}

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
    let target = format!("{}:{}", record.target, record.port);
    dns.set(NAME, 60, vec![record]);
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |mut channels| async move {
        let body = wait_for_status(port.http_url("/").as_ref(), 200).await?;
        assert_eq!(body, "from-srv");

        let srv = srv_status(&mut channels).await?;
        assert_eq!(srv.len(), 1);
        assert_eq!(srv[0].name, NAME);
        assert_eq!(srv[0].targets, [target]);
        assert_eq!(srv[0].error, None);
        assert!(srv[0].refreshed_at.is_some());
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

/// A name that does not exist (NXDOMAIN) has no target, so the route answers 502. RFC 8767
/// section 4 counts the NXDOMAIN answer as a successful lookup, so no error is reported.
#[tokio::test]
async fn srv_without_an_answer_answers_502() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |mut channels| async move {
        wait_for_status(port.http_url("/").as_ref(), 502).await?;

        let srv = srv_status(&mut channels).await?;
        assert_eq!(srv.len(), 1);
        assert!(srv[0].targets.is_empty());
        assert_eq!(srv[0].error, None, "{srv:?}");
        assert!(srv[0].refreshed_at.is_some(), "{srv:?}");
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
        call(&mut channels, SetConfig { config }).await??;

        wait_for_body(&url, "from-srv", Duration::from_secs(3)).await?;
        Ok(())
    })
    .await
}

/// RFC 2782: "A client MUST attempt to contact the target host with the lowest-numbered priority
/// it can reach", so a reachable target with priority 0 gets every request and the target with
/// priority 1 gets none. The status lists both, the lower priority first.
#[tokio::test]
async fn srv_uses_the_lowest_priority_it_can_reach() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    let (_primary, primary) = upstream("primary").await?;
    let (_backup, mut backup) = upstream("backup").await?;
    backup.priority = 1;
    let targets = [&primary, &backup].map(|record| format!("{}:{}", record.target, record.port));
    dns.set(NAME, 60, vec![backup, primary]);
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |mut channels| async move {
        let url = port.http_url("/").to_string();
        wait_for_body(&url, "primary", Duration::from_secs(5)).await?;
        let client = reqwest::Client::new();
        for _ in 0..10 {
            assert_eq!(client.get(&url).send().await?.text().await?, "primary");
        }
        assert_eq!(srv_status(&mut channels).await?[0].targets, targets);
        Ok(())
    })
    .await
}

/// RFC 2782: when no target of the lowest-numbered priority can be reached, the client uses the
/// next priority that it can reach.
#[tokio::test]
async fn srv_falls_back_to_the_next_priority_when_the_lowest_is_unreachable() -> anyhow::Result<()>
{
    let dns = MockDns::start().await?;
    // Port 1 on loopback refuses the connection.
    let unreachable = SrvRecord::new(1, "127.0.0.1");
    let (_backup, mut backup) = upstream("backup").await?;
    backup.priority = 1;
    dns.set(NAME, 60, vec![unreachable, backup]);
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |_| async move {
        let url = port.http_url("/").to_string();
        wait_for_body(&url, "backup", Duration::from_secs(10)).await
    })
    .await
}

/// Waits until the SRV status of `NAME` satisfies `done`, for at most 15 seconds: the shortest
/// refresh after a TTL of 1 second is 5 seconds.
async fn wait_for_srv(
    channels: &mut ServerChannels,
    done: impl Fn(&SrvStatus) -> bool,
) -> anyhow::Result<SrvStatus> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let srv = srv_status(channels).await?.remove(0);
        if done(&srv) {
            return Ok(srv);
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("the SRV status did not change: {srv:?}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// RFC 8767 section 4: an answer with another rcode than NOERROR or NXDOMAIN is a failure to
/// refresh, and the last data may still be used. The error is reported.
#[tokio::test]
async fn a_server_failure_keeps_the_last_targets() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    let (_upstream, record) = upstream("kept").await?;
    let target = format!("{}:{}", record.target, record.port);
    dns.set(NAME, 1, vec![record]);
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |mut channels| async move {
        let url = port.http_url("/").to_string();
        wait_for_body(&url, "kept", Duration::from_secs(5)).await?;
        dns.fail(NAME);
        let failed = wait_for_srv(&mut channels, |srv| srv.error.is_some()).await?;
        assert_eq!(failed.targets, [target]);
        wait_for_body(&url, "kept", Duration::from_secs(1)).await
    })
    .await
}

/// RFC 8767 section 4: an authoritative NXDOMAIN refreshes the data, and RFC 2308 says the name
/// does not exist. The old targets are no longer used, so the route answers 502.
#[tokio::test]
async fn nxdomain_removes_the_targets() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    let (_upstream, record) = upstream("gone").await?;
    dns.set(NAME, 1, vec![record]);
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |mut channels| async move {
        let url = port.http_url("/").to_string();
        wait_for_body(&url, "gone", Duration::from_secs(5)).await?;
        dns.remove(NAME);
        wait_for_srv(&mut channels, |srv| srv.targets.is_empty()).await?;
        wait_for_status(&url, 502).await?;
        Ok(())
    })
    .await
}

/// RFC 2782: one SRV record with the target "." means that the service is decidedly not
/// available, so the route answers 502.
#[tokio::test]
async fn a_root_target_means_no_service() -> anyhow::Result<()> {
    let dns = MockDns::start().await?;
    dns.set(NAME, 60, vec![SrvRecord::new(80, ".")]);
    let port = alloc_tcp_port().await?;

    with_server(storage(&port, Some(dns.addr)), |mut channels| async move {
        wait_for_status(port.http_url("/").as_ref(), 502).await?;
        let srv = srv_status(&mut channels).await?;
        assert!(srv[0].targets.is_empty(), "{srv:?}");
        Ok(())
    })
    .await
}
