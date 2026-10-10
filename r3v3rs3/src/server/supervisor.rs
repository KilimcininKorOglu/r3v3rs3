//! Supervises the worker process that serves the ports of this master.
//!
//! The supervisor owns the duplicated descriptors of the listeners and the IPC half of the
//! worker. It restarts the worker with a backoff when it dies, sends the current configuration
//! whenever the state changes, and pings it as a heartbeat.

use super::fdmanifest::ListenerFd;
use super::ipc::{
    Handshake, PROTOCOL_VERSION, WorkerCommand, WorkerConfig, WorkerEvent, read_frame, write_frame,
};
use crate::command::ServerCommand;
use crate::server::ServerEvent;
use anyhow::Context;
use r3v3rs3_api::app::{AppConfig, AppInfo};
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::process::Child;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

/// The master pings the worker in this interval.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// The first restart delay. It doubles up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(60);

/// Runs the supervisor until the master shuts down.
pub async fn run(
    bus_tx: broadcast::Sender<ServerEvent>,
    command: mpsc::Sender<ServerCommand>,
    exe: std::path::PathBuf,
    app_info: AppInfo,
    app: AppConfig,
    manifest: Vec<ListenerFd>,
) -> anyhow::Result<()> {
    let events = bus_tx.subscribe();
    if manifest.is_empty() {
        info!("no listeners to hand to a worker");
        return Ok(());
    }
    let (snapshot_tx, mut snapshot_rx) = mpsc::channel(1);
    command
        .send(ServerCommand::WorkerConfigSnapshot { reply: snapshot_tx })
        .await?;
    let config: WorkerConfig = snapshot_rx
        .recv()
        .await
        .ok_or_else(|| anyhow::anyhow!("the server did not answer the configuration request"))?;

    // The workers serve the ports, so this process stops accepting.
    command.send(ServerCommand::SuspendListeners).await?;

    supervise(
        events, bus_tx, command, exe, app_info, app, manifest, config,
    )
    .await
}

/// Serves the listeners through one worker at a time until the master shuts down.
#[allow(clippy::too_many_arguments)]
async fn supervise(
    mut events: broadcast::Receiver<ServerEvent>,
    bus_tx: broadcast::Sender<ServerEvent>,
    command: mpsc::Sender<ServerCommand>,
    exe: std::path::PathBuf,
    app_info: AppInfo,
    app: AppConfig,
    manifest: Vec<ListenerFd>,
    config: WorkerConfig,
) -> anyhow::Result<()> {
    let mut worker: Option<KillOnDrop> = None;
    let mut backoff = FIRST_BACKOFF;
    // A heartbeat with no answer since the last tick marks the worker as unreachable.
    let mut awaiting_status = false;
    let (events_tx, mut events_rx) = mpsc::channel::<WorkerEvent>(16);
    let (mut config_tx, mut config_rx) = mpsc::channel::<WorkerConfig>(1);
    let (mut ipc_tx, _ipc_rx) = mpsc::channel::<IpcMessage>(16);
    let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        if worker.is_none()
            && let Some(session) = spawn_worker(
                &exe,
                &app_info,
                &app,
                &manifest,
                &config,
                events_tx.clone(),
                &mut backoff,
            )
            .await
        {
            config_tx = session.config_tx;
            ipc_tx = session.ipc_tx;
            worker = Some(KillOnDrop(session.child));
        }
        tokio::select! {
            event = events.recv() => {
                if stop_or_refresh(event, &command, &mut config_tx).await? {
                    break;
                }
            }
            _ = heartbeat.tick() => {
                if heartbeat_tick(&mut worker, &command, &ipc_tx, &mut awaiting_status)
                    .await?
                {
                    restart_dead_worker(&mut worker, &mut backoff).await;
                }
            }
            event = events_rx.recv() => {
                handle_worker_event(
                    event,
                    &bus_tx,
                    &command,
                    &mut awaiting_status,
                    &mut worker,
                )
                .await;
            }
            config = config_rx.recv() => {
                let Some(config) = config else { continue };
                let _ = ipc_tx.send(IpcMessage::Config(Box::new(config))).await;
            }
        }
    }
    Ok(())
}

/// Starts a worker and returns it. A failed start waits out the backoff and returns `None`,
/// so the loop tries again.
async fn spawn_worker(
    exe: &std::path::Path,
    app_info: &AppInfo,
    app: &AppConfig,
    manifest: &[ListenerFd],
    config: &WorkerConfig,
    events_tx: mpsc::Sender<WorkerEvent>,
    backoff: &mut Duration,
) -> Option<WorkerSession> {
    info!("starting a worker");
    match start_worker(exe, app_info, app, manifest, config, events_tx).await {
        Ok(session) => {
            *backoff = FIRST_BACKOFF;
            Some(session)
        }
        Err(err) => {
            error!(%err, "failed to start a worker, retrying");
            tokio::time::sleep(*backoff).await;
            *backoff = (*backoff * 2).min(MAX_BACKOFF);
            None
        }
    }
}

/// Drops a worker that the heartbeat found dead and waits out the backoff.
async fn restart_dead_worker(worker: &mut Option<KillOnDrop>, backoff: &mut Duration) {
    *worker = None;
    tokio::time::sleep(*backoff).await;
    *backoff = (*backoff * 2).min(MAX_BACKOFF);
}

/// Runs one heartbeat. `true` means the worker exited, so the caller restarts it.
async fn heartbeat_tick(
    worker: &mut Option<KillOnDrop>,
    command: &mpsc::Sender<ServerCommand>,
    ipc_tx: &mpsc::Sender<IpcMessage>,
    awaiting_status: &mut bool,
) -> anyhow::Result<bool> {
    let Some(child) = worker.as_mut() else {
        return ping(ipc_tx).await;
    };
    if child.0.try_wait()?.is_some() {
        warn!("the worker exited, restarting it");
        *awaiting_status = false;
        return Ok(true);
    }
    let pid = child.0.id();
    if *awaiting_status {
        warn!("the worker missed its heartbeat answer");
        let _ = command
            .send(ServerCommand::SetWorkerStatus { pid, status: None })
            .await;
    }
    *awaiting_status = ipc_tx
        .send(IpcMessage::Command(WorkerCommand::Status))
        .await
        .is_ok();
    Ok(false)
}

/// A worker that has no process yet only gets a plain ping.
async fn ping(ipc_tx: &mpsc::Sender<IpcMessage>) -> anyhow::Result<bool> {
    if ipc_tx
        .send(IpcMessage::Command(WorkerCommand::Ping))
        .await
        .is_err()
    {
        warn!("the IPC task of the worker is gone");
    }
    Ok(false)
}

/// Carries one event of the worker into the state of the master.
async fn handle_worker_event(
    event: Option<WorkerEvent>,
    bus_tx: &broadcast::Sender<ServerEvent>,
    command: &mpsc::Sender<ServerCommand>,
    awaiting_status: &mut bool,
    worker: &mut Option<KillOnDrop>,
) {
    match event {
        Some(WorkerEvent::Fatal(reason)) => {
            error!(%reason, "the worker gave up");
            *worker = None;
        }
        // The runtime state that the worker serves flows into the event stream of the master,
        // so the admin API and the WebUI see it.
        Some(WorkerEvent::Event(event)) => {
            let _ = bus_tx.send(event);
        }
        Some(WorkerEvent::Status(status)) => {
            *awaiting_status = false;
            let pid = status.pid;
            let _ = command
                .send(ServerCommand::SetWorkerStatus {
                    pid,
                    status: Some(*status),
                })
                .await;
        }
        Some(WorkerEvent::Pong) | None => {}
    }
}

/// `true` ends the supervisor, because the master shuts down.
async fn stop_or_refresh(
    event: Result<ServerEvent, broadcast::error::RecvError>,
    command: &mpsc::Sender<ServerCommand>,
    config_tx: &mut mpsc::Sender<WorkerConfig>,
) -> anyhow::Result<bool> {
    match event {
        Ok(ServerEvent::Shutdown) | Err(broadcast::error::RecvError::Closed) => Ok(true),
        Err(broadcast::error::RecvError::Lagged(_)) => Ok(false),
        Ok(_) => {
            let (reply_tx, mut reply_rx) = mpsc::channel(1);
            command
                .send(ServerCommand::WorkerConfigSnapshot { reply: reply_tx })
                .await?;
            if let Some(config) = reply_rx.recv().await {
                let _ = config_tx.send(config).await;
            }
            Ok(false)
        }
    }
}

enum IpcMessage {
    Command(WorkerCommand),
    Config(Box<WorkerConfig>),
}

/// Stops the worker even when the supervisor stops early.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// One running worker: its process and the tasks that drive its channel.
struct WorkerSession {
    child: Child,
    config_tx: mpsc::Sender<WorkerConfig>,
    ipc_tx: mpsc::Sender<IpcMessage>,
}

/// Starts the worker process, sends the handshake and keeps the IPC half.
async fn start_worker(
    exe: &std::path::Path,
    app_info: &AppInfo,
    app: &AppConfig,
    manifest: &[ListenerFd],
    config: &WorkerConfig,
    events_tx: mpsc::Sender<WorkerEvent>,
) -> anyhow::Result<WorkerSession> {
    let (ipc, worker_half) = UnixStream::pair()?;
    let child = worker_command(exe, app_info, worker_half.as_raw_fd())
        .spawn()
        .context("failed to spawn the worker")?;
    std::mem::drop(worker_half);

    let (mut ipc, config, app) = (ipc, config.clone(), app.clone());
    let manifest = manifest
        .iter()
        .map(ListenerFd::try_clone)
        .collect::<std::io::Result<Vec<_>>>()?;
    // The handshake of a worker that starts slowly blocks, so it runs off the runtime.
    let session = tokio::task::spawn_blocking(move || -> anyhow::Result<WorkerSession> {
        handshake(&mut ipc, &app, &manifest, &config)?;
        let (config_tx, _config_rx) = mpsc::channel(1);
        let (ipc_tx, ipc_rx) = mpsc::channel(16);
        spawn_writer(ipc.try_clone()?, ipc_rx);
        spawn_reader(ipc.try_clone()?, events_tx);
        Ok(WorkerSession {
            child,
            config_tx,
            ipc_tx,
        })
    })
    .await
    .context("the worker task failed")??;
    Ok(session)
}

/// Builds the worker process command that reads its IPC socket on descriptor 3.
fn worker_command(
    exe: &std::path::Path,
    app_info: &AppInfo,
    ipc_fd: std::os::fd::RawFd,
) -> std::process::Command {
    let mut command = std::process::Command::new(exe);
    command
        .args([
            "worker",
            "--ipc",
            "3",
            "--config-dir",
            &app_info.config_path.display().to_string(),
            "--log-dir",
            &app_info.log_path.display().to_string(),
        ])
        .stdin(std::process::Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(move || {
            if libc::dup2(ipc_fd, 3) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command
}

/// Sends the handshake, the configuration and the descriptors to the worker.
fn handshake(
    ipc: &mut UnixStream,
    app: &AppConfig,
    manifest: &[ListenerFd],
    config: &WorkerConfig,
) -> anyhow::Result<()> {
    let _ = app;
    write_frame(
        ipc,
        &Handshake {
            protocol: PROTOCOL_VERSION,
            listeners: manifest
                .iter()
                .map(|listener| super::ipc::ListenerMeta {
                    proto: listener.proto,
                    addr: listener.addr,
                })
                .collect(),
        },
    )?;
    write_frame(ipc, config)?;
    super::fdpass::send_fds(
        ipc,
        &manifest
            .iter()
            .map(|listener| listener.fd.as_raw_fd())
            .collect::<Vec<_>>(),
    )?;
    Ok(())
}

/// Writes the messages of the master to the worker on a blocking thread.
fn spawn_writer(mut ipc: UnixStream, mut rx: mpsc::Receiver<IpcMessage>) {
    tokio::task::spawn_blocking(move || {
        while let Some(message) = rx.blocking_recv() {
            let result = match message {
                IpcMessage::Command(command) => write_frame(&mut ipc, &command),
                IpcMessage::Config(config) => write_frame(&mut ipc, &WorkerCommand::Config(config)),
            };
            if let Err(err) = result {
                error!(%err, "failed to write to the worker");
                break;
            }
        }
    });
}

/// Reads the events of the worker on a blocking thread.
fn spawn_reader(mut ipc: UnixStream, tx: mpsc::Sender<WorkerEvent>) {
    tokio::task::spawn_blocking(move || {
        loop {
            match read_frame::<WorkerEvent>(&mut ipc) {
                Ok(event) => {
                    if tx.blocking_send(event).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    info!(%err, "the event channel of the worker ended");
                    break;
                }
            }
        }
    });
}
