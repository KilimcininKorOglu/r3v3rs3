//! The zero-downtime upgrade: a SIGUSR2 hands the listeners to a new process.
//!
//! The old process forks itself with `R3V3RS3_UPGRADE_IPC_FD=3`. The new process loads its
//! state from the storage, answers with one ready byte, inherits the descriptors of the
//! listeners, and serves them while the old process drains its open connections.

use super::fdmanifest::ListenerFd;
use super::fdpass::recv_fds;
use super::ipc::{Handshake, read_frame, write_frame};
use anyhow::{Context, bail};
use std::os::fd::{AsRawFd as _, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;
use tracing::info;

/// The child answers this byte when its state is loaded and it can inherit the listeners.
const READY: u8 = b'R';

/// The longest time that the old process waits for the new one to become ready.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// The largest number of listeners that one handover passes.
const MAX_LISTENERS: usize = 1024;

/// The side of the new process: announces that it is ready and inherits the listeners.
pub async fn child_handover(ipc_fd: i32) -> anyhow::Result<Vec<ListenerFd>> {
    let mut ipc = UnixStream::from(unsafe { OwnedFd::from_raw_fd(ipc_fd) });
    ipc.set_nonblocking(false)
        .context("failed to configure the upgrade socket")?;
    use std::io::Write as _;
    ipc.write_all(&[READY])
        .context("failed to announce the readiness")?;
    // The descriptors travel before the metadata, so that every read of this socket starts
    // at the beginning of one message. A read that starts inside another message drops the
    // ancillary data that the descriptor message carries.
    let fds = recv_fds(&ipc, MAX_LISTENERS).context("failed to receive the listeners")?;
    let handshake: Handshake = read_frame(&mut ipc)?;
    if handshake.protocol != super::ipc::PROTOCOL_VERSION {
        bail!("the previous process speaks another upgrade protocol");
    }
    Ok(handshake
        .listeners
        .iter()
        .cloned()
        .zip(fds)
        .map(|(meta, fd)| ListenerFd::new(meta.proto, meta.addr, fd))
        .collect())
}

/// The side of the old process: starts the new one and hands the listeners over.
pub async fn hand_over(exe_args: &[String], manifest: &[ListenerFd]) -> anyhow::Result<()> {
    let (mut ipc, child_half) = UnixStream::pair()?;
    let mut command = new_upgrade_command(exe_args, child_half.as_raw_fd())?;
    // The new process runs on its own; this one only hands the listeners over and drains.
    let child = command.spawn().context("failed to start the new process")?;
    let new_pid = child.id() as i32;
    std::mem::drop(child);
    std::mem::drop(child_half);

    let metas = manifest
        .iter()
        .map(|listener| super::ipc::ListenerMeta {
            proto: listener.proto,
            addr: listener.addr,
        })
        .collect::<Vec<_>>();
    let fds = manifest
        .iter()
        .map(|listener| listener.raw_fd())
        .collect::<Vec<_>>();
    let handover = tokio::task::spawn_blocking(move || write_handover(&mut ipc, &metas, &fds));
    match tokio::time::timeout(READY_TIMEOUT, handover).await {
        Ok(result) => {
            result.map_err(|err| anyhow::anyhow!("the handover task failed: {err}"))??
        }
        Err(_) => {
            unsafe {
                libc::kill(new_pid, libc::SIGKILL);
            }
            bail!("the new process did not become ready in time");
        }
    }
    info!("the listeners went to the new process, this one drains");
    // systemd tracks the old process as the main one, so it must learn the successor before
    // this process exits and the cgroup of the service stays alive.
    super::notify::hand_over_main_pid(new_pid as u32);
    Ok(())
}

/// The path of the binary that the next process runs. When the file on disk was replaced,
/// the kernel reports the old inode as `<path> (deleted)`; the upgrade wants the binary that
/// carries the path now, so the suffix goes.
fn upgrade_exe() -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe().context("failed to locate the server binary")?;
    Ok(strip_deleted(exe))
}

/// Strips the `(deleted)` marker that the kernel appends to a replaced binary path.
fn strip_deleted(exe: std::path::PathBuf) -> std::path::PathBuf {
    let path = exe.to_string_lossy();
    match path.strip_suffix(" (deleted)") {
        Some(path) => path.into(),
        None => exe,
    }
}

/// The command of the new process that reads its IPC socket on descriptor 3.
fn new_upgrade_command(
    exe_args: &[String],
    ipc_fd: std::os::fd::RawFd,
) -> anyhow::Result<std::process::Command> {
    let exe = upgrade_exe()?;
    let mut command = std::process::Command::new(exe);
    command
        .args(exe_args)
        .env("R3V3RS3_UPGRADE_IPC_FD", "3")
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
    Ok(command)
}

/// Waits for the ready byte of the new process, sends the descriptors, and then the metadata.
/// The descriptors go first, so that the read of the other end starts at the beginning of the
/// descriptor message and keeps its ancillary data.
fn write_handover(
    ipc: &mut UnixStream,
    listeners: &[super::ipc::ListenerMeta],
    fds: &[std::os::fd::RawFd],
) -> anyhow::Result<()> {
    use std::io::Read as _;
    let mut ready = [0u8; 1];
    ipc.read_exact(&mut ready)
        .context("the new process did not answer")?;
    if ready[0] != READY {
        bail!("the new process sent an unexpected answer");
    }
    super::fdpass::send_fds(ipc, fds)?;
    write_frame(
        ipc,
        &Handshake {
            protocol: super::ipc::PROTOCOL_VERSION,
            listeners: listeners.to_vec(),
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replaced_binary_path_loses_its_deleted_marker() {
        let path = std::path::PathBuf::from("/usr/local/bin/r3v3rs3 (deleted)");
        assert_eq!(
            strip_deleted(path),
            std::path::PathBuf::from("/usr/local/bin/r3v3rs3")
        );
        let path = std::path::PathBuf::from("/usr/local/bin/r3v3rs3");
        assert_eq!(strip_deleted(path.clone()), path);
    }
}
