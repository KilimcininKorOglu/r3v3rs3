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

/// Starts the echo upstream on the given address.
fn spawn_echo_upstream(upstream_addr: SocketAddr) {
    tokio::spawn(async move {
        let upstream = tokio::net::TcpListener::bind(upstream_addr).await.unwrap();
        loop {
            if let Ok((socket, _)) = upstream.accept().await {
                tokio::spawn(echo_upstream(socket));
            }
        }
    });
}

/// Writes the config of a TCP proxy from the proxy address to the upstream into the directory.
async fn write_config(
    dir: &std::path::Path,
    proxy_port: &common::TestPort,
    upstream_addr: &SocketAddr,
) -> anyhow::Result<()> {
    let storage = SqliteStorage::open(dir).await?;
    storage.save_app_config(&AppConfig::default()).await?;
    storage
        .save_ports(&[port_entry("test", proxy_port.multiaddr_tcp())])
        .await?;
    storage
        .save_proxies(&[ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            status: None,
            proxy: tcp_proxy_to(upstream_addr),
        }])
        .await?;
    Ok(())
}

/// Writes the payload and checks that the upstream echoes it back.
async fn expect_echo(stream: &mut tokio::net::TcpStream, payload: &[u8]) -> anyhow::Result<()> {
    let mut buf = [0u8; 6];
    stream.write_all(payload).await?;
    stream.read_exact(&mut buf[..payload.len()]).await?;
    assert_eq!(&buf[..payload.len()], payload);
    Ok(())
}

#[tokio::test]
async fn a_sigusr2_hands_the_listeners_to_a_new_process() -> anyhow::Result<()> {
    let upstream_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    spawn_echo_upstream(upstream_port.socket_addr());

    let dir = std::env::temp_dir().join(format!("r3v3rs3-upgrade-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    write_config(&dir, &proxy_port, &upstream_port.socket_addr()).await?;

    let exe = env!("CARGO_BIN_EXE_r3-server");
    let first = start_server(exe, &dir)?;
    check_handover(first, proxy_port.socket_addr()).await?;

    // Stop the new process, which the old one left running on its own.
    let _ = std::process::Command::new("pkill")
        .args(["-f", &format!("^{} start", exe)])
        .status();
    std::fs::remove_dir_all(&dir).ok();
    Ok(())
}

/// Starts the server binary without the WebUI on the config directory.
fn start_server(exe: &str, dir: &std::path::Path) -> anyhow::Result<std::process::Child> {
    Ok(std::process::Command::new(exe)
        .args([
            "start",
            "--no-webui",
            "--config-dir",
            &dir.display().to_string(),
            "--log-dir",
            &dir.display().to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .spawn()?)
}

/// Checks that after a SIGUSR2 a new process serves the port, the open connection keeps
/// streaming, and the old process exits successfully after its drain.
async fn check_handover(
    mut first: std::process::Child,
    proxy_addr: SocketAddr,
) -> anyhow::Result<()> {
    let mut open = wait_for_proxy(&proxy_addr).await?;
    expect_echo(&mut open, b"before").await?;

    // The upgrade: the old process hands the listeners over and drains.
    unsafe {
        libc::kill(first.id() as i32, libc::SIGUSR2);
    }

    // The port keeps serving while the old process drains.
    let mut client = wait_for_proxy(&proxy_addr).await?;
    expect_echo(&mut client, b"second").await?;

    // The connection that was open before the upgrade keeps streaming until its end.
    expect_echo(&mut open, b"after").await?;

    // Closing the old connections lets the old process finish its drain.
    drop(open);
    drop(client);
    let status = tokio::task::spawn_blocking(move || first.wait()).await??;
    assert!(status.success(), "{status}");
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
