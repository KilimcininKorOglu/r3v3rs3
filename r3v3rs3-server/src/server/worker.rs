//! The worker subcommand: serves the listeners that a master process hands over.
//!
//! The master opens the IPC socket, sends the [`Handshake`], the descriptors of the listeners
//! and the [`WorkerConfig`], and then keeps the channel open for commands. The worker reads
//! nothing from a storage: its storage is a memory storage that the master replaces through
//! [`WorkerCommand::Config`].

use super::fdpass::recv_fds;
use crate::command::ServerCommand;
use crate::config::memory::MemoryStorage;
use crate::fdmanifest::ListenerFd;
use crate::ipc::{
    Handshake, PROTOCOL_VERSION, QuarantineChange, WorkerCommand, WorkerConfig, WorkerEvent,
    WorkerStatus, read_frame, write_frame,
};
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
    let app_info = app_info(config_dir, log_dir)?;
    let mut ipc = UnixStream::from(unsafe { OwnedFd::from_raw_fd(ipc_fd) });
    // The frames of the handshake and of the commands are read on blocking threads.
    ipc.set_nonblocking(false)
        .context("failed to configure the IPC socket")?;

    let (config, listeners) = receive_handover(&mut ipc)?;
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
    let writer = Writer::new(&ipc)?;
    report_quarantines(&channels, writer.clone()).await?;

    let result = serve_master(&ipc, &writer, &storage, &channels, &config, started_at).await;
    channels.shutdown();
    let _ = server_task.await;
    result
}

/// Reads the handshake, the configuration and the listening sockets that the master hands over.
fn receive_handover(ipc: &mut UnixStream) -> anyhow::Result<(WorkerConfig, Vec<ListenerFd>)> {
    let handshake: Handshake = read_frame(ipc)?;
    if handshake.protocol != PROTOCOL_VERSION {
        bail!(
            "the master speaks protocol {} but this worker speaks {PROTOCOL_VERSION}",
            handshake.protocol
        );
    }
    let config: WorkerConfig = read_frame(ipc)?;
    let fds = recv_fds(ipc, handshake.listeners.len())?;
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
        .collect();
    Ok((config, listeners))
}

/// The writing half of the IPC socket. The command loop and the quarantine reports share it, so
/// a lock keeps their frames whole.
#[derive(Clone)]
struct Writer(Arc<std::sync::Mutex<UnixStream>>);

impl Writer {
    fn new(ipc: &UnixStream) -> anyhow::Result<Self> {
        let stream = ipc.try_clone().context("failed to clone the IPC socket")?;
        Ok(Self(Arc::new(std::sync::Mutex::new(stream))))
    }

    fn send(&self, event: &WorkerEvent) -> std::io::Result<()> {
        let mut stream = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        write_frame(&mut *stream, event)
    }
}

/// Sends every quarantine of this worker to the master, which lists it and sends its webhook.
async fn report_quarantines(
    channels: &crate::server::ServerChannels,
    writer: Writer,
) -> anyhow::Result<()> {
    let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    channels
        .command
        .send(ServerCommand::SetQuarantineNotifier { sender })
        .await
        .context("the server of the worker is gone")?;
    tokio::spawn(async move {
        while let Some(entry) = receiver.recv().await {
            if let Err(err) = writer.send(&WorkerEvent::Quarantined(Box::new(entry))) {
                error!(%err, "failed to report a quarantine to the master");
            }
        }
    });
    Ok(())
}

/// Answers the commands of the master until it hangs up.
async fn serve_master(
    ipc: &UnixStream,
    writer: &Writer,
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
    writer
        .send(&ready)
        .context("failed to announce the readiness of the worker")?;
    loop {
        let command = tokio::time::timeout(HEARTBEAT_TIMEOUT, read_command(ipc)).await;
        let Ok(Some(command)) = command else {
            error!("the master stopped answering, the worker exits");
            return Ok(());
        };
        if !handle_command(command, writer, storage, channels, started_at).await? {
            return Ok(());
        }
    }
}

/// Runs one command of the master. `false` ends the worker.
async fn handle_command(
    command: WorkerCommand,
    writer: &Writer,
    storage: &Arc<MemoryStorage>,
    channels: &crate::server::ServerChannels,
    started_at: std::time::Instant,
) -> anyhow::Result<bool> {
    match command {
        WorkerCommand::Config(new_config) => apply_config(*new_config, storage, channels).await?,
        WorkerCommand::Drain | WorkerCommand::Shutdown => {
            info!("the master ordered the worker down");
            return Ok(false);
        }
        WorkerCommand::Ping => writer
            .send(&WorkerEvent::Pong)
            .context("failed to answer the heartbeat")?,
        WorkerCommand::Status => writer
            .send(&WorkerEvent::Status(Box::new(status(started_at))))
            .context("failed to answer the status request")?,
        WorkerCommand::Quarantine(entry) => {
            apply_quarantine(channels, QuarantineChange::Add(entry)).await?;
        }
        WorkerCommand::Unquarantine(ip) => {
            apply_quarantine(channels, QuarantineChange::Remove(ip)).await?;
        }
    }
    Ok(true)
}

fn status(started_at: std::time::Instant) -> WorkerStatus {
    WorkerStatus {
        pid: std::process::id(),
        uptime_secs: started_at.elapsed().as_secs(),
        open_connections: crate::stats::open_connections() as u64,
    }
}

/// Applies a quarantine change of the master without reporting it back.
async fn apply_quarantine(
    channels: &crate::server::ServerChannels,
    change: QuarantineChange,
) -> anyhow::Result<()> {
    channels
        .command
        .send(ServerCommand::ApplyQuarantine {
            change,
            notify: false,
        })
        .await
        .context("the server of the worker is gone")
}

/// Writes the configuration of the master into the storage and reloads the server from it.
async fn apply_config(
    new_config: WorkerConfig,
    storage: &Arc<MemoryStorage>,
    channels: &crate::server::ServerChannels,
) -> anyhow::Result<()> {
    let certs = certs_of(&new_config.certs)?;
    storage
        .replace(
            new_config.app,
            new_config.ports,
            new_config.proxies,
            new_config.access_lists,
            certs,
        )
        .await;
    channels
        .command
        .send(ServerCommand::WorkerApply)
        .await
        .context("the server of the worker is gone")?;
    info!("applied the new configuration of the master");
    Ok(())
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
fn certs_of(certs: &[crate::ipc::WorkerCert]) -> anyhow::Result<Vec<Arc<crate::certs::Cert>>> {
    certs
        .iter()
        .cloned()
        .map(|cert| cert.into_cert().map(Arc::new).map_err(|err| anyhow!(err)))
        .collect()
}

fn app_info(config_dir: &Path, log_dir: &Path) -> anyhow::Result<AppInfo> {
    Ok(AppInfo {
        version: crate::release::version()?,
        target: "worker",
        profile: "worker",
        features: &[],
        rustc: "",
        config_path: config_dir.to_path_buf(),
        log_path: log_dir.to_path_buf(),
        platform_enabled: false,
        update_available: None,
    })
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
