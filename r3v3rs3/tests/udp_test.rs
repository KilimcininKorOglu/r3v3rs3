use r3v3rs3_api::{
    port::{Port, PortEntry, UpstreamServer},
    proxy::{Proxy, ProxyEntry, ProxyKind, UdpProxy},
};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::UdpSocket, sync::mpsc, time::timeout};
mod common;
use common::{alloc_udp_port, with_server, TestPort, TestStorage};

/// Starts an upstream server that echoes each packet and reports the source address of the packet.
async fn start_echo_upstream(
    port: &TestPort,
) -> anyhow::Result<mpsc::UnboundedReceiver<SocketAddr>> {
    let upstream = UdpSocket::bind(port.socket_addr()).await?;
    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut buf = [0; 1024];
        while let Ok((size, addr)) = upstream.recv_from(&mut buf).await {
            let _ = sender.send(addr);
            let _ = upstream.send_to(&buf[..size], addr).await;
        }
    });
    Ok(receiver)
}

fn udp_storage(proxy_port: &TestPort, upstream_port: &TestPort, idle: Duration) -> TestStorage {
    TestStorage::builder()
        .ports(vec![PortEntry {
            id: "test".parse().unwrap(),
            port: Port {
                active: true,
                name: String::new(),
                listen: proxy_port.multiaddr_udp(),
                opts: Default::default(),
            },
        }])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            proxy: Proxy {
                ports: vec!["test".parse().unwrap()],
                kind: ProxyKind::Udp(UdpProxy {
                    upstream_servers: vec![UpstreamServer::new(upstream_port.multiaddr_udp())],
                    session_idle_timeout: idle,
                    ..Default::default()
                }),
                ..Default::default()
            },
        }])
        .build()
}

#[tokio::test]
async fn udp_proxy() -> anyhow::Result<()> {
    let upstream_port = alloc_udp_port().await?;
    let proxy_port = alloc_udp_port().await?;
    let _sources = start_echo_upstream(&upstream_port).await?;
    let config = udp_storage(&proxy_port, &upstream_port, Duration::from_secs(60));

    with_server(config, |_| async move {
        let proxy = proxy_port.socket_addr();
        let first = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;
        let second = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;

        // Each client gets the reply to its own packets, also when the packets interleave.
        for _ in 0..2 {
            first.send_to(b"first", proxy).await?;
            second.send_to(b"second", proxy).await?;
            assert_eq!(recv_reply(&first, proxy).await?, b"first");
            assert_eq!(recv_reply(&second, proxy).await?, b"second");
        }
        Ok(())
    })
    .await
}

#[tokio::test]
async fn udp_session_closes_after_the_idle_timeout() -> anyhow::Result<()> {
    let upstream_port = alloc_udp_port().await?;
    let proxy_port = alloc_udp_port().await?;
    let mut sources = start_echo_upstream(&upstream_port).await?;
    let config = udp_storage(&proxy_port, &upstream_port, Duration::from_millis(300));

    with_server(config, |_| async move {
        let proxy = proxy_port.socket_addr();
        let client = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;

        client.send_to(b"one", proxy).await?;
        assert_eq!(recv_reply(&client, proxy).await?, b"one");
        client.send_to(b"two", proxy).await?;
        assert_eq!(recv_reply(&client, proxy).await?, b"two");
        let first = sources.recv().await;
        assert_eq!(sources.recv().await, first, "the session is still open");

        tokio::time::sleep(Duration::from_millis(900)).await;
        client.send_to(b"three", proxy).await?;
        assert_eq!(recv_reply(&client, proxy).await?, b"three");
        assert_ne!(
            sources.recv().await,
            first,
            "a new session has a new socket"
        );
        Ok(())
    })
    .await
}

async fn recv_reply(client: &UdpSocket, proxy: SocketAddr) -> anyhow::Result<Vec<u8>> {
    let mut buf = [0; 1024];
    let (size, addr) = timeout(Duration::from_secs(5), client.recv_from(&mut buf)).await??;
    assert_eq!(addr, proxy);
    Ok(buf[..size].to_vec())
}
