use r3v3rs3_api::{
    port::{Port, PortEntry, UpstreamServer},
    proxy::{Proxy, ProxyEntry, ProxyKind, UdpProxy},
};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::UdpSocket, sync::mpsc, time::timeout};
mod common;
use common::{TestPort, TestStorage, alloc_udp_port, with_server};

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
            status: None,
        }])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            status: None,
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
        check_interleaved_clients(&proxy_port).await
    })
    .await
}

/// Checks that two clients each get the replies to their own packets, also when the packets
/// interleave.
async fn check_interleaved_clients(proxy_port: &TestPort) -> anyhow::Result<()> {
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
}

#[tokio::test]
async fn udp_session_closes_after_the_idle_timeout() -> anyhow::Result<()> {
    let upstream_port = alloc_udp_port().await?;
    let proxy_port = alloc_udp_port().await?;
    let mut sources = start_echo_upstream(&upstream_port).await?;
    let config = udp_storage(&proxy_port, &upstream_port, Duration::from_millis(300));

    with_server(config, |_| async move {
        check_idle_session_closes(&proxy_port, &mut sources).await
    })
    .await
}

/// Sends a packet and checks that the echo of the upstream comes back to the client.
async fn send_and_expect_echo(
    client: &UdpSocket,
    proxy: SocketAddr,
    payload: &[u8],
) -> anyhow::Result<()> {
    client.send_to(payload, proxy).await?;
    assert_eq!(recv_reply(client, proxy).await?, payload);
    Ok(())
}

/// Checks that a session keeps its upstream socket while it is active and gets a new one after
/// the idle timeout.
async fn check_idle_session_closes(
    proxy_port: &TestPort,
    sources: &mut mpsc::UnboundedReceiver<SocketAddr>,
) -> anyhow::Result<()> {
    let proxy = proxy_port.socket_addr();
    let client = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;

    send_and_expect_echo(&client, proxy, b"one").await?;
    send_and_expect_echo(&client, proxy, b"two").await?;
    let first = sources.recv().await;
    assert_eq!(sources.recv().await, first, "the session is still open");

    tokio::time::sleep(Duration::from_millis(900)).await;
    send_and_expect_echo(&client, proxy, b"three").await?;
    assert_ne!(
        sources.recv().await,
        first,
        "a new session has a new socket"
    );
    Ok(())
}

#[tokio::test]
async fn udp_datagram_size_limits_payload() -> anyhow::Result<()> {
    let upstream_port = alloc_udp_port().await?;
    let proxy_port = alloc_udp_port().await?;
    let _sources = start_echo_upstream(&upstream_port).await?;
    let config = udp_storage_with_size(&proxy_port, &upstream_port, Duration::from_secs(60), 512);

    with_server(config, |_| async move {
        let proxy = proxy_port.socket_addr();
        let client = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;

        let large_packet = vec![0x42; 1000];
        client.send_to(&large_packet, proxy).await?;
        let reply = recv_reply(&client, proxy).await?;
        assert_eq!(reply.len(), 512);
        assert_eq!(reply, vec![0x42; 512]);
        Ok(())
    })
    .await
}

fn udp_storage_with_size(
    proxy_port: &TestPort,
    upstream_port: &TestPort,
    idle: Duration,
    max_datagram_size: usize,
) -> TestStorage {
    TestStorage::builder()
        .ports(vec![PortEntry {
            id: "test".parse().unwrap(),
            port: Port {
                active: true,
                name: String::new(),
                listen: proxy_port.multiaddr_udp(),
                opts: Default::default(),
            },
            status: None,
        }])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            status: None,
            proxy: Proxy {
                ports: vec!["test".parse().unwrap()],
                kind: ProxyKind::Udp(UdpProxy {
                    upstream_servers: vec![UpstreamServer::new(upstream_port.multiaddr_udp())],
                    session_idle_timeout: idle,
                    max_datagram_size,
                    ..Default::default()
                }),
                ..Default::default()
            },
        }])
        .build()
}

async fn recv_reply(client: &UdpSocket, proxy: SocketAddr) -> anyhow::Result<Vec<u8>> {
    let mut buf = [0; 1024];
    let (size, addr) = timeout(Duration::from_secs(5), client.recv_from(&mut buf)).await??;
    assert_eq!(addr, proxy);
    Ok(buf[..size].to_vec())
}
