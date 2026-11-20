//! Range ports end to end through TPROXY. The tests need Linux, `CAP_NET_ADMIN`, `nft` and
//! `ip`, so they are ignored; `make test-tproxy` runs them in a privileged container.

use crate::common;
use common::{TestStorage, with_server};
use r3v3rs3_api::{
    port::UpstreamServer,
    proxy::{Proxy, ProxyEntry, ProxyKind, TcpProxy, UdpProxy},
};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    time::timeout,
};

/// How often a client tries before the server has installed the TPROXY table.
const ATTEMPTS: usize = 50;
const ATTEMPT_WAIT: Duration = Duration::from_millis(200);

fn storage(listen: &str, upstream: &str, kind: fn(UpstreamServer) -> ProxyKind) -> TestStorage {
    let port = common::port_entry("range", listen.parse().unwrap());
    TestStorage::builder()
        .ports(vec![port])
        .proxies(vec![ProxyEntry {
            id: "rproxy".parse().unwrap(),
            source: None,
            status: None,
            proxy: Proxy {
                ports: vec!["range".parse().unwrap()],
                kind: kind(UpstreamServer::new(upstream.parse().unwrap())),
                ..Default::default()
            },
        }])
        .build()
}

fn udp_kind(server: UpstreamServer) -> ProxyKind {
    ProxyKind::Udp(UdpProxy {
        upstream_servers: vec![server],
        session_idle_timeout: Duration::from_secs(2),
        ..Default::default()
    })
}

fn tcp_kind(server: UpstreamServer) -> ProxyKind {
    ProxyKind::Tcp(TcpProxy {
        upstream_servers: vec![server],
        ..Default::default()
    })
}

/// A UDP upstream that answers every datagram with its own port number.
async fn udp_upstream(addr: &str) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(addr).await?;
    let port = socket.local_addr()?.port().to_string();
    tokio::spawn(async move {
        let mut buf = [0; 64];
        while let Ok((_, from)) = socket.recv_from(&mut buf).await {
            let _ = socket.send_to(port.as_bytes(), from).await;
        }
    });
    Ok(())
}

/// Sends to `target` until a reply comes, and returns the reply and its source.
async fn udp_exchange(target: SocketAddr) -> anyhow::Result<(String, SocketAddr)> {
    let client = UdpSocket::bind("127.0.0.1:0").await?;
    let mut buf = [0; 64];
    for _ in 0..ATTEMPTS {
        client.send_to(b"ping", target).await?;
        if let Ok(received) = timeout(ATTEMPT_WAIT, client.recv_from(&mut buf)).await {
            let (size, from) = received?;
            return Ok((String::from_utf8_lossy(&buf[..size]).into_owned(), from));
        }
    }
    anyhow::bail!("no reply from {target}")
}

/// A datagram to the third port of the range reaches the third port of the upstream range
/// (each port maps to the port at its position). The reply comes from the port the client sent
/// to: RFC 8085 section 5.1 has a UDP client check the source of each reply, so a reply from the
/// first port of the range would be dropped.
#[tokio::test]
#[ignore = "needs Linux, CAP_NET_ADMIN, nft and ip (make test-tproxy)"]
async fn a_udp_range_port_reaches_the_upstream_port_at_its_position() -> anyhow::Result<()> {
    udp_upstream("127.0.0.1:40202").await?;
    let config = storage(
        "/ip4/127.0.0.1/udp/40100-40104",
        "/ip4/127.0.0.1/udp/40200-40204",
        udp_kind,
    );
    with_server(config, |_| async move {
        let target: SocketAddr = "127.0.0.1:40102".parse()?;
        let (reply, from) = udp_exchange(target).await?;
        assert_eq!(reply, "40202", "the datagram reached another upstream port");
        assert_eq!(from, target, "the reply left from another port");
        Ok(())
    })
    .await
}

/// A TCP upstream that writes its own port number to every connection.
async fn tcp_upstream(addr: &str) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    let port = listener.local_addr()?.port().to_string();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = stream.write_all(port.as_bytes()).await;
        }
    });
    Ok(())
}

/// Connects to `target` until the proxy answers, and returns what the upstream wrote.
async fn tcp_exchange(target: SocketAddr) -> anyhow::Result<String> {
    for _ in 0..ATTEMPTS {
        if let Ok(mut stream) = TcpStream::connect(target).await {
            let mut reply = String::new();
            let read = timeout(Duration::from_secs(2), stream.read_to_string(&mut reply)).await;
            if read.is_ok_and(|r| r.is_ok()) && !reply.is_empty() {
                return Ok(reply);
            }
        }
        tokio::time::sleep(ATTEMPT_WAIT).await;
    }
    anyhow::bail!("no reply from {target}")
}

/// A connection to the fourth port of the range reaches the fourth port of the upstream range.
#[tokio::test]
#[ignore = "needs Linux, CAP_NET_ADMIN, nft and ip (make test-tproxy)"]
async fn a_tcp_range_port_reaches_the_upstream_port_at_its_position() -> anyhow::Result<()> {
    tcp_upstream("127.0.0.1:41203").await?;
    let config = storage(
        "/ip4/127.0.0.1/tcp/41100-41104",
        "/ip4/127.0.0.1/tcp/41200-41204",
        tcp_kind,
    );
    with_server(config, |_| async move {
        let reply = tcp_exchange("127.0.0.1:41103".parse()?).await?;
        assert_eq!(
            reply, "41203",
            "the connection reached another upstream port"
        );
        Ok(())
    })
    .await
}
