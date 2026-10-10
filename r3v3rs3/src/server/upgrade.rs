//! The zero-downtime upgrade: a SIGUSR2 hands the listeners to a new process.
//!
//! The old process forks itself with `R3V3RS3_UPGRADE_IPC_FD=3`. The new process loads its
//! state from the storage, answers with one ready byte, inherits the descriptors of the
//! listeners, and serves them while the old process drains its open connections.

use super::fdmanifest::ListenerFd;
use super::fdpass::recv_fds;
use super::ipc::{Handshake, read_frame, write_frame};
use anyhow::{Context, bail};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;
use tracing::info;

/// The child answers this byte when its state is loaded and it can inherit the listeners.
const READY: u8 = b'R';

/// The longest time that the old process waits for the new one to become ready.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// The side of the new process: announces that it is ready and inherits the listeners.
pub async fn child_handover(ipc_fd: i32) -> anyhow::Result<Vec<ListenerFd>> {
    let mut ipc = UnixStream::from(unsafe { OwnedFd::from_raw_fd(ipc_fd) });
    ipc.set_nonblocking(false)
        .context("failed to configure the upgrade socket")?;
    use std::io::Write as _;
    ipc.write_all(&[READY])
        .context("failed to announce the readiness")?;
    let handshake: Handshake = read_frame(&mut ipc)?;
    if handshake.protocol != super::ipc::PROTOCOL_VERSION {
        bail!("the previous process speaks another upgrade protocol");
    }
    let fds = recv_fds(&ipc, handshake.listeners.len())?;
    Ok(handshake
        .listeners
        .iter()
        .zip(fds)
        .map(|(meta, fd)| ListenerFd::new(meta.proto, meta.addr, fd))
        .collect())
}

/// The side of the old process: starts the new one and hands the listeners over.
pub async fn hand_over(exe_args: &[String], manifest: Vec<ListenerFd>) -> anyhow::Result<()> {
    let (mut ipc, child_half) = UnixStream::pair()?;
    let raw = std::os::fd::AsRawFd::as_raw_fd(&child_half);
    let exe = std::env::current_exe().context("failed to locate the server binary")?;
    let mut command = std::process::Command::new(exe);
    command
        .args(exe_args)
        .env("R3V3RS3_UPGRADE_IPC_FD", "3")
        .stdin(std::process::Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(move || {
            if libc::dup2(raw, 3) == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    // The new process runs on its own; this one only hands the listeners over and drains.
    let child = command.spawn().context("failed to start the new process")?;
    let new_pid = child.id() as i32;
    std::mem::drop(child);
    std::mem::drop(child_half);

    let handover = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        use std::io::Read as _;
        let mut ready = [0u8; 1];
        ipc.read_exact(&mut ready)
            .context("the new process did not answer")?;
        if ready[0] != READY {
            bail!("the new process sent an unexpected answer");
        }
        write_frame(
            &mut ipc,
            &Handshake {
                protocol: super::ipc::PROTOCOL_VERSION,
                listeners: manifest
                    .iter()
                    .map(|listener| super::ipc::ListenerMeta {
                        proto: listener.proto,
                        addr: listener.addr,
                    })
                    .collect(),
            },
        )?;
        let fds = manifest
            .iter()
            .map(|listener| listener.raw_fd())
            .collect::<Vec<_>>();
        super::fdpass::send_fds(&ipc, &fds)?;
        Ok(())
    });
    match tokio::time::timeout(READY_TIMEOUT, handover).await {
        Ok(result) => result??,
        Err(_) => {
            unsafe {
                libc::kill(new_pid, libc::SIGKILL);
            }
            bail!("the new process did not become ready in time");
        }
    }
    info!("the listeners went to the new process, this one drains");
    Ok(())
}
