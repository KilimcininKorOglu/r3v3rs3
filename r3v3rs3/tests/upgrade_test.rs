//! The zero-downtime upgrade: a SIGUSR2 hands the listeners to a new process, which serves the
//! port while the old one drains.

use r3v3rs3::config::sqlite::SqliteStorage;
use r3v3rs3::config::storage::Storage as _;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::port::UpstreamServer;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyKind, TcpProxy};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
mod common;
use common::{alloc_tcp_port, port_entry};

/// Echoes every chunk back until the client closes the connection.
async fn echo_upstream(mut socket: tokio::net::TcpStream) {
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

fn tcp_proxy_to(upstream: &SocketAddr) -> Proxy {
    Proxy {
        ports: vec!["test".parse().unwrap()],
        kind: ProxyKind::Tcp(TcpProxy {
            client_cert: None,
            connect_timeout: r3v3rs3_api::upstream::DEFAULT_CONNECT_TIMEOUT,
            load_balancing: Default::default(),
            health_check: Default::default(),
            circuit_breaker: Default::default(),
            proxy_protocol: None,
            upstream_servers: vec![UpstreamServer::new(
                format!("/ip4/127.0.0.1/tcp/{}", upstream.port())
                    .parse()
                    .unwrap(),
            )],
        }),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_sigusr2_hands_the_listeners_to_a_new_process() -> anyhow::Result<()> {
    let upstream_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    let upstream_addr = upstream_port.socket_addr();
    tokio::spawn(async move {
        let upstream = tokio::net::TcpListener::bind(upstream_addr).await.unwrap();
        loop {
            if let Ok((socket, _)) = upstream.accept().await {
                tokio::spawn(echo_upstream(socket));
            }
        }
    });

    let dir = std::env::temp_dir().join(format!("r3v3rs3-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let storage = SqliteStorage::open(&dir).await?;
    storage.save_app_config(&AppConfig::default()).await?;
    storage
        .save_ports(&[port_entry("test", proxy_port.multiaddr_tcp())])
        .await?;
    storage
        .save_proxies(&[ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            proxy: tcp_proxy_to(&upstream_port.socket_addr()),
        }])
        .await?;

    let exe = env!("CARGO_BIN_EXE_r3v3rs3");
    let mut first = std::process::Command::new(exe)
        .args([
            "start",
            "--no-webui",
            "--config-dir",
            &dir.display().to_string(),
            "--log-dir",
            &dir.display().to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .spawn()?;

    let proxy_addr = proxy_port.socket_addr();
    let mut open = wait_for_proxy(&proxy_addr).await?;
    open.write_all(b"before").await?;
    let mut buf = [0u8; 6];
    open.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"before");

    // The upgrade: the old process hands the listeners over and drains.
    unsafe {
        libc::kill(first.id() as i32, libc::SIGUSR2);
    }

    // The port keeps serving while the old process drains.
    let mut client = wait_for_proxy(&proxy_addr).await?;
    client.write_all(b"second").await?;
    client.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"second");

    // The connection that was open before the upgrade keeps streaming until its end.
    open.write_all(b"after").await?;
    open.read_exact(&mut buf[..5]).await?;
    assert_eq!(&buf[..5], b"after");

    // Closing the old connections lets the old process finish its drain.
    drop(open);
    drop(client);
    let status = tokio::task::spawn_blocking(move || first.wait()).await??;
    assert!(status.success(), "{status}");

    // Stop the new process, which the old one left running on its own.
    let _ = std::process::Command::new("pkill")
        .args(["-f", &format!("^{} start", exe)])
        .status();
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

/// Connects to the proxy until a process listens.
async fn wait_for_proxy(addr: &SocketAddr) -> anyhow::Result<tokio::net::TcpStream> {
    for _ in 0..100 {
        if let Ok(client) = tokio::net::TcpStream::connect(addr).await {
            return Ok(client);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("nothing listens on {addr}")
}
