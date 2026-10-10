//! The worker process against a master that this test plays: the test opens a socket pair,
//! hands the descriptors of a listener over and drives the commands.

use r3v3rs3::server::fdmanifest::ListenerProto;
use r3v3rs3::server::fdpass::send_fds;
use r3v3rs3::server::ipc::{
    Handshake, ListenerMeta, PROTOCOL_VERSION, WorkerCommand, WorkerConfig, WorkerEvent,
    read_frame, write_frame,
};
use r3v3rs3_api::access_list::AccessListEntry;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::port::UpstreamServer;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyKind, TcpProxy};
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
mod common;
use common::{alloc_tcp_port, port_entry};

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
async fn a_worker_serves_the_listeners_of_the_master() -> anyhow::Result<()> {
    let upstream_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    let upstream_addr = upstream_port.socket_addr();
    let listener = std::net::TcpListener::bind(proxy_port.socket_addr())?;
    let proxy_addr = listener.local_addr()?;
    tokio::spawn(async move {
        let upstream = tokio::net::TcpListener::bind(upstream_addr).await.unwrap();
        loop {
            if let Ok((socket, _)) = upstream.accept().await {
                tokio::spawn(echo_upstream(socket));
            }
        }
    });

    let (mut master, worker_end) = std::os::unix::net::UnixStream::pair()?;
    let log_dir = std::env::temp_dir().join(format!("r3v3rs3-worker-{}", std::process::id()));
    std::fs::create_dir_all(&log_dir)?;

    let config = WorkerConfig {
        ports: vec![port_entry("test", proxy_port.multiaddr_tcp())],
        proxies: vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            proxy: tcp_proxy_to(&upstream_port.socket_addr()),
        }],
        access_lists: Vec::<AccessListEntry>::new(),
        certs: Vec::new(),
        app: AppConfig::default(),
    };

    let child_fd = unsafe { libc::dup(worker_end.as_raw_fd()) };
    assert!(child_fd >= 0);
    let exe = env!("CARGO_BIN_EXE_r3v3rs3");
    let mut command = std::process::Command::new(exe);
    command
        .args([
            "worker",
            "--ipc",
            "3",
            "--config-dir",
            &log_dir.join("config").display().to_string(),
            "--log-dir",
            &log_dir.display().to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::inherit());
    let raw_worker_end = worker_end.as_raw_fd();
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(move || {
            if libc::dup2(raw_worker_end, 3) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn()?;

    write_frame(
        &mut master,
        &Handshake {
            protocol: PROTOCOL_VERSION,
            listeners: vec![ListenerMeta {
                proto: ListenerProto::Tcp,
                addr: proxy_addr,
            }],
        },
    )?;
    write_frame(&mut master, &config)?;
    send_fds(&master, &[child_fd])?;

    // The kernel keeps the listener alive through the descriptor of the worker only, so every
    // connection reaches the worker.
    drop(listener);
    std::mem::drop(worker_end);

    let mut client = wait_for_proxy(&proxy_addr).await?;
    client.write_all(b"first").await?;
    let mut buf = [0u8; 6];
    client.read_exact(&mut buf[..5]).await?;
    assert_eq!(&buf[..5], b"first");

    // The heartbeat of the master gets an answer.
    write_frame(&mut master, &WorkerCommand::Ping)?;
    let event: WorkerEvent = read_frame(&mut master)?;
    assert!(matches!(event, WorkerEvent::Pong));

    // A new configuration replaces the state of the worker.
    let second_port = alloc_tcp_port().await?;
    let second_addr = second_port.socket_addr();
    tokio::spawn(async move {
        let upstream = tokio::net::TcpListener::bind(second_addr).await.unwrap();
        loop {
            if let Ok((socket, _)) = upstream.accept().await {
                tokio::spawn(echo_upstream(socket));
            }
        }
    });
    let mut new_config = config.clone();
    new_config.proxies[0].proxy = tcp_proxy_to(&second_port.socket_addr());
    write_frame(&mut master, &WorkerCommand::Config(Box::new(new_config)))?;

    let mut client = wait_for_proxy(&proxy_addr).await?;
    client.write_all(b"second").await?;
    client.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"second");
    drop(client);

    write_frame(&mut master, &WorkerCommand::Shutdown)?;
    drop(master);
    let status = tokio::task::spawn_blocking(move || child.wait()).await??;
    assert!(status.success(), "{status}");

    std::fs::remove_dir_all(&log_dir).ok();
    Ok(())
}

/// Connects to the proxy until the worker listens.
async fn wait_for_proxy(addr: &SocketAddr) -> anyhow::Result<tokio::net::TcpStream> {
    for _ in 0..50 {
        if let Ok(client) = tokio::net::TcpStream::connect(addr).await {
            return Ok(client);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!("the worker does not listen on {addr}")
}

/// The supervisor hands the listeners of a real master to a worker, and the master stops
/// accepting while the worker serves the port.
#[tokio::test]
async fn the_supervisor_serves_the_ports_through_a_worker() -> anyhow::Result<()> {
    use r3v3rs3::server::Server;
    use r3v3rs3_api::app::{AppConfig, WorkerMode};

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

    let log_dir = std::env::temp_dir().join(format!("r3v3rs3-supervisor-{}", std::process::id()));
    std::fs::create_dir_all(&log_dir)?;
    let storage = common::TestStorage::builder()
        .ports(vec![port_entry("test", proxy_port.multiaddr_tcp())])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            proxy: tcp_proxy_to(&upstream_port.socket_addr()),
        }])
        .build();
    let app_info = r3v3rs3::config::new_appinfo(&log_dir, &log_dir);
    let (server, channels) =
        Server::new_shared(app_info.clone(), std::sync::Arc::new(storage)).await;
    let manifest = server.listener_manifest()?;
    assert_eq!(manifest.len(), 1);
    let server_task = tokio::spawn(server.start());

    let app = AppConfig {
        worker_mode: WorkerMode::Workers,
        ..Default::default()
    };
    let supervisor = r3v3rs3::server::supervisor::run(
        channels.event.subscribe(),
        channels.command.clone(),
        app_info,
        app,
        manifest,
    );
    let supervisor_task = tokio::spawn(supervisor);

    let proxy_addr = proxy_port.socket_addr();
    let mut client = wait_for_proxy(&proxy_addr).await?;
    client.write_all(b"hello").await?;
    let mut buf = [0u8; 5];
    client.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"hello");

    channels.shutdown();
    supervisor_task.abort();
    server_task.await??;
    std::fs::remove_dir_all(&log_dir).ok();
    Ok(())
}
