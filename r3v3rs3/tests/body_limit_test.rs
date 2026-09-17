use axum::Router;
use r3v3rs3_api::proxy::HttpProxy;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

mod common;
use common::{TestStorage, serve_http_upstream};
use common::{alloc_tcp_port, http_port_entry, http_proxy_entry, http_route, with_server};

/// A proxy whose routes answer with the request body: `/` has the proxy limit of 16 bytes, `/small`
/// has a limit of 4 bytes and `/free` has no limit.
async fn limited_proxy() -> anyhow::Result<HttpProxy> {
    let echo =
        serve_http_upstream(Router::new().fallback(|body: String| async move { body })).await?;
    let route = |path: &str, max_body_size: Option<u64>| {
        let mut route = http_route(path, echo.as_str(), None);
        route.max_body_size = max_body_size;
        route
    };
    Ok(HttpProxy {
        routes: vec![
            route("/", None),
            route("/small", Some(4)),
            route("/free", Some(0)),
        ],
        max_body_size: 16,
        ..Default::default()
    })
}

/// Sends a chunked HTTP/1.1 request without `Content-Length`, and returns the status line.
async fn post_chunked(addr: SocketAddr, path: &str, chunks: &[&str]) -> anyhow::Result<String> {
    let mut stream = TcpStream::connect(addr).await?;
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    for chunk in chunks {
        request.push_str(&format!("{:x}\r\n{chunk}\r\n", chunk.len()));
    }
    request.push_str("0\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    // The proxy can close the connection after the response, before it reads the whole request.
    let _ = stream.read_to_end(&mut response).await;
    let response = String::from_utf8_lossy(&response);
    Ok(response.lines().next().unwrap_or_default().to_string())
}

#[tokio::test]
async fn request_bodies_are_limited() -> anyhow::Result<()> {
    let port = alloc_tcp_port().await?;
    let config = TestStorage::builder()
        .ports(vec![http_port_entry("blimit", &port)])
        .proxies(vec![http_proxy_entry(
            "blimitp",
            "blimit",
            limited_proxy().await?,
        )])
        .build();

    with_server(config, |_| async move {
        let http1 = reqwest::Client::new();
        let http2 = reqwest::Client::builder().http2_prior_knowledge().build()?;
        for client in [&http1, &http2] {
            let post = |path: &str, body: &'static str| {
                let request = client.post(port.http_url(path)).body(body);
                async move {
                    let res = request.send().await?;
                    anyhow::Ok((res.status().as_u16(), res.text().await?))
                }
            };
            let sixteen = "0123456789abcdef";
            assert_eq!(post("/", sixteen).await?, (200, sixteen.to_string()));
            assert_eq!(post("/", "0123456789abcdefg").await?.0, 413);
            assert_eq!(post("/small", "abcd").await?.0, 200);
            assert_eq!(post("/small", "abcde").await?.0, 413);
            let long = "0123456789abcdef0123456789abcdef";
            assert_eq!(post("/free", long).await?, (200, long.to_string()));
        }

        let addr = port.socket_addr();
        let chunks = ["0123456789", "abcdef"];
        assert_eq!(post_chunked(addr, "/", &chunks).await?, "HTTP/1.1 200 OK");
        let chunks = ["0123456789", "abcdefg"];
        assert_eq!(
            post_chunked(addr, "/", &chunks).await?,
            "HTTP/1.1 413 Payload Too Large"
        );
        Ok(())
    })
    .await
}
