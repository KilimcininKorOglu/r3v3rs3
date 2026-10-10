use axum::{Router, extract::Request, routing::get};
use bytes::Bytes;
use http_body_util::Full;
use hyper::Request as HyperRequest;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use r3v3rs3_api::proxy::HttpProxy;

mod common;
use common::{
    TestStorage, alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server,
};

fn proxy(upstream: &str) -> HttpProxy {
    HttpProxy {
        vhosts: vec!["h2.localhost".parse().unwrap()],
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
