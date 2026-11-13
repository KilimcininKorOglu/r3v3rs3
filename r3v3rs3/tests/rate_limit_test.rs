use r3v3rs3_api::{
    client_ip::ClientIpConfig,
    policy::{RateLimit, RatePeriod},
    proxy::HttpProxy,
};
use reqwest::{StatusCode, header::RETRY_AFTER};

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

fn per_minute(requests: u32) -> RateLimit {
    RateLimit {
        requests,
        per: RatePeriod::Minute,
        burst: 0,
    }
}

#[tokio::test]
async fn rate_limit_rejects_clients_over_the_limit() -> anyhow::Result<()> {
    let limited_port = alloc_tcp_port().await?;
    let forwarded_port = alloc_tcp_port().await?;
    let mut server = mockito::Server::new_async().await;
    let upstream = server.url();

    let mock_limited = server
        .mock("GET", "/limited")
        .with_body("limited")
        .expect(2)
        .create_async()
        .await;
    let mock_free = server
        .mock("GET", "/")
        .with_body("free")
        .expect(5)
        .create_async()
        .await;
    let mock_forwarded = server
        .mock("GET", "/forwarded")
        .with_body("forwarded")
        .expect(2)
        .create_async()
        .await;

    let limited = limited_proxy(&upstream);
    let forwarded = forwarded_proxy(&upstream)?;

    let config = TestStorage::builder()
        .ports(vec![
            http_port_entry("limited", &limited_port),
            http_port_entry("fwd", &forwarded_port),
        ])
        .proxies(vec![
            http_proxy_entry("proxy1", "limited", limited),
            http_proxy_entry("proxy2", "fwd", forwarded),
        ])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        check_limited_port(&client, &limited_port).await?;
        check_forwarded_port(&client, &forwarded_port).await
    })
    .await?;

    mock_limited.assert_async().await;
    mock_free.assert_async().await;
    mock_forwarded.assert_async().await;
    Ok(())
}

/// A proxy limited to two requests a minute, with an unlimited `/free` route.
fn limited_proxy(upstream: &str) -> HttpProxy {
    let mut free_route = http_route("/free", upstream, None);
    free_route.rate_limit = Some(RateLimit::default());

    HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![free_route, http_route("/", upstream, None)],
        upgrade_insecure: false,
        rate_limit: per_minute(2),
        ..Default::default()
    }
}

/// A proxy limited to one request a minute per client, which trusts `X-Forwarded-For` from
/// loopback.
fn forwarded_proxy(upstream: &str) -> anyhow::Result<HttpProxy> {
    Ok(HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream, None)],
        upgrade_insecure: false,
        client_ip: ClientIpConfig {
            trust_cdn: true,
            trusted_proxies: vec!["127.0.0.0/8".parse()?, "::1/128".parse()?],
        },
        rate_limit: per_minute(1),
        ..Default::default()
    })
}

/// Checks that the proxy limit rejects the third request with a `Retry-After` header and that
/// the unlimited route is not limited.
async fn check_limited_port(
    client: &reqwest::Client,
    limited_port: &common::TestPort,
) -> anyhow::Result<()> {
    for _ in 0..2 {
        let resp = client.get(limited_port.http_url("/limited")).send().await?;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let resp = client.get(limited_port.http_url("/limited")).send().await?;
    assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
    let retry_after: u64 = resp
        .headers()
        .get(RETRY_AFTER)
        .expect("Retry-After header")
        .to_str()?
        .parse()?;
    assert!((1..=30).contains(&retry_after));

    for _ in 0..5 {
        let resp = client.get(limited_port.http_url("/free")).send().await?;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    Ok(())
}

/// Checks that the limit counts each forwarded client address on its own.
async fn check_forwarded_port(
    client: &reqwest::Client,
    forwarded_port: &common::TestPort,
) -> anyhow::Result<()> {
    let forwarded_request = |client_ip: &'static str| {
        client
            .get(forwarded_port.http_url("/forwarded"))
            .header("x-forwarded-for", client_ip)
            .send()
    };
    assert_eq!(
        forwarded_request("198.51.100.1").await?.status(),
        StatusCode::OK
    );
    assert_eq!(
        forwarded_request("198.51.100.1").await?.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        forwarded_request("198.51.100.2").await?.status(),
        StatusCode::OK
    );

    Ok(())
}
