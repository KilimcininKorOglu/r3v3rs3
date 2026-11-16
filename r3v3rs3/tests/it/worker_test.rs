//! The worker process against a master that this test plays: the test opens a socket pair,
//! hands the descriptors of a listener over and drives the commands.

use crate::common;
use common::{alloc_tcp_port, port_entry};
use r3v3rs3::fdmanifest::ListenerProto;
use r3v3rs3::ipc::{
    Handshake, ListenerMeta, PROTOCOL_VERSION, WorkerCommand, WorkerConfig, WorkerEvent,
    read_frame, write_frame,
};
use r3v3rs3::server::fdpass::send_fds;
use r3v3rs3_api::access_list::AccessListEntry;
use r3v3rs3_api::app::AppConfig;
use r3v3rs3_api::port::UpstreamServer;
use r3v3rs3_api::proxy::{Proxy, ProxyEntry, ProxyKind, TcpProxy};
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::time::Duration;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

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

/// Checks that the heartbeat of the master gets its answer.
fn expect_pong(master: &mut std::os::unix::net::UnixStream) -> anyhow::Result<()> {
    write_frame(master, &WorkerCommand::Ping)?;
    // The worker announces its configuration over the same channel, so the events
    // that arrive before the answer do not block it.
    loop {
        let frame: WorkerEvent = read_frame(master)?;
        if matches!(frame, WorkerEvent::Pong) {
            return Ok(());
        }
    }
}

/// Checks that the heartbeat and the status request of the master get their answers. The
/// status names the worker process `pid` and counts the one client connection that is open.
fn check_ping_and_status(
    master: &mut std::os::unix::net::UnixStream,
    pid: u32,
) -> anyhow::Result<()> {
    expect_pong(master)?;

    // The status request reports the live state of the worker process.
    write_frame(master, &WorkerCommand::Status)?;
    let WorkerEvent::Status(status) = read_frame(master)? else {
        anyhow::bail!("the worker did not answer the status request");
    };
    assert_eq!(status.pid, pid);
    assert_eq!(status.open_connections, 1);
    Ok(())
}

/// Starts an upstream that answers every connection with `marker`, so a reply shows which
/// upstream served it.
fn spawn_marker_upstream(addr: SocketAddr, marker: &'static [u8]) {
    tokio::spawn(async move {
        let upstream = tokio::net::TcpListener::bind(addr).await.unwrap();
        while let Ok((mut socket, _)) = upstream.accept().await {
            tokio::spawn(async move { socket.write_all(marker).await });
        }
    });
}

/// Checks that a new configuration moves the proxy of the worker to a new upstream.
async fn check_new_config(
    master: &mut std::os::unix::net::UnixStream,
    config: &WorkerConfig,
    proxy_addr: SocketAddr,
) -> anyhow::Result<()> {
    // A new configuration replaces the state of the worker.
    let second_port = alloc_tcp_port().await?;
    spawn_marker_upstream(second_port.socket_addr(), b"second");
    let mut new_config = config.clone();
    new_config.proxies[0].proxy = tcp_proxy_to(&second_port.socket_addr());
    write_frame(master, &WorkerCommand::Config(Box::new(new_config)))?;
    // The worker answers the commands in order, so the pong follows the applied configuration.
    expect_pong(master)?;

    // The first upstream echoes the payload, so only the new upstream answers "second" to it.
    let mut client = wait_for_proxy(&proxy_addr).await?;
    let mut buf = [0u8; 6];
    client.write_all(b"xxxxxx").await?;
    client.read_exact(&mut buf).await?;
    assert_eq!(&buf, b"second");
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
    // The worker holds its own copy now. Without this close a worker that exits leaves the
    // channel open, and the test waits for an answer forever instead of failing.
    unsafe { libc::close(child_fd) };

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
    pid: u32,
) -> anyhow::Result<()> {
    check_ping_and_status(master, pid)?;
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

    check_commands(&mut master, &config, proxy_addr, child.id()).await?;
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

/// A one minute quarantine of the address.
fn quarantine_entry(ip: std::net::IpAddr) -> r3v3rs3_api::quarantine::QuarantinedIpEntry {
    r3v3rs3_api::quarantine::QuarantinedIpEntry {
        ip,
        reason: "manual".into(),
        violations: 1,
        banned_at: 0,
        banned_until: 0,
        remaining_seconds: 60,
    }
}

/// A master server without ports whose quarantine changes go to the returned channel, as they
/// go to its worker in the workers mode.
async fn master_with_link() -> anyhow::Result<(
    r3v3rs3::server::ServerChannels,
    tokio::sync::mpsc::Receiver<r3v3rs3::ipc::QuarantineChange>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
)> {
    let storage = common::TestStorage::builder().build();
    let log_dir = std::env::temp_dir().join(format!("r3v3rs3-wquarantine-{}", std::process::id()));
    std::fs::create_dir_all(&log_dir)?;
    let app_info = r3v3rs3::config::new_appinfo(&log_dir, &log_dir);
    let (server, channels) =
        r3v3rs3::server::Server::new_shared(app_info, std::sync::Arc::new(storage)).await;
    let server_task = tokio::spawn(server.start());
    let (sender, link) = tokio::sync::mpsc::channel(4);
    channels
        .command
        .send(r3v3rs3::command::ServerCommand::SetWorkerLink { sender })
        .await?;
    Ok((channels, link, server_task))
}

/// In the workers mode the worker serves the ports, so a quarantine change of the master goes
/// to the worker.
#[tokio::test]
async fn the_quarantine_changes_of_the_master_go_to_the_worker() -> anyhow::Result<()> {
    use r3v3rs3::ipc::QuarantineChange;
    use r3v3rs3::server::rpc::quarantine::{QuarantineIp, UnquarantineIp};

    let (mut channels, mut link, server_task) = master_with_link().await?;
    let ip: std::net::IpAddr = "203.0.113.80".parse()?;
    let quarantine = QuarantineIp {
        ip,
        reason: "manual".into(),
        duration_seconds: 60,
    };
    common::call(&mut channels, quarantine).await??;
    assert!(matches!(link.recv().await, Some(QuarantineChange::Add(entry)) if entry.ip == ip));
    common::call(&mut channels, UnquarantineIp { ip }).await??;
    assert_eq!(link.recv().await, Some(QuarantineChange::Remove(ip)));
    channels.shutdown();
    server_task.await?
}

/// A quarantine of the worker shows in the list of the master.
#[tokio::test]
async fn a_quarantine_of_the_worker_shows_in_the_master() -> anyhow::Result<()> {
    use r3v3rs3::command::ServerCommand;
    use r3v3rs3::ipc::QuarantineChange;
    use r3v3rs3::server::rpc::quarantine::GetQuarantinedList;

    let (mut channels, _link, server_task) = master_with_link().await?;
    let worker_ban = quarantine_entry("203.0.113.81".parse()?);
    let change = QuarantineChange::Add(Box::new(worker_ban.clone()));
    channels
        .command
        .send(ServerCommand::ApplyQuarantine {
            change,
            notify: true,
        })
        .await?;
    let list = common::call(&mut channels, GetQuarantinedList).await??;
    let ips = list.iter().map(|entry| entry.ip).collect::<Vec<_>>();
    assert_eq!(ips, [worker_ban.ip]);
    channels.shutdown();
    server_task.await?
}

/// Starts a worker with an HTTP proxy that trusts `X-Forwarded-For` from 127.0.0.1, so a test
/// on loopback can act as any client address. Returns the master end of its channel, the
/// worker process and the URL of the proxy.
async fn start_http_worker(
    log_dir: &std::path::Path,
) -> anyhow::Result<(
    std::os::unix::net::UnixStream,
    std::process::Child,
    url::Url,
)> {
    let upstream =
        common::serve_http_upstream(axum::Router::new().fallback(|| async { "ok" })).await?;
    let proxy_port = alloc_tcp_port().await?;
    let (listener, proxy_addr) = bind_proxy_listener(&proxy_port)?;
    std::fs::create_dir_all(log_dir)?;
    let proxy = r3v3rs3_api::proxy::HttpProxy {
        routes: vec![common::http_route("/", upstream.as_str(), None)],
        client_ip: r3v3rs3_api::client_ip::ClientIpConfig {
            trust_cdn: false,
            trusted_proxies: vec!["127.0.0.0/8".parse()?],
        },
        upgrade_insecure: false,
        ..Default::default()
    };
    let config = WorkerConfig {
        ports: vec![common::http_port_entry("test", &proxy_port)],
        proxies: vec![common::http_proxy_entry("test2", "test", proxy)],
        ..worker_config(&proxy_port, &proxy_addr)
    };
    let (master, child) = start_worker(listener, proxy_addr, &config, log_dir)?;
    // The worker listens on the IPv4 loopback only, so the URL names that address and not
    // `localhost`, which can resolve to `::1` first.
    Ok((master, child, format!("http://{proxy_addr}/").parse()?))
}

/// Whether a request as the client `ip` gets an answer through the worker.
async fn served(url: &url::Url, ip: &str) -> bool {
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let request = reqwest::Client::new()
        .get(url.clone())
        .header("x-forwarded-for", ip)
        .send()
        .await;
    request.is_ok_and(|resp| resp.status().is_success())
}

/// The worker applies the quarantine commands of the master: a quarantined client is refused,
/// another client is still served, and the client is served again after the unquarantine.
#[tokio::test]
async fn a_worker_takes_the_quarantine_commands_of_the_master() -> anyhow::Result<()> {
    let log_dir = std::env::temp_dir().join(format!("r3v3rs3-wqcmd-{}", std::process::id()));
    let (mut master, child, url) = start_http_worker(&log_dir).await?;
    wait_for_proxy(&url.socket_addrs(|| None)?[0]).await?;
    check_quarantine_refuses(&mut master, &url).await?;
    check_unquarantine_serves(&mut master, &url).await?;
    stop_worker(master, child).await?;
    std::fs::remove_dir_all(&log_dir).ok();
    Ok(())
}

/// After a quarantine command the worker refuses the client and still serves another one.
async fn check_quarantine_refuses(
    master: &mut std::os::unix::net::UnixStream,
    url: &url::Url,
) -> anyhow::Result<()> {
    let entry = quarantine_entry("203.0.113.82".parse()?);
    write_frame(master, &WorkerCommand::Quarantine(Box::new(entry)))?;
    // The worker answers the commands in order, so the pong follows the applied quarantine.
    expect_pong(master)?;
    assert!(
        !served(url, "203.0.113.82").await,
        "a quarantined client was served"
    );
    assert!(
        served(url, "198.51.100.83").await,
        "another client was refused"
    );
    Ok(())
}

/// After an unquarantine command the worker serves the client again.
async fn check_unquarantine_serves(
    master: &mut std::os::unix::net::UnixStream,
    url: &url::Url,
) -> anyhow::Result<()> {
    write_frame(
        master,
        &WorkerCommand::Unquarantine("203.0.113.82".parse()?),
    )?;
    expect_pong(master)?;
    assert!(
        served(url, "203.0.113.82").await,
        "the unquarantined client was refused"
    );
    Ok(())
}

/// The admin status holds what the supervisor reports: the live state of a worker and, after
/// a missed heartbeat answer, the same worker as unreachable.
#[tokio::test]
async fn the_admin_status_reports_the_workers_of_the_master() -> anyhow::Result<()> {
    use r3v3rs3::command::ServerCommand;
    use r3v3rs3::ipc::WorkerStatus;
    use r3v3rs3::server::Server;
    use r3v3rs3::server::rpc::workers::GetWorkers;
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
