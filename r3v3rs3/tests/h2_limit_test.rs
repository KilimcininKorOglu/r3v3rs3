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
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, port_entry,
    wait_for_listener, with_server,
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

#[tokio::test]
async fn an_oversized_header_list_resets_the_stream_and_the_port_serves_on() -> anyhow::Result<()> {
    let proxy_port = alloc_tcp_port().await?;
    let upstream_port = alloc_tcp_port().await?;

    let app = Router::new().route("/version", get(version));
    tokio::spawn(axum_server::bind(upstream_port.socket_addr()).serve(app.into_make_service()));
    wait_for_listener(upstream_port.socket_addr()).await?;

    let upstream_url = upstream_port.http_url("/");
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("h2", &proxy_port)])
        .proxies(vec![http_proxy_entry(
            "p",
            "h2",
            proxy(upstream_url.as_str()),
        )])
        .build();

    with_server(config, |_| async move {
        wait_for_listener(proxy_port.socket_addr()).await?;
        let url: hyper::Uri = proxy_port.http_url("/version").as_str().parse()?;

        // The hardened builder still serves a normal HTTP/2 request.
        let response = h2_client()
            .request(
                HyperRequest::builder()
                    .uri(url.clone())
                    .header("host", "h2.localhost")
                    .body(Full::new(Bytes::new()))?,
            )
            .await?;
        assert_eq!(response.status(), 200);

        // A header list above the server limit gets the stream reset instead of served.
        let big = "a".repeat(100_000);
        let result = h2_client()
            .request(
                HyperRequest::builder()
                    .uri(url.clone())
                    .header("host", "h2.localhost")
                    .header("x-flood", &big)
                    .body(Full::new(Bytes::new()))
                    .expect("the flood request itself is valid"),
            )
            .await;
        assert!(
            result.is_err() || !result.as_ref().unwrap().status().is_success(),
            "the oversized header list must not be served"
        );

        // The port serves a new connection after the flood attempt.
        let response = h2_client()
            .request(
                HyperRequest::builder()
                    .uri(url.clone())
                    .header("host", "h2.localhost")
                    .body(Full::new(Bytes::new()))?,
            )
            .await?;
        assert_eq!(response.status(), 200);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn the_global_limits_apply_and_a_port_override_wins() -> anyhow::Result<()> {
    let global_port = alloc_tcp_port().await?;
    let override_port = alloc_tcp_port().await?;
    let upstream_port = alloc_tcp_port().await?;

    let app = Router::new().route("/version", get(version));
    tokio::spawn(axum_server::bind(upstream_port.socket_addr()).serve(app.into_make_service()));
    wait_for_listener(upstream_port.socket_addr()).await?;

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

    with_server(config, |_| async move {
        wait_for_listener(global_port.socket_addr()).await?;
        wait_for_listener(override_port.socket_addr()).await?;
        // A request under the reduced global limit is served.
        let url: hyper::Uri = global_port.http_url("/version").as_str().parse()?;
        let response = h2_client()
            .request(
                HyperRequest::builder()
                    .uri(url.clone())
                    .header("host", "g.localhost")
                    .body(Full::new(Bytes::new()))?,
            )
            .await?;
        assert_eq!(response.status(), 200);

        // A header list above the reduced global limit gets the stream reset.
        let big = "a".repeat(10_000);
        let result = h2_client()
            .request(
                HyperRequest::builder()
                    .uri(url.clone())
                    .header("host", "g.localhost")
                    .header("x-flood", &big)
                    .body(Full::new(Bytes::new()))
                    .expect("the flood request itself is valid"),
            )
            .await;
        assert!(
            result.is_err() || !result.as_ref().unwrap().status().is_success(),
            "the global limit must reject the oversized header list"
        );

        // The port override serves the same header list.
        let url: hyper::Uri = override_port.http_url("/version").as_str().parse()?;
        let response = h2_client()
            .request(
                HyperRequest::builder()
                    .uri(url)
                    .header("host", "l.localhost")
                    .header("x-flood", &big)
                    .body(Full::new(Bytes::new()))?,
            )
            .await?;
        assert_eq!(response.status(), 200);
        Ok(())
    })
    .await
}
