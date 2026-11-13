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

/// Writes the payload and checks that the upstream echoes it back.
async fn expect_echo(stream: &mut tokio::net::TcpStream, payload: &[u8]) -> anyhow::Result<()> {
    let mut buf = [0u8; 6];
    stream.write_all(payload).await?;
    stream.read_exact(&mut buf[..payload.len()]).await?;
    assert_eq!(&buf[..payload.len()], payload);
    Ok(())
}

/// The worker configuration with a TCP proxy from the proxy port to the upstream.
fn worker_config(proxy_port: &common::TestPort, upstream_addr: &SocketAddr) -> WorkerConfig {
    WorkerConfig {
        ports: vec![port_entry("test", proxy_port.multiaddr_tcp())],
        proxies: vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            status: None,
            proxy: tcp_proxy_to(upstream_addr),
        }],
        access_lists: Vec::<AccessListEntry>::new(),
        certs: Vec::new(),
        app: AppConfig::default(),
    }
}

/// Starts the worker binary with its end of the socket pair as descriptor 3.
fn spawn_worker(
    worker_end: &std::os::unix::net::UnixStream,
    log_dir: &std::path::Path,
) -> anyhow::Result<std::process::Child> {
    let exe = env!("CARGO_BIN_EXE_r3-server");
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
            if libc::dup2(raw_worker_end, 3) == -1 || libc::fcntl(3, libc::F_SETFD, 0) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    Ok(command.spawn()?)
}

/// Sends the handshake, the configuration and the listener descriptor to the worker.
fn send_handshake(
    master: &mut std::os::unix::net::UnixStream,
    proxy_addr: SocketAddr,
    config: &WorkerConfig,
    child_fd: i32,
) -> anyhow::Result<()> {
    write_frame(
        master,
        &Handshake {
            protocol: PROTOCOL_VERSION,
            listeners: vec![ListenerMeta {
                proto: ListenerProto::Tcp,
                addr: proxy_addr,
            }],
        },
    )?;
    write_frame(master, config)?;
    send_fds(master, &[child_fd])?;
    Ok(())
}

/// Checks that the heartbeat and the status request of the master get their answers.
fn check_ping_and_status(master: &mut std::os::unix::net::UnixStream) -> anyhow::Result<()> {
    // The heartbeat of the master gets an answer.
    write_frame(master, &WorkerCommand::Ping)?;
    // The worker announces its configuration over the same channel, so the events
    // that arrive before the answer do not block it.
    loop {
        let frame: WorkerEvent = read_frame(master)?;
        if matches!(frame, WorkerEvent::Pong) {
            break;
        }
    }

    // The status request reports the live state of the worker process.
    write_frame(master, &WorkerCommand::Status)?;
    let WorkerEvent::Status(status) = read_frame(master)? else {
        anyhow::bail!("the worker did not answer the status request");
    };
    assert!(status.pid > 0);
    assert!(status.open_connections > 0);
    Ok(())
}

/// Checks that a new configuration moves the proxy of the worker to a new upstream.
async fn check_new_config(
    master: &mut std::os::unix::net::UnixStream,
    config: &WorkerConfig,
    proxy_addr: SocketAddr,
) -> anyhow::Result<()> {
    // A new configuration replaces the state of the worker.
    let second_port = alloc_tcp_port().await?;
    spawn_echo_upstream(second_port.socket_addr());
    let mut new_config = config.clone();
    new_config.proxies[0].proxy = tcp_proxy_to(&second_port.socket_addr());
    write_frame(master, &WorkerCommand::Config(Box::new(new_config)))?;

    let mut client = wait_for_proxy(&proxy_addr).await?;
    expect_echo(&mut client, b"second").await?;
    drop(client);
    Ok(())
}

/// Binds the listener that the test hands to the worker and returns it with its address.
fn bind_proxy_listener(
    proxy_port: &common::TestPort,
) -> anyhow::Result<(std::net::TcpListener, SocketAddr)> {
    let listener = std::net::TcpListener::bind(proxy_port.socket_addr())?;
    let proxy_addr = listener.local_addr()?;
    Ok((listener, proxy_addr))
}

/// Starts a worker, hands it the listener with its configuration, and returns the master end
/// of the socket pair and the worker process.
fn start_worker(
    listener: std::net::TcpListener,
    proxy_addr: SocketAddr,
    config: &WorkerConfig,
    log_dir: &std::path::Path,
) -> anyhow::Result<(std::os::unix::net::UnixStream, std::process::Child)> {
    let (mut master, worker_end) = std::os::unix::net::UnixStream::pair()?;

    let child_fd = unsafe { libc::dup(worker_end.as_raw_fd()) };
    assert!(child_fd >= 0);
    let child = spawn_worker(&worker_end, log_dir)?;

    send_handshake(&mut master, proxy_addr, config, child_fd)?;

    // The kernel keeps the listener alive through the descriptor of the worker only, so every
    // connection reaches the worker.
    drop(listener);
    std::mem::drop(worker_end);
    Ok((master, child))
}

/// Connects through the worker, checks the echo of the payload and returns the open connection.
async fn connect_and_echo(
    proxy_addr: &SocketAddr,
    payload: &[u8],
) -> anyhow::Result<tokio::net::TcpStream> {
    let mut client = wait_for_proxy(proxy_addr).await?;
    expect_echo(&mut client, payload).await?;
    Ok(client)
}

/// Checks the heartbeat, the status request and a configuration change of the worker.
async fn check_commands(
    master: &mut std::os::unix::net::UnixStream,
    config: &WorkerConfig,
    proxy_addr: SocketAddr,
) -> anyhow::Result<()> {
    check_ping_and_status(master)?;
    check_new_config(master, config, proxy_addr).await
}

/// Sends the shutdown command and checks that the worker exits successfully.
async fn stop_worker(
    mut master: std::os::unix::net::UnixStream,
    mut child: std::process::Child,
) -> anyhow::Result<()> {
    write_frame(&mut master, &WorkerCommand::Shutdown)?;
    drop(master);
    let status = tokio::task::spawn_blocking(move || child.wait()).await??;
    assert!(status.success(), "{status}");
    Ok(())
}

#[tokio::test]
async fn a_worker_serves_the_listeners_of_the_master() -> anyhow::Result<()> {
    let upstream_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    let (listener, proxy_addr) = bind_proxy_listener(&proxy_port)?;
    spawn_echo_upstream(upstream_port.socket_addr());

    let log_dir = std::env::temp_dir().join(format!("r3v3rs3-worker-{}", std::process::id()));
    std::fs::create_dir_all(&log_dir)?;

    let config = worker_config(&proxy_port, &upstream_port.socket_addr());
    let (mut master, child) = start_worker(listener, proxy_addr, &config, &log_dir)?;

    // The first connection stays open until the worker stops.
    let client = connect_and_echo(&proxy_addr, b"first").await?;

    check_commands(&mut master, &config, proxy_addr).await?;
    stop_worker(master, child).await?;
    drop(client);

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

/// Reads the event stream for up to 100 periods of 100 ms and reports whether the configuration
/// event of the worker arrived.
async fn saw_worker_event(
    bus: &mut tokio::sync::broadcast::Receiver<r3v3rs3_api::event::ServerEvent>,
) -> bool {
    for _ in 0..100 {
        let event = tokio::time::timeout(Duration::from_millis(100), bus.recv()).await;
        if let Ok(Ok(r3v3rs3_api::event::ServerEvent::AppConfigUpdated { .. })) = event {
            return true;
        }
    }
    false
}

/// The supervisor hands the listeners of a real master to a worker, and the master stops
/// accepting while the worker serves the port.
#[tokio::test]
async fn the_supervisor_serves_the_ports_through_a_worker() -> anyhow::Result<()> {
    use r3v3rs3::server::Server;
    use r3v3rs3_api::app::{AppConfig, WorkerMode};

    let upstream_port = alloc_tcp_port().await?;
    let proxy_port = alloc_tcp_port().await?;
    spawn_echo_upstream(upstream_port.socket_addr());

    let log_dir = std::env::temp_dir().join(format!("r3v3rs3-supervisor-{}", std::process::id()));
    std::fs::create_dir_all(&log_dir)?;
    let storage = common::TestStorage::builder()
        .ports(vec![port_entry("test", proxy_port.multiaddr_tcp())])
        .proxies(vec![ProxyEntry {
            id: "test2".parse().unwrap(),
            source: None,
            status: None,
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
        channels.event.clone(),
        channels.command.clone(),
        std::path::PathBuf::from(env!("CARGO_BIN_EXE_r3-server")),
        app_info,
        app,
        manifest,
    );
    let supervisor_task = tokio::spawn(supervisor);

    // The runtime state of the worker flows into the event stream of the master: the worker
    // announces its configuration over the IPC channel once it serves.
    let mut bus = channels.event.subscribe();
    assert!(
        saw_worker_event(&mut bus).await,
        "the event of the worker did not reach the master"
    );

    let proxy_addr = proxy_port.socket_addr();
    let mut client = wait_for_proxy(&proxy_addr).await?;
    expect_echo(&mut client, b"hello").await?;

    channels.shutdown();
    if let Err(err) = supervisor_task.await {
        panic!("the supervisor task failed: {err}");
    }
    server_task.await??;
    std::fs::remove_dir_all(&log_dir).ok();
    Ok(())
}

/// The admin status holds what the supervisor reports: the live state of a worker and, after
/// a missed heartbeat answer, the same worker as unreachable.
#[tokio::test]
async fn the_admin_status_reports_the_workers_of_the_master() -> anyhow::Result<()> {
    use r3v3rs3::command::ServerCommand;
    use r3v3rs3::server::rpc::workers::GetWorkers;
    use r3v3rs3::server::{Server, ipc::WorkerStatus};
    use r3v3rs3_api::app::WorkerMode;

    let log_dir = std::env::temp_dir().join(format!("r3v3rs3-wstatus-{}", std::process::id()));
    std::fs::create_dir_all(&log_dir)?;
    let storage = common::TestStorage::builder().build();
    let app_info = r3v3rs3::config::new_appinfo(&log_dir, &log_dir);
    let (server, mut channels) = Server::new_shared(app_info, std::sync::Arc::new(storage)).await;
    let server_task = tokio::spawn(server.start());

    let pid = 4242;
    channels
        .command
        .send(ServerCommand::SetWorkerStatus {
            pid,
            status: Some(WorkerStatus {
                pid,
                uptime_secs: 5,
                open_connections: 3,
            }),
        })
        .await?;
    let report = common::call(&mut channels, GetWorkers).await??;
    assert_eq!(report.mode, WorkerMode::Off);
    assert_eq!(report.workers.len(), 1);
    assert_eq!(report.workers[0].id, "4242");
    assert_eq!(
        report.workers[0].status.as_ref().unwrap().open_connections,
        3
    );

    // A heartbeat with no answer keeps the worker listed with no status.
    channels
        .command
        .send(ServerCommand::SetWorkerStatus { pid, status: None })
        .await?;
    let report = common::call(&mut channels, GetWorkers).await??;
    assert_eq!(report.workers.len(), 1);
    assert!(report.workers[0].status.is_none());

    channels.shutdown();
    server_task.await??;
    std::fs::remove_dir_all(&log_dir).ok();
    Ok(())
}
