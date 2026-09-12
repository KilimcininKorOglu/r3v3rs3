use r3v3rs3_api::{
    client_ip::ClientIpConfig,
    policy::{RateLimit, RatePeriod},
    proxy::HttpProxy,
};
use reqwest::{header::RETRY_AFTER, StatusCode};

mod common;
use common::{
    alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server, TestStorage,
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

    let mut free_route = http_route("/free", &upstream, None);
    free_route.rate_limit = Some(RateLimit::default());

    let limited = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![free_route, http_route("/", &upstream, None)],
        upgrade_insecure: false,
        rate_limit: per_minute(2),
        ..Default::default()
    };
    let forwarded = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream, None)],
        upgrade_insecure: false,
        client_ip: ClientIpConfig {
            trust_cdn: true,
            trusted_proxies: vec!["127.0.0.0/8".parse()?, "::1/128".parse()?],
        },
        rate_limit: per_minute(1),
        ..Default::default()
    };

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
    })
    .await?;

    mock_limited.assert_async().await;
    mock_free.assert_async().await;
    mock_forwarded.assert_async().await;
    Ok(())
}
