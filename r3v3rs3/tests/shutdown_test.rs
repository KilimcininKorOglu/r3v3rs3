//! The graceful drain: an open connection keeps streaming after the shutdown, and the listener
//! refuses the new connections while the old ones run.

use r3v3rs3_api::{
    port::UpstreamServer,
    proxy::{Proxy, ProxyEntry, ProxyKind, TcpProxy},
};
use std::time::Duration;
mod common;
use common::{TestPort, alloc_tcp_port, port_entry, wait_for_listener, wait_until, with_server};

/// Echoes every chunk back until the client closes the connection.
async fn echo_upstream(mut socket: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 64];
    loop {
        match socket.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if socket.write_all(&buf[..n]).await.is_err() {
                    break;
                }
            }
        }
    }
}

#[tokio::test]
async fn shutdown_drains_the_open_connections() -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listen_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    let upstream_addr = listen_port.socket_addr();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(upstream_addr).await.unwrap();
        loop {
            if let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(echo_upstream(socket));
            }
        }
    });

    let config = common::TestStorage::builder()
        .ports(vec![port_entry("test", proxy_port.multiaddr_tcp())])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            proxy: tcp_proxy_to(&listen_port),
        }])
        .build();

    with_server(config, |channels| async move {
        let proxy_addr = proxy_port.socket_addr();
        wait_for_listener(proxy_addr).await?;

        let mut open = tokio::net::TcpStream::connect(proxy_addr).await?;
        open.write_all(b"before").await?;
        let mut buf = [0u8; 6];
        open.read_exact(&mut buf).await?;
        assert_eq!(&buf, b"before");

        channels.shutdown();
        tokio::time::sleep(Duration::from_millis(300)).await;

        // The old connection still streams after the shutdown.
        open.write_all(b"after").await?;
        open.read_exact(&mut buf[..5]).await?;
        assert_eq!(&buf[..5], b"after");

        // The listener is gone, so the kernel refuses the new connections.
        wait_until(
            || async move { Ok(tokio::net::TcpStream::connect(proxy_addr).await.is_err()) },
            "the listener still accepts connections after the shutdown",
        )
        .await?;

        // Dropping the old connection lets the drain end, so `with_server` returns.
        drop(open);
        Ok(())
    })
    .await
}

fn tcp_proxy_to(upstream: &TestPort) -> Proxy {
    Proxy {
        ports: vec!["test".parse().unwrap()],
        kind: ProxyKind::Tcp(TcpProxy {
            client_cert: None,
            connect_timeout: r3v3rs3_api::upstream::DEFAULT_CONNECT_TIMEOUT,
            load_balancing: Default::default(),
            health_check: Default::default(),
            circuit_breaker: Default::default(),
            proxy_protocol: None,
            upstream_servers: vec![UpstreamServer::new(upstream.multiaddr_tcp())],
        }),
        ..Default::default()
    }
}
