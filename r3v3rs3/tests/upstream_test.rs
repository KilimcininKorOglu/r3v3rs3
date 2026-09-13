use axum::{routing::get, Router};
use r3v3rs3_api::{
    multiaddr::Multiaddr,
    port::UpstreamServer,
    proxy::{HttpProxy, ProxyKind, Route, TcpProxy},
    upstream::UpstreamTimeouts,
};
use std::{
    future::IntoFuture,
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream},
    time::timeout,
};

mod common;
use common::{alloc_tcp_port, http_route, port_entry, proxy_entry, with_server};
use common::{TestPort, TestStorage};

/// Starts a listener that accepts TCP connections and never sends a byte, so a TLS handshake
/// with it does not finish.
async fn start_silent_upstream() -> anyhow::Result<TestPort> {
    let port = alloc_tcp_port().await?;
    let listener = TcpListener::bind(port.socket_addr()).await?;
    tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            open.push(stream);
        }
    });
    Ok(port)
}

fn timeouts(connect: Duration, request: Duration) -> UpstreamTimeouts {
    UpstreamTimeouts { connect, request }
}

/// Builds a storage with one port that listens on `listen` and one proxy of `kind` on it.
fn single_proxy(listen: Multiaddr, kind: ProxyKind) -> TestStorage {
    TestStorage::builder()
        .ports(vec![port_entry("test", listen)])
        .proxies(vec![proxy_entry("test2", "test", kind)])
        .build()
}

fn http(proxy: HttpProxy) -> ProxyKind {
    ProxyKind::Http(Box::new(proxy))
}

#[tokio::test]
async fn http_request_timeout_returns_gateway_timeout() -> anyhow::Result<()> {
    let upstream_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    let app = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            "slow"
        }),
    );
    let listener = TcpListener::bind(upstream_port.socket_addr()).await?;
    tokio::spawn(axum::serve(listener, app).into_future());

    let upstream = upstream_port.http_url("/");
    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![
            Route {
                timeouts: Some(timeouts(Duration::from_secs(10), Duration::from_secs(5))),
                ..http_route("/patient", upstream.as_str(), None)
            },
            http_route("/", upstream.as_str(), None),
        ],
        timeouts: timeouts(Duration::from_secs(10), Duration::from_millis(200)),
        ..Default::default()
    };
    let config = single_proxy(proxy_port.multiaddr_http(), http(proxy));

    with_server(config, |_| async move {
        let started = Instant::now();
        let resp = reqwest::get(proxy_port.http_url("/slow")).await?;
        assert_eq!(resp.status(), 504);
        assert!(started.elapsed() < Duration::from_secs(1));

        let resp = reqwest::get(proxy_port.http_url("/patient/slow")).await?;
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().await?, "slow");
        Ok(())
    })
    .await
}

#[tokio::test]
async fn http_connect_timeout_covers_the_tls_handshake() -> anyhow::Result<()> {
    let upstream_port = start_silent_upstream().await?;
    let proxy_port = alloc_tcp_port().await?;

    let upstream = format!("https://localhost:{}/", upstream_port.socket_addr().port());
    let proxy = HttpProxy {
        vhosts: vec!["localhost".parse().unwrap()],
        routes: vec![http_route("/", &upstream, None)],
        // The request timeout is disabled, so only the connect timeout can end the request.
        timeouts: timeouts(Duration::from_millis(300), Duration::ZERO),
        ..Default::default()
    };
    let config = single_proxy(proxy_port.multiaddr_http(), http(proxy));

    with_server(config, |_| async move {
        let request = reqwest::get(proxy_port.http_url("/"));
        let resp = timeout(Duration::from_secs(5), request).await??;
        assert_eq!(resp.status(), 504);
        Ok(())
    })
    .await
}

#[tokio::test]
async fn tcp_connect_timeout_closes_the_client_connection() -> anyhow::Result<()> {
    let upstream_port = start_silent_upstream().await?;
    let proxy_port = alloc_tcp_port().await?;

    let tcp = TcpProxy {
        upstream_servers: vec![UpstreamServer {
            addr: format!(
                "/dns/localhost/tcp/{}/tls",
                upstream_port.socket_addr().port()
            )
            .parse()
            .unwrap(),
        }],
        connect_timeout: Duration::from_millis(300),
        ..Default::default()
    };
    let config = single_proxy(proxy_port.multiaddr_tcp(), ProxyKind::Tcp(tcp));

    with_server(config, |_| async move {
        let mut client = TcpStream::connect(proxy_port.socket_addr()).await?;
        let mut buf = [0; 16];
        let read = timeout(Duration::from_secs(5), client.read(&mut buf)).await?;
        assert_eq!(read.unwrap_or(0), 0);
        Ok(())
    })
    .await
}
