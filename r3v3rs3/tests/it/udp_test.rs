use crate::common;
use common::{TestPort, TestStorage, alloc_udp_port, with_server};
use r3v3rs3::server::rpc::proxies::AddProxy;
use r3v3rs3_api::{
    error::Error,
    port::{Port, PortEntry, UpstreamServer},
    proxy::{Proxy, ProxyEntry, ProxyKind, UdpProxy},
};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::UdpSocket, sync::mpsc, time::timeout};

/// Starts an upstream server that echoes each packet and reports the source address of the packet.
async fn start_echo_upstream(
    port: &TestPort,
) -> anyhow::Result<mpsc::UnboundedReceiver<SocketAddr>> {
    start_upstream(port, |packet| packet.to_vec()).await
}

/// Starts an upstream server that answers each packet with `reply(packet)` and reports the
/// source address of the packet.
async fn start_upstream(
    port: &TestPort,
    reply: fn(&[u8]) -> Vec<u8>,
) -> anyhow::Result<mpsc::UnboundedReceiver<SocketAddr>> {
    let upstream = UdpSocket::bind(port.socket_addr()).await?;
    let (sender, receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut buf = [0; 2048];
        while let Ok((size, addr)) = upstream.recv_from(&mut buf).await {
            let _ = sender.send(addr);
            let _ = upstream.send_to(&reply(&buf[..size]), addr).await;
        }
    });
    Ok(receiver)
}

/// The source address of the next packet that reached the upstream.
async fn next_source(
    sources: &mut mpsc::UnboundedReceiver<SocketAddr>,
) -> anyhow::Result<SocketAddr> {
    timeout(Duration::from_secs(5), sources.recv())
        .await?
        .ok_or_else(|| anyhow::anyhow!("the upstream stopped"))
}

fn udp_storage(proxy_port: &TestPort, upstream_port: &TestPort, idle: Duration) -> TestStorage {
    let default_size = r3v3rs3_api::upstream::DEFAULT_MAX_DATAGRAM_SIZE;
    udp_storage_with_size(proxy_port, upstream_port, idle, default_size)
}

/// The shutdown drains open UDP sessions up to `shutdown_graceful_timeout` (30 s), so the tests
/// that do not test the idle timeout use one that ends soon after their last packet.
const SHORT_IDLE: Duration = Duration::from_secs(2);

#[tokio::test]
async fn udp_proxy() -> anyhow::Result<()> {
    let upstream_port = alloc_udp_port().await?;
    let _sources = start_echo_upstream(&upstream_port).await?;
    let proxy_port = alloc_udp_port().await?;
    let config = udp_storage(&proxy_port, &upstream_port, SHORT_IDLE);

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
    let mut sources = start_echo_upstream(&upstream_port).await?;
    let proxy_port = alloc_udp_port().await?;
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
    let first = next_source(sources).await?;
    assert_eq!(
        next_source(sources).await?,
        first,
        "the session is still open"
    );

    tokio::time::sleep(Duration::from_millis(900)).await;
    send_and_expect_echo(&client, proxy, b"three").await?;
    assert_ne!(
        next_source(sources).await?,
        first,
        "a new session has a new socket"
    );
    Ok(())
}

/// How long a test waits to be sure that a dropped packet does not arrive.
const NO_PACKET_WAIT: Duration = Duration::from_millis(500);

/// A packet of the client above `max_datagram_size` is dropped, not truncated: a truncated
/// packet would reach the upstream as corrupt data that it cannot detect. A packet of exactly
/// the limit passes.
#[tokio::test]
async fn udp_drops_a_client_packet_above_the_size_limit() -> anyhow::Result<()> {
    let upstream_port = alloc_udp_port().await?;
    let mut sources = start_echo_upstream(&upstream_port).await?;
    let proxy_port = alloc_udp_port().await?;
    let config = udp_storage_with_size(&proxy_port, &upstream_port, SHORT_IDLE, 512);

    with_server(config, |_| async move {
        let proxy = proxy_port.socket_addr();
        let client = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;
        client.send_to(&[0x42; 513], proxy).await?;
        let reached = timeout(NO_PACKET_WAIT, sources.recv()).await;
        assert!(
            reached.is_err(),
            "the oversized packet reached the upstream"
        );
        send_and_expect_echo(&client, proxy, &[0x42; 512]).await
    })
    .await
}

/// A reply of the upstream above `max_datagram_size` is dropped, not truncated, and the session
/// still carries the next reply.
#[tokio::test]
async fn udp_drops_an_upstream_reply_above_the_size_limit() -> anyhow::Result<()> {
    let upstream_port = alloc_udp_port().await?;
    let reply = |packet: &[u8]| match packet {
        b"big" => vec![0x24; 513],
        other => other.to_vec(),
    };
    let _sources = start_upstream(&upstream_port, reply).await?;
    let proxy_port = alloc_udp_port().await?;
    let config = udp_storage_with_size(&proxy_port, &upstream_port, SHORT_IDLE, 512);

    with_server(config, |_| async move {
        let proxy = proxy_port.socket_addr();
        let client = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;
        client.send_to(b"big", proxy).await?;
        let mut buf = [0; 1024];
        let reply = timeout(NO_PACKET_WAIT, client.recv_from(&mut buf)).await;
        assert!(reply.is_err(), "the oversized reply reached the client");
        send_and_expect_echo(&client, proxy, b"small").await
    })
    .await
}

/// RFC 8085 section 5.1: a UDP receiver checks the source address of each datagram. A packet
/// that another host sends to the upstream socket of a session is not the reply of the upstream,
/// so it never reaches the client; the reply of the upstream still does.
#[tokio::test]
async fn udp_drops_a_packet_that_does_not_come_from_the_upstream() -> anyhow::Result<()> {
    // The upstream holds its port before the proxy port is picked, so the two never meet: a
    // proxy without its own port would let the client reach the upstream directly.
    let upstream_port = alloc_udp_port().await?;
    let mut sources = start_echo_upstream(&upstream_port).await?;
    let proxy_port = alloc_udp_port().await?;
    let config = udp_storage(&proxy_port, &upstream_port, SHORT_IDLE);

    with_server(config, |_| async move {
        check_spoofed_reply(&proxy_port, &mut sources).await
    })
    .await
}

/// Sends a packet from another socket to the session socket of the proxy and checks that the
/// client gets only the echo of the upstream.
async fn check_spoofed_reply(
    proxy_port: &TestPort,
    sources: &mut mpsc::UnboundedReceiver<SocketAddr>,
) -> anyhow::Result<()> {
    let proxy = proxy_port.socket_addr();
    let client = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;
    send_and_expect_echo(&client, proxy, b"open").await?;
    let session = next_source(sources).await?;
    // The packet must have passed the proxy: the upstream sees the session socket, never the
    // client itself.
    assert_ne!(
        session,
        client.local_addr()?,
        "the client reached the upstream without the proxy"
    );

    let attacker = UdpSocket::bind(SocketAddr::new(proxy.ip(), 0)).await?;
    attacker.send_to(b"spoofed", session).await?;
    let mut buf = [0; 1024];
    let reply = timeout(NO_PACKET_WAIT, client.recv_from(&mut buf)).await;
    assert!(
        reply.is_err(),
        "a packet of another host reached the client: {:?}",
        reply.map(|res| res.map(|(size, _)| buf[..size].to_vec()))
    );
    send_and_expect_echo(&client, proxy, b"still").await
}

/// A range port sends each port to the upstream port at the same position, so a proxy on it
/// needs upstream ranges of the same size. A single upstream port, or a range of another size,
/// leaves ports without an upstream port and is refused before it is saved.
#[tokio::test]
async fn a_range_port_refuses_upstreams_of_another_size() -> anyhow::Result<()> {
    let mut range = common::port_entry("range", "/ip4/127.0.0.1/udp/49160-49169".parse()?);
    // The test checks the saved configuration only, so the port does not listen.
    range.port.active = false;
    let config = TestStorage::builder().ports(vec![range]).build();
    with_server(config, |mut channels| async move {
        let add = |upstream: &str| AddProxy {
            entry: Proxy {
                ports: vec!["range".parse().unwrap()],
                kind: ProxyKind::Udp(UdpProxy {
                    upstream_servers: vec![UpstreamServer::new(upstream.parse().unwrap())],
                    ..Default::default()
                }),
                ..Default::default()
            },
            owner: None,
        };
        for wrong in ["/ip4/127.0.0.1/udp/3478", "/ip4/127.0.0.1/udp/50000-50008"] {
            let result = common::call(&mut channels, add(wrong)).await?;
            assert!(
                matches!(result, Err(Error::UpstreamRangeMismatch { .. })),
                "{wrong} was not refused: {result:?}"
            );
        }
        common::call(&mut channels, add("/ip4/127.0.0.1/udp/50000-50009")).await??;
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
