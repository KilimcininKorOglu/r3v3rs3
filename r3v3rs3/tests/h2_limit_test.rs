use axum::{Router, extract::Request, routing::get};
use bytes::Bytes;
use http_body_util::Full;
use hyper::Request as HyperRequest;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use r3v3rs3_api::app::{AppConfig, Http2Limits, Http3Limits};
use r3v3rs3_api::port::{PortEntry, PortOptions};
use r3v3rs3_api::proxy::HttpProxy;

mod common;
use common::{
    TestPort, TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route,
    port_entry, wait_for_listener, with_server,
};

/// A port whose options override the HTTP/2 limits.
fn limited_port(
    id: &str,
    listen: r3v3rs3_api::multiaddr::Multiaddr,
    limits: Http2Limits,
) -> PortEntry {
    let mut entry = port_entry(id, listen);
    entry.port.opts = PortOptions {
        http2_limits: Some(limits),
        ..Default::default()
    };
    entry
}

fn proxy(upstream: &str) -> HttpProxy {
    proxy_on("h2.localhost", upstream)
}

fn proxy_on(vhost: &str, upstream: &str) -> HttpProxy {
    HttpProxy {
        vhosts: vec![vhost.parse().unwrap()],
        routes: vec![http_route("/", upstream, None)],
        ..Default::default()
    }
}

async fn version(req: Request) -> String {
    format!("{:?}", req.version())
}

/// A prior-knowledge HTTP/2 client for the plain port of the proxy.
fn h2_client() -> Client<HttpConnector, Full<Bytes>> {
    Client::builder(TokioExecutor::new())
        .http2_only(true)
        .build_http()
}

/// Serves the `/version` upstream on the port and waits until it listens.
async fn serve_version_upstream(upstream_port: &TestPort) -> anyhow::Result<()> {
    let app = Router::new().route("/version", get(version));
    tokio::spawn(axum_server::bind(upstream_port.socket_addr()).serve(app.into_make_service()));
    wait_for_listener(upstream_port.socket_addr()).await
}

/// Sends a prior-knowledge HTTP/2 GET with the `host` header and an optional `x-flood` header,
/// and returns the response status.
async fn h2_status(
    url: &hyper::Uri,
    host: &str,
    flood: Option<&str>,
) -> anyhow::Result<hyper::StatusCode> {
    let mut builder = HyperRequest::builder()
        .uri(url.clone())
        .header("host", host);
    if let Some(flood) = flood {
        builder = builder.header("x-flood", flood);
    }
    let response = h2_client()
        .request(builder.body(Full::new(Bytes::new()))?)
        .await?;
    Ok(response.status())
}

/// Sends a prior-knowledge HTTP/2 GET with the `x-flood` header and returns true when the
/// request fails or its status is not a success.
async fn flood_is_refused(url: &hyper::Uri, host: &str, big: &str) -> bool {
    let result = h2_client()
        .request(
            HyperRequest::builder()
                .uri(url.clone())
                .header("host", host)
                .header("x-flood", big)
                .body(Full::new(Bytes::new()))
                .expect("the flood request itself is valid"),
        )
        .await;
    result.is_err() || !result.as_ref().unwrap().status().is_success()
}

#[tokio::test]
async fn an_oversized_header_list_resets_the_stream_and_the_port_serves_on() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let upstream_port = alloc_tcp_port().await?;

    serve_version_upstream(&upstream_port).await?;

    let upstream_url = upstream_port.http_url("/");
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("h2", &proxy_port)])
        .proxies(vec![http_proxy_entry(
            "p",
            "h2",
            proxy(upstream_url.as_str()),
        )])
        .build();

    with_server(config, |_| check_flood_reset(proxy_port)).await
}

/// Checks that the port serves a normal request, resets an oversized header list, and serves
/// a new connection after the flood attempt.
async fn check_flood_reset(proxy_port: TestPort) -> anyhow::Result<()> {
    wait_for_listener(proxy_port.socket_addr()).await?;
    let url: hyper::Uri = proxy_port.http_url("/version").as_str().parse()?;

    // The hardened builder still serves a normal HTTP/2 request.
    assert_eq!(h2_status(&url, "h2.localhost", None).await?, 200);

    // A header list above the server limit gets the stream reset instead of served.
    let big = "a".repeat(100_000);
    assert!(
        flood_is_refused(&url, "h2.localhost", &big).await,
        "the oversized header list must not be served"
    );

    // The port serves a new connection after the flood attempt.
    assert_eq!(h2_status(&url, "h2.localhost", None).await?, 200);
    Ok(())
}

#[tokio::test]
async fn the_global_limits_apply_and_a_port_override_wins() -> anyhow::Result<()> {
    let global_port = alloc_tcp_port().await?;
    let override_port = alloc_tcp_port().await?;
    let upstream_port = alloc_tcp_port().await?;

    serve_version_upstream(&upstream_port).await?;

    let upstream_url = upstream_port.http_url("/");
    let config = TestStorage::builder()
        .config(AppConfig {
            http2: Http2Limits {
                max_header_list_size: 8192,
                max_pending_accept_reset_streams: 128,
            },
            http3: Http3Limits::default(),
            ..Default::default()
        })
        .ports(vec![
            http_port_entry("global", &global_port),
            limited_port(
                "limited",
                override_port.multiaddr_http(),
                Http2Limits {
                    max_header_list_size: 65536,
                    max_pending_accept_reset_streams: 128,
                },
            ),
        ])
        .proxies(vec![
            http_proxy_entry(
                "g",
                "global",
                proxy_on("g.localhost", upstream_url.as_str()),
            ),
            http_proxy_entry(
                "l",
                "limited",
                proxy_on("l.localhost", upstream_url.as_str()),
            ),
        ])
        .build();

    with_server(config, |_| {
        check_global_and_override(global_port, override_port)
    })
    .await
}

/// Checks that the reduced global limit serves a small request and resets a 10 000 byte header
/// list, while the port override serves the same header list.
async fn check_global_and_override(
    global_port: TestPort,
    override_port: TestPort,
) -> anyhow::Result<()> {
    wait_for_listener(global_port.socket_addr()).await?;
    wait_for_listener(override_port.socket_addr()).await?;
    // A request under the reduced global limit is served.
    let url: hyper::Uri = global_port.http_url("/version").as_str().parse()?;
    assert_eq!(h2_status(&url, "g.localhost", None).await?, 200);

    // A header list above the reduced global limit gets the stream reset.
    let big = "a".repeat(10_000);
    assert!(
        flood_is_refused(&url, "g.localhost", &big).await,
        "the global limit must reject the oversized header list"
    );

    // The port override serves the same header list.
    let url: hyper::Uri = override_port.http_url("/version").as_str().parse()?;
    assert_eq!(h2_status(&url, "l.localhost", Some(&big)).await?, 200);
    Ok(())
}
