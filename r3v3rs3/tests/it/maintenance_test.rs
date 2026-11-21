use axum::Router;
use r3v3rs3::server::ServerChannels;
use r3v3rs3::server::rpc::proxies::UpdateProxy;
use r3v3rs3_api::proxy::{HttpProxy, MaintenanceConfig, ProxyEntry, ProxyKind};
use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, RETRY_AFTER};

use crate::common;
use common::{
    TestStorage, alloc_tcp_port, call, http_port_entry, http_proxy_entry, http_route,
    serve_http_upstream, with_server,
};

async fn check_maintenance(
    client: &reqwest::Client,
    url: &str,
    status: StatusCode,
    retry: &str,
    content_type_prefix: &str,
    body_substring: &str,
) -> anyhow::Result<()> {
    let resp = client.get(url).send().await?;
    assert_eq!(resp.status(), status);
    assert_eq!(
        resp.headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok()),
        Some(retry)
    );
    let content_type = resp.headers().get(CONTENT_TYPE).unwrap().to_str()?;
    assert!(content_type.starts_with(content_type_prefix));
    let body = resp.text().await?;
    assert!(body.contains(body_substring));
    Ok(())
}

async fn check_upstream(client: &reqwest::Client, url: &str, expected: &str) -> anyhow::Result<()> {
    let resp = client.get(url).send().await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await?, expected);
    Ok(())
}

fn make_entry(upstream: &str, maintenance: MaintenanceConfig) -> ProxyEntry {
    ProxyEntry {
        id: "px1".parse().unwrap(),
        source: None,
        status: None,
        proxy: r3v3rs3_api::proxy::Proxy {
            ports: vec!["p1".parse().unwrap()],
            kind: ProxyKind::Http(Box::new(HttpProxy {
                routes: vec![http_route("/", upstream, None)],
                maintenance,
                ..Default::default()
            })),
            ..Default::default()
        },
    }
}

async fn run_maintenance_steps(
    mut channels: ServerChannels,
    url: String,
    upstream: String,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    check_maintenance(
        &client,
        &url,
        StatusCode::SERVICE_UNAVAILABLE,
        "60",
        "text/html",
        "Under Maintenance",
    )
    .await?;

    let off = MaintenanceConfig {
        enabled: false,
        status: 503,
        body: String::new(),
        retry_after: None,
        error_page: false,
    };
    call(
        &mut channels,
        UpdateProxy {
            entry: make_entry(&upstream, off),
        },
    )
    .await??;
    check_upstream(&client, &url, "upstream response").await?;

    check_themed_maintenance(&mut channels, &client, &url, &upstream).await?;

    let custom = MaintenanceConfig {
        enabled: true,
        status: 503,
        body: "Custom system maintenance in progress.".into(),
        retry_after: Some(180),
        error_page: false,
    };
    call(
        &mut channels,
        UpdateProxy {
            entry: make_entry(&upstream, custom),
        },
    )
    .await??;
    check_maintenance(
        &client,
        &url,
        StatusCode::SERVICE_UNAVAILABLE,
        "180",
        "text/plain",
        "Custom system maintenance",
    )
    .await?;
    Ok(())
}

/// A stopped platform app: the page of the error page theme, never the fixed maintenance page
/// and never the custom body, with the Retry-After of the setting.
async fn check_themed_maintenance(
    channels: &mut ServerChannels,
    client: &reqwest::Client,
    url: &str,
    upstream: &str,
) -> anyhow::Result<()> {
    let themed = MaintenanceConfig {
        enabled: true,
        status: 503,
        body: "Custom system maintenance in progress.".into(),
        retry_after: Some(30),
        error_page: true,
    };
    call(
        channels,
        UpdateProxy {
            entry: make_entry(upstream, themed),
        },
    )
    .await??;
    check_maintenance(
        client,
        url,
        StatusCode::SERVICE_UNAVAILABLE,
        "30",
        "text/html",
        "Service Unavailable",
    )
    .await?;
    let body = client.get(url).send().await?.text().await?;
    assert!(!body.contains("Under Maintenance"), "{body}");
    assert!(!body.contains("Custom system maintenance"), "{body}");
    Ok(())
}

/// The proxy of a stopped platform app has a route without servers. It must answer the 503 page
/// of the error page theme, never the 502 of a route without an upstream server.
#[tokio::test]
async fn a_route_without_servers_in_themed_maintenance_answers_503() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let proxy = HttpProxy {
        routes: vec![r3v3rs3_api::proxy::Route::default()],
        maintenance: MaintenanceConfig {
            enabled: true,
            error_page: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("p1", &port)])
        .proxies(vec![http_proxy_entry("px1", "p1", proxy)])
        .build();
    let url = port.http_url("/");
    with_server(config, |_channels| async move {
        let resp = reqwest::get(url.as_str()).await?;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = resp.text().await?;
        assert!(body.contains("Service Unavailable"), "{body}");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn maintenance_mode_serves_maintenance_page_and_toggles() -> anyhow::Result<()> {
    let upstream =
        serve_http_upstream(Router::new().fallback(|| async { "upstream response" })).await?;
    let port = alloc_tcp_port().await?;

    let initial_maint = MaintenanceConfig {
        enabled: true,
        status: 503,
        body: String::new(),
        retry_after: Some(60),
        error_page: false,
    };
    let proxy = HttpProxy {
        routes: vec![http_route("/", upstream.as_str(), None)],
        maintenance: initial_maint,
        ..Default::default()
    };

    let config = TestStorage::builder()
        .ports(vec![http_port_entry("p1", &port)])
        .proxies(vec![http_proxy_entry("px1", "p1", proxy)])
        .build();

    let up = upstream.to_string();
    let url = port.http_url("/");
    with_server(config, |channels| {
        run_maintenance_steps(channels, url.to_string(), up)
    })
    .await
}
