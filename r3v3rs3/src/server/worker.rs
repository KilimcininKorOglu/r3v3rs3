//! The worker subcommand: serves the listeners that a master process hands over.
//!
//! The master opens the IPC socket, sends the [`Handshake`], the descriptors of the listeners
//! and the [`WorkerConfig`], and then keeps the channel open for commands. The worker reads
//! nothing from a storage: its storage is a memory storage that the master replaces through
//! [`WorkerCommand::Config`].

use super::fdmanifest::ListenerFd;
use super::fdpass::recv_fds;
use super::ipc::{
    Handshake, PROTOCOL_VERSION, WorkerCommand, WorkerConfig, WorkerEvent, WorkerStatus,
    read_frame, write_frame,
};
use crate::config::memory::MemoryStorage;
use crate::server::Server;
use anyhow::{Context, anyhow, bail};
use r3v3rs3_api::app::AppInfo;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

/// A worker exits when no frame arrives from the master in this time.
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(45);

/// Runs the worker until the master hangs up or orders it down.
pub async fn run(ipc_fd: i32, config_dir: &Path, log_dir: &Path) -> anyhow::Result<()> {
    let app_info = app_info(config_dir, log_dir);
    let mut ipc = UnixStream::from(unsafe { OwnedFd::from_raw_fd(ipc_fd) });
    // The frames of the handshake and of the commands are read on blocking threads.
    ipc.set_nonblocking(false)
        .context("failed to configure the IPC socket")?;

    let handshake: Handshake = read_frame(&mut ipc)?;
    if handshake.protocol != PROTOCOL_VERSION {
        bail!(
            "the master speaks protocol {} but this worker speaks {PROTOCOL_VERSION}",
            handshake.protocol
        );
    }
    let config: WorkerConfig = read_frame(&mut ipc)?;
    let fds = recv_fds(&ipc, handshake.listeners.len())?;
    if fds.len() != handshake.listeners.len() {
        bail!(
            "the master sent {} descriptors for {} listeners",
            fds.len(),
            handshake.listeners.len()
        );
    }
    let listeners = handshake
        .listeners
        .iter()
        .zip(fds)
        .map(|(meta, fd)| ListenerFd::new(meta.proto, meta.addr, fd))
        .collect::<Vec<_>>();
    info!(
        listeners = listeners.len(),
        ports = config.ports.len(),
        "worker received its configuration"
    );

    let certs = certs_of(&config.certs)?;
    let storage = Arc::new(MemoryStorage::new(
        config.app.clone(),
        config.ports.clone(),
        config.proxies.clone(),
        config.access_lists.clone(),
        certs.clone(),
    ));
    let (server, channels) = Server::new_worker(app_info, storage.clone(), listeners).await;
    let started_at = std::time::Instant::now();
    // The master publishes the status of this worker to its own event stream, so the admin
    // API and the WebUI see the runtime state that the worker serves.
    let _ = channels
        .command
        .send(crate::command::ServerCommand::SetBroadcastEvents { enabled: true })
        .await;
    let server_task = tokio::spawn(server.start());

    let result = serve_master(&mut ipc, &storage, &channels, &config, started_at).await;
    channels.shutdown();
    let _ = server_task.await;
    result
}

/// Answers the commands of the master until it hangs up.
async fn serve_master(
    ipc: &mut UnixStream,
    storage: &Arc<MemoryStorage>,
    channels: &crate::server::ServerChannels,
    config: &WorkerConfig,
    started_at: std::time::Instant,
) -> anyhow::Result<()> {
    // The master reads this side of the channel since the handover, so this announcement
    // reaches it even though nobody listened when the worker loaded its state.
    let ready = WorkerEvent::Event(r3v3rs3_api::event::ServerEvent::AppConfigUpdated {
        config: Box::new(config.app.clone().masked()),
    });
    write_frame(ipc, &ready).context("failed to announce the readiness of the worker")?;
    loop {
        let command = tokio::time::timeout(HEARTBEAT_TIMEOUT, read_command(ipc)).await;
        let Ok(Some(command)) = command else {
            error!("the master stopped answering, the worker exits");
            return Ok(());
        };
        match command {
            WorkerCommand::Config(new_config) => {
                storage
                    .replace(
                        new_config.ports.clone(),
                        new_config.proxies.clone(),
                        new_config.access_lists.clone(),
                        certs_of(&new_config.certs)?,
                    )
                    .await;
                channels
                    .command
                    .send(crate::command::ServerCommand::WorkerApply)
                    .await
                    .context("the server of the worker is gone")?;
                info!("applied the new configuration of the master");
            }
            WorkerCommand::Drain | WorkerCommand::Shutdown => {
                info!("the master ordered the worker down");
                return Ok(());
            }
            WorkerCommand::Ping => {
                write_frame(ipc, &WorkerEvent::Pong).context("failed to answer the heartbeat")?;
            }
            WorkerCommand::Status => {
                let status = WorkerStatus {
                    pid: std::process::id(),
                    uptime_secs: started_at.elapsed().as_secs(),
                    open_connections: crate::proxy::open_connections() as u64,
                };
                write_frame(ipc, &WorkerEvent::Status(Box::new(status)))
                    .context("failed to answer the status request")?;
            }
        }
    }
}

/// Reads one command on a blocking thread, so the runtime stays free. `None` ends the channel.
async fn read_command(ipc: &UnixStream) -> Option<WorkerCommand> {
    let mut stream = match ipc.try_clone() {
        Ok(stream) => stream,
        Err(err) => {
            error!(%err, "failed to clone the IPC socket");
            return None;
        }
    };
    match tokio::task::spawn_blocking(move || {
        loop {
            match read_frame::<WorkerCommand>(&mut stream) {
                Ok(command) => return Some(command),
                // A descriptor that a service manager handed over can stay in the nonblocking
                // state; a frame that is not there yet is not the end of the channel.
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return None,
            }
        }
    })
    .await
    {
        Ok(Some(command)) => Some(command),
        Ok(None) => {
            info!("the IPC channel ended");
            None
        }
        Err(err) => {
            error!(%err, "the IPC task failed");
            None
        }
    }
}

/// Rebuilds the parsed certificates of the configuration.
fn certs_of(certs: &[super::ipc::WorkerCert]) -> anyhow::Result<Vec<Arc<crate::certs::Cert>>> {
    certs
        .iter()
        .cloned()
        .map(|cert| cert.into_cert().map(Arc::new).map_err(|err| anyhow!(err)))
        .collect()
}

fn app_info(config_dir: &Path, log_dir: &Path) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        target: "worker",
        profile: "worker",
        features: &[],
        rustc: "",
        config_path: config_dir.to_path_buf(),
        log_path: log_dir.to_path_buf(),
        platform_enabled: false,
    }
}

/// The directories of a worker. A worker writes no configuration, so only the log directory
/// holds anything, and only the audit log of its own actions.
pub fn ensure_dirs(config_dir: &PathBuf, log_dir: &PathBuf) -> anyhow::Result<()> {
    for dir in [config_dir, log_dir] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
    }
    Ok(())
}
