use r3v3rs3_api::{
    port::{Port, PortEntry, UpstreamServer},
    proxy::{Proxy, ProxyEntry, ProxyKind, UdpProxy},
};
use std::{net::SocketAddr, time::Duration};
use tokio::{net::UdpSocket, time::timeout};
mod common;
use common::{alloc_udp_port, with_server, TestStorage};

#[tokio::test]
async fn udp_proxy() -> anyhow::Result<()> {
    let upstream_port = alloc_udp_port().await?;
    let proxy_port = alloc_udp_port().await?;

    let upstream = UdpSocket::bind(upstream_port.socket_addr()).await?;
    tokio::spawn(async move {
        let mut buf = [0; 1024];
        while let Ok((size, addr)) = upstream.recv_from(&mut buf).await {
            let _ = upstream.send_to(&buf[..size], addr).await;
        }
    });

    let config = TestStorage::builder()
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
            proxy: Proxy {
                ports: vec!["test".parse().unwrap()],
                kind: ProxyKind::Udp(UdpProxy {
                    upstream_servers: vec![UpstreamServer {
                        addr: upstream_port.multiaddr_udp(),
                    }],
                }),
                ..Default::default()
            },
        }])
        .build();

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

async fn recv_reply(client: &UdpSocket, proxy: SocketAddr) -> anyhow::Result<Vec<u8>> {
    let mut buf = [0; 1024];
    let (size, addr) = timeout(Duration::from_secs(5), client.recv_from(&mut buf)).await??;
    assert_eq!(addr, proxy);
    Ok(buf[..size].to_vec())
}
