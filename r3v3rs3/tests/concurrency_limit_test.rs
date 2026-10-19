use std::sync::Arc;
use std::time::Duration;

use axum::{Router, routing::get};
use r3v3rs3_api::policy::ConcurrencyLimit;
use r3v3rs3_api::proxy::HttpProxy;
use reqwest::{StatusCode, header::RETRY_AFTER};
use tokio::sync::Notify;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    serve_http_upstream, with_server,
};

#[tokio::test]
async fn concurrency_limit_rejects_excess_in_flight_requests_per_ip() -> anyhow::Result<()> {
    let gate1 = Arc::new(Notify::new());
    let g1 = gate1.clone();
    let gate2 = Arc::new(Notify::new());
    let g2 = gate2.clone();

    let app = Router::new()
        .route(
            "/slow1",
            get(move || {
                let g = g1.clone();
                async move {
                    g.notified().await;
                    "slow1 done"
                }
            }),
        )
        .route(
            "/slow2",
            get(move || {
                let g = g2.clone();
                async move {
                    g.notified().await;
                    "slow2 done"
                }
            }),
        )
        .route("/fast", get(|| async { "fast done" }));

    let upstream = serve_http_upstream(app).await?;
    let port = alloc_tcp_port().await?;

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", upstream.as_str(), None)],
        concurrency_limit: ConcurrencyLimit {
            max: 2,
            header: None,
        },
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("test", &port)])
        .proxies(vec![http_proxy_entry("proxy", "test", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();

        let slow1_url = port.http_url("/slow1");
        let slow2_url = port.http_url("/slow2");
        let fast_url = port.http_url("/fast");

        let c1 = client.clone();
        let h1 = tokio::spawn(async move { c1.get(slow1_url).send().await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        let c2 = client.clone();
        let h2 = tokio::spawn(async move { c2.get(slow2_url).send().await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 3rd concurrent request from the same IP must be rejected with 429
        let resp3 = client.get(fast_url.clone()).send().await?;
        assert_eq!(resp3.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            resp3
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
            Some("1")
        );

        // Release first request
        gate1.notify_one();
        let res1 = h1.await??;
        assert_eq!(res1.status(), StatusCode::OK);

        // Now a slot is free, fast request succeeds
        let resp4 = client.get(fast_url).send().await?;
        assert_eq!(resp4.status(), StatusCode::OK);

        // Release second request
        gate2.notify_one();
        let res2 = h2.await??;
        assert_eq!(res2.status(), StatusCode::OK);

        Ok(())
    })
    .await
}

#[tokio::test]
async fn concurrency_limit_groups_by_custom_header_and_allows_route_override() -> anyhow::Result<()>
{
    let gate = Arc::new(Notify::new());
    let g = gate.clone();

    let app = Router::new()
        .route(
            "/hold",
            get(move || {
                let g = g.clone();
                async move {
                    g.notified().await;
                    "held"
                }
            }),
        )
        .route("/", get(|| async { "unlimited done" }))
        .route("/unlimited", get(|| async { "unlimited done" }));

    let upstream = serve_http_upstream(app).await?;
    let port = alloc_tcp_port().await?;

    let mut unlimited_route = http_route("/unlimited", upstream.as_str(), None);
    unlimited_route.concurrency_limit = Some(ConcurrencyLimit {
        max: 0,
        header: None,
    });

    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![unlimited_route, http_route("/", upstream.as_str(), None)],
        concurrency_limit: ConcurrencyLimit {
            max: 1,
            header: Some("x-api-key".into()),
        },
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("test", &port)])
        .proxies(vec![http_proxy_entry("proxy", "test", proxy)])
        .build();

    with_server(config, |_| async move {
        let client = reqwest::Client::new();
        let hold_url = port.http_url("/hold");
        let unlimited_url = port.http_url("/unlimited");

        // Key A takes the slot
        let c = client.clone();
        let u = hold_url.clone();
        let h_a = tokio::spawn(async move { c.get(u).header("x-api-key", "key-a").send().await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Key B has its own concurrency slot, so Key B gets in to hold
        let c = client.clone();
        let u = hold_url.clone();
        let h_b = tokio::spawn(async move { c.get(u).header("x-api-key", "key-b").send().await });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Second request with Key A is rejected
        let resp_a2 = client
            .get(hold_url)
            .header("x-api-key", "key-a")
            .send()
            .await?;
        assert_eq!(resp_a2.status(), StatusCode::TOO_MANY_REQUESTS);

        // Route with concurrency override (max: 0) is not limited
        let resp_unlimited = client
            .get(unlimited_url)
            .header("x-api-key", "key-a")
            .send()
            .await?;
        assert_eq!(resp_unlimited.status(), StatusCode::OK);

        // Release both in-flight requests
        gate.notify_waiters();
        assert_eq!(h_a.await??.status(), StatusCode::OK);
        assert_eq!(h_b.await??.status(), StatusCode::OK);

        Ok(())
    })
    .await
}
